use crate::{
    CachePolicy, Identity, MetricDescriptor, MetricKind, MetricTier, PreparedSetup,
    ProviderDescriptor, ReportCache, Secret, SecretMap, SetupContext, SetupField, SetupFieldKind,
    SetupInput, SetupSchema, TrackerContext, TrackerError, TrackerProvider, UsageMetric,
    UsageReport,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

#[derive(Default)]
pub struct CodexProvider {
    endpoint_override: Option<String>,
}

fn invalid() -> TrackerError {
    TrackerError::new(
        "invalid_provider_response",
        "Codex returned invalid usage data",
    )
}
fn authentication() -> TrackerError {
    TrackerError::new(
        "authentication_failed",
        "Codex credential is unavailable or rejected; sign in again and update the tracker credential",
    )
}

impl CodexProvider {
    /// Suggest an existing upstream credential file during interactive onboarding.
    /// The caller persists the selected absolute path; collection never rediscovers it.
    pub fn discover_credential_file() -> Option<std::path::PathBuf> {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        let configured = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from);
        Self::credential_path(&home, configured.as_deref()).filter(|path| path.is_file())
    }

    fn credential_path(
        home: &std::path::Path,
        configured: Option<&std::path::Path>,
    ) -> Option<std::path::PathBuf> {
        let directory =
            configured.map_or_else(|| home.join(".codex"), std::path::Path::to_path_buf);
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir().ok()?.join(directory)
        };
        Some(directory.join("auth.json"))
    }

    fn credentials(
        settings: &Map<String, Value>,
        secrets: &SecretMap,
    ) -> Result<(String, Option<String>), TrackerError> {
        if let Some(path) = settings.get("credential_file").and_then(Value::as_str) {
            let bytes = std::fs::read(path).map_err(|_| authentication())?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| authentication())?;
            let token = value
                .pointer("/tokens/access_token")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(authentication)?;
            let account = value
                .pointer("/tokens/account_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Ok((token.to_owned(), account))
        } else {
            let token = secrets
                .get("token")
                .filter(|s| !s.expose().trim().is_empty())
                .ok_or_else(authentication)?;
            Ok((
                token.expose().to_owned(),
                settings
                    .get("account_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ))
        }
    }

    async fn fetch(
        &self,
        http: &reqwest::Client,
        token: &str,
        account: Option<&str>,
    ) -> Result<UsageReport, TrackerError> {
        #[cfg(test)]
        let test_http = reqwest::Client::new();
        #[cfg(test)]
        let http = if self.endpoint_override.is_some() {
            &test_http
        } else {
            http
        };
        let mut request = http
            .get(
                self.endpoint_override
                    .as_deref()
                    .unwrap_or("https://chatgpt.com/backend-api/wham/usage"),
            )
            .bearer_auth(token);
        if let Some(account) = account {
            request = request.header("ChatGPT-Account-Id", account);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                TrackerError::new("timeout", "Codex usage request timed out")
            } else {
                TrackerError::new("transport_error", "Codex usage request failed")
            }
        })?;
        match response.status().as_u16() {
            200..=299 => {}
            401 => {
                return Err(TrackerError::new(
                    "authentication_failed",
                    "Could not refresh Codex usage: the access token expired or was rejected. Sign in with `codex login` for this subscription, then retry `yaait usage --refresh`. If this tracker uses a supplied token, replace it with `yaait setup <TRACKER_ID>`.",
                ));
            }
            403 => {
                return Err(TrackerError::new(
                    "authentication_failed",
                    "Could not refresh Codex usage: access was denied. Check this subscription's credential and ChatGPT account/workspace ID.",
                ));
            }
            429 => {
                return Err(TrackerError::new(
                    "rate_limited",
                    "Codex usage endpoint is rate limited",
                ));
            }
            status => return Err(invalid().detail("http_status", u64::from(status))),
        }
        let payload: Value = response.json().await.map_err(|_| invalid())?;
        let mut report = parse_report(payload)?;
        if let Some(account) = account {
            report
                .identity
                .get_or_insert_with(Identity::default)
                .account = Some(account.into());
        }
        crate::validate_report(&report)?;
        Ok(report)
    }
}

#[async_trait]
impl TrackerProvider for CodexProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        let mut descriptor = ProviderDescriptor {
            id: "codex".parse().expect("static provider ID"), name: "Codex".into(),
            description: "5-hour and weekly subscription limits and reset times.".into(),
            setup: SetupSchema { fields: vec![
                SetupField { key: "token".into(), label: "Subscription access token".into(), description: "OAuth access token, not an API key. Supply either this or a credential file.".into(), kind: SetupFieldKind::Secret, required: false, allowed_values: None },
                SetupField { key: "credential_file".into(), label: "Credential file".into(), description: "Absolute path to this subscription's Codex auth.json. Read again on each fresh fetch; the upstream CLI manages token refresh.".into(), kind: SetupFieldKind::Path, required: false, allowed_values: None },
                SetupField { key: "account_id".into(), label: "ChatGPT account ID".into(), description: "Account or workspace ID for a supplied token; read from credential files automatically.".into(), kind: SetupFieldKind::String, required: false, allowed_values: None },
            ] },
            metrics: [("primary", "5-hour limit", MetricTier::Primary), ("secondary", "Weekly limit", MetricTier::Primary), ("code-review", "Code review", MetricTier::Detail)].into_iter().map(|(id,label,tier)| MetricDescriptor { id: id.into(), label: label.into(), description: "Percentage of the subscription window used.".into(), kind: MetricKind::Quota, tier, unit: "percent".into() }).collect(),
        };
        descriptor.metrics.push(MetricDescriptor {
            id: "credits".into(),
            label: "Credit balance".into(),
            description: "Available Codex credits when reported; currency is unspecified.".into(),
            kind: MetricKind::Balance,
            tier: MetricTier::Primary,
            unit: "credit".into(),
        });
        descriptor
    }
    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        mut input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError> {
        crate::validate_setup_input(&self.descriptor().setup, &input)?;
        let token = input
            .remove("token")
            .and_then(|v| v.as_str().map(str::to_owned));
        let path = input.get("credential_file").and_then(Value::as_str);
        if token.is_some() == path.is_some() {
            return Err(TrackerError::invalid(
                "supply exactly one of token or credential_file",
            ));
        }
        if path.is_some_and(|p| !std::path::Path::new(p).is_absolute()) {
            return Err(TrackerError::invalid(
                "credential_file must be an absolute path",
            ));
        }
        let mut secrets = SecretMap::new();
        if let Some(token) = token {
            secrets.insert("token".into(), Secret::new(token));
        }
        let (token, account) = Self::credentials(&input, &secrets)?;
        if let Some(account) = &account {
            input.insert("account_id".into(), Value::String(account.clone()));
        }
        let report = self.fetch(&ctx.http, &token, account.as_deref()).await?;
        Ok(PreparedSetup {
            public_settings: input,
            secrets,
            initial_report: Some(report),
        })
    }
    async fn collect(
        &self,
        ctx: &TrackerContext,
        policy: CachePolicy,
    ) -> Result<UsageReport, TrackerError> {
        let cache = ReportCache::new(ctx.cache_dir.clone());
        if policy == CachePolicy::Cached
            && let Some(report) = cache.fresh(crate::DEFAULT_CACHE_TTL, Utc::now())
        {
            return Ok(report);
        }
        let (token, account) = Self::credentials(&ctx.tracker.settings, &ctx.credentials)?;
        if ctx
            .tracker
            .settings
            .get("account_id")
            .and_then(Value::as_str)
            != account.as_deref()
        {
            return Err(TrackerError::new(
                "authentication_failed",
                "Codex credential file now belongs to a different account; configure a separate tracker",
            ));
        }
        let report = self.fetch(&ctx.http, &token, account.as_deref()).await?;
        cache.store(&report);
        Ok(report)
    }
}

fn metric(
    id: &str,
    label: &str,
    tier: MetricTier,
    used: f64,
    period: Option<String>,
    reset: Option<DateTime<Utc>>,
) -> Result<UsageMetric, TrackerError> {
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return Err(invalid());
    }
    Ok(UsageMetric {
        id: id.into(),
        label: label.into(),
        kind: MetricKind::Quota,
        tier,
        unit: "percent".into(),
        used: Some(used),
        remaining: Some(100.0 - used),
        limit: Some(100.0),
        value: None,
        period,
        resets_at: reset,
        attributes: Map::new(),
    })
}

fn parse_report(payload: Value) -> Result<UsageReport, TrackerError> {
    if !payload.is_object()
        || (payload.get("rate_limit").is_none()
            && payload.get("code_review_rate_limit").is_none()
            && payload.get("credits").is_none())
    {
        return Err(invalid());
    }
    let mut metrics = vec![];
    for (id, label, pointer, tier) in [
        (
            "primary",
            "5-hour limit",
            "/rate_limit/primary_window",
            MetricTier::Primary,
        ),
        (
            "secondary",
            "Weekly limit",
            "/rate_limit/secondary_window",
            MetricTier::Primary,
        ),
        (
            "code-review",
            "Code review",
            "/code_review_rate_limit/primary_window",
            MetricTier::Detail,
        ),
    ] {
        let Some(window) = payload.pointer(pointer).filter(|v| !v.is_null()) else {
            continue;
        };
        let used = window
            .get("used_percent")
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?;
        let reset = window
            .get("reset_at")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_i64()
                    .and_then(|s| DateTime::from_timestamp(s, 0))
                    .ok_or_else(invalid)
            })
            .transpose()?;
        let period = window
            .get("limit_window_seconds")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_u64()
                    .filter(|s| *s > 0)
                    .map(|s| format!("{s}s"))
                    .ok_or_else(invalid)
            })
            .transpose()?;
        metrics.push(metric(id, label, tier, used, period, reset)?);
    }
    let mut attributes = Map::new();
    if let Some(credits) = payload.get("credits").filter(|v| !v.is_null()) {
        let object = credits.as_object().ok_or_else(invalid)?;
        let mut metadata = Map::new();
        for key in ["has_credits", "unlimited"] {
            if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
                if !value.is_boolean() {
                    return Err(invalid());
                }
                metadata.insert(key.into(), value.clone());
            }
        }
        if !metadata.is_empty() {
            attributes.insert("credits".into(), Value::Object(metadata));
        }
        if let Some(value) = object.get("balance").filter(|v| !v.is_null()) {
            let balance = value
                .as_f64()
                .or_else(|| value.as_str()?.parse::<f64>().ok())
                .ok_or_else(invalid)?;
            if !balance.is_finite() || balance < 0.0 {
                return Err(invalid());
            }
            metrics.push(UsageMetric {
                id: "credits".into(),
                label: "Credit balance".into(),
                kind: MetricKind::Balance,
                tier: MetricTier::Primary,
                unit: "credit".into(),
                used: None,
                remaining: None,
                limit: None,
                value: Some(balance),
                period: None,
                resets_at: None,
                attributes: Map::new(),
            });
        }
    }
    Ok(UsageReport {
        observed_at: Utc::now(),
        identity: payload
            .get("plan_type")
            .and_then(Value::as_str)
            .map(|plan| Identity {
                plan: Some(plan.into()),
                ..Identity::default()
            }),
        metrics,
        attributes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};
    use std::collections::BTreeMap;
    fn fixture() -> Value {
        serde_json::json!({"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_at":1800000000},"secondary_window":{"used_percent":70,"limit_window_seconds":604800,"reset_at":1800000001}},"code_review_rate_limit":{"primary_window":{"used_percent":5}},"credits":{"balance":"12.5","has_credits":true,"unlimited":false}})
    }
    fn context(id: &str, token: &str, cache_dir: std::path::PathBuf) -> TrackerContext {
        let now = Utc::now();
        let mut credentials = SecretMap::new();
        credentials.insert("token".into(), Secret::new(token));
        TrackerContext {
            tracker: crate::TrackerManifest {
                schema_version: 1,
                id: id.parse().unwrap(),
                provider: "codex".parse().unwrap(),
                name: id.into(),
                description: None,
                enabled: true,
                created_at: now,
                updated_at: now,
                settings: Map::new(),
                extensions: BTreeMap::new(),
            },
            data_dir: cache_dir.clone(),
            cache_dir,
            credentials,
            http: reqwest::Client::new(),
        }
    }
    #[test]
    fn maps_separate_windows_and_preserves_unknown_resets() {
        let report = parse_report(fixture()).unwrap();
        crate::validate_report(&report).unwrap();
        assert_eq!(report.metrics.len(), 4);
        assert_eq!(report.metrics[0].id, "primary");
        assert_eq!(report.metrics[0].label, "5-hour limit");
        assert_eq!(report.metrics[0].used, Some(25.0));
        assert_eq!(report.metrics[0].remaining, Some(75.0));
        assert_eq!(report.metrics[0].unit, "percent");
        assert!(report.metrics[0].resets_at.is_some());
        assert_eq!(report.metrics[1].id, "secondary");
        assert_eq!(report.metrics[1].label, "Weekly limit");
        assert_eq!(report.metrics[1].remaining, Some(30.0));
        assert_eq!(report.metrics[2].tier, MetricTier::Detail);
        assert_eq!(report.metrics[2].resets_at, None);
    }
    #[test]
    fn descriptor_uses_the_same_limit_labels_as_reports() {
        let descriptor = CodexProvider::default().descriptor();
        let report = parse_report(fixture()).unwrap();
        for metric in report.metrics.iter().take(2) {
            let described = descriptor
                .metrics
                .iter()
                .find(|item| item.id == metric.id)
                .unwrap();
            assert_eq!(described.label, metric.label);
        }
    }
    #[test]
    fn credential_discovery_respects_each_cli_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            CodexProvider::credential_path(temp.path(), None).unwrap(),
            temp.path().join(".codex/auth.json")
        );
        let other = temp.path().join("work-subscription");
        assert_eq!(
            CodexProvider::credential_path(temp.path(), Some(&other)).unwrap(),
            other.join("auth.json")
        );
    }

    #[test]
    fn credits_accept_numeric_or_string_balances_and_preserve_unknowns() {
        for balance in [serde_json::json!(12.5), serde_json::json!("12.5")] {
            let report =
                parse_report(serde_json::json!({"credits":{"balance":balance,"has_credits":true}}))
                    .unwrap();
            crate::validate_report(&report).unwrap();
            assert_eq!(report.metrics[0].id, "credits");
            assert_eq!(report.metrics[0].value, Some(12.5));
            assert_eq!(report.metrics[0].unit, "credit");
            assert_eq!(report.metrics[0].tier, MetricTier::Primary);
            assert_eq!(report.attributes["credits"]["has_credits"], true);
            assert!(report.attributes["credits"].get("unlimited").is_none());
        }
        for payload in [
            serde_json::json!({"credits":{}}),
            serde_json::json!({"credits":{"balance":null}}),
            serde_json::json!({"credits":{"unlimited":true}}),
        ] {
            let report = parse_report(payload).unwrap();
            assert!(report.metrics.is_empty());
        }
        for balance in [
            serde_json::json!(-1),
            serde_json::json!("NaN"),
            serde_json::json!("sensitive"),
            serde_json::json!(true),
        ] {
            let error =
                parse_report(serde_json::json!({"credits":{"balance":balance}})).unwrap_err();
            assert!(!error.message.contains("sensitive"));
        }
    }

    #[test]
    fn unavailable_windows_remain_unknown() {
        let report =
            parse_report(serde_json::json!({"rate_limit":null, "code_review_rate_limit":null}))
                .unwrap();
        assert!(report.metrics.is_empty());
        crate::validate_report(&report).unwrap();
    }

    #[test]
    fn rejects_unrelated_and_invalid_payloads_without_leaking_data() {
        for payload in [
            serde_json::json!({"secret":"sensitive"}),
            serde_json::json!([]),
            serde_json::json!({"rate_limit":{"primary_window":{"used_percent":101}}}),
        ] {
            let error = parse_report(payload).unwrap_err();
            assert_eq!(error.code, "invalid_provider_response");
            assert!(!format!("{error:?}").contains("sensitive"));
        }
    }
    #[tokio::test]
    async fn validates_setup_and_separates_account_caches() {
        let server = MockServer::start_async().await;
        let personal = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer personal-token");
                then.status(200).json_body(fixture());
            })
            .await;
        let work = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer work-token");
                let mut payload = fixture();
                payload["rate_limit"]["primary_window"]["used_percent"] = Value::from(90);
                then.status(200).json_body(payload);
            })
            .await;
        let provider = CodexProvider {
            endpoint_override: Some(server.url("/usage")),
        };
        let temp = tempfile::tempdir().unwrap();
        let setup = SetupContext {
            tracker_id: "personal".parse().unwrap(),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("setup"),
            http: reqwest::Client::new(),
        };
        let prepared = provider
            .validate_setup(
                &setup,
                serde_json::json!({"token":"personal-token"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .await
            .unwrap();
        assert!(prepared.public_settings.get("token").is_none());
        assert_eq!(prepared.secrets["token"].expose(), "personal-token");
        let a = context("personal", "personal-token", temp.path().join("personal"));
        let b = context("work", "work-token", temp.path().join("work"));
        let first = provider.collect(&a, CachePolicy::Cached).await.unwrap();
        let second = provider.collect(&b, CachePolicy::Cached).await.unwrap();
        assert_eq!(first.metrics[0].remaining, Some(75.0));
        assert_eq!(second.metrics[0].remaining, Some(10.0));
        let cached = provider.collect(&a, CachePolicy::Cached).await.unwrap();
        assert_eq!(cached.observed_at, first.observed_at);
        assert_eq!(personal.calls_async().await, 2);
        assert_eq!(work.calls_async().await, 1);
        provider.collect(&a, CachePolicy::Refresh).await.unwrap();
        assert_eq!(personal.calls_async().await, 3);
    }
    #[tokio::test]
    async fn classifies_http_errors_without_exposing_response_bodies() {
        let server = MockServer::start_async().await;
        let provider = CodexProvider {
            endpoint_override: Some(server.url("/usage")),
        };
        for (status, code) in [
            (401, "authentication_failed"),
            (403, "authentication_failed"),
            (429, "rate_limited"),
            (500, "invalid_provider_response"),
        ] {
            let mock = server
                .mock_async(|when, then| {
                    when.method(GET).path("/usage");
                    then.status(status).body("secret-token");
                })
                .await;
            let error = provider
                .fetch(&reqwest::Client::new(), "secret-token", None)
                .await
                .unwrap_err();
            assert_eq!(error.code, code);
            if status == 401 {
                assert!(error.message.contains("expired or was rejected"));
                assert!(error.message.contains("codex login"));
            }
            assert!(!format!("{error:?}").contains("secret-token"));
            mock.delete_async().await;
        }
    }
    #[tokio::test]
    async fn credential_file_pins_account_and_rejects_a_switch() {
        let server = MockServer::start_async().await;
        let request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer original-token")
                    .header("chatgpt-account-id", "original-account");
                then.status(200).json_body(fixture());
            })
            .await;
        let provider = CodexProvider {
            endpoint_override: Some(server.url("/usage")),
        };
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("auth.json");
        std::fs::write(&file, serde_json::to_vec(&serde_json::json!({"tokens":{"access_token":"original-token", "account_id":"original-account"}})).unwrap()).unwrap();
        let setup = SetupContext {
            tracker_id: "personal".parse().unwrap(),
            data_dir: temp.path().into(),
            cache_dir: temp.path().join("setup"),
            http: reqwest::Client::new(),
        };
        let prepared = provider
            .validate_setup(
                &setup,
                serde_json::json!({"credential_file":file.to_str().unwrap()})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .await
            .unwrap();
        assert_eq!(prepared.public_settings["account_id"], "original-account");
        assert!(prepared.secrets.is_empty());
        let mut ctx = context("personal", "unused", temp.path().join("cache"));
        ctx.tracker.settings = prepared.public_settings;
        ctx.credentials.clear();
        let report = provider.collect(&ctx, CachePolicy::Refresh).await.unwrap();
        assert_eq!(
            report.identity.unwrap().account.as_deref(),
            Some("original-account")
        );
        std::fs::write(file, serde_json::to_vec(&serde_json::json!({"tokens":{"access_token":"new-token", "account_id":"new-account"}})).unwrap()).unwrap();
        let error = provider
            .collect(&ctx, CachePolicy::Refresh)
            .await
            .unwrap_err();
        assert_eq!(error.code, "authentication_failed");
        assert_eq!(request.calls_async().await, 2);
    }

    #[tokio::test]
    async fn requires_exactly_one_credential_source() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = SetupContext {
            tracker_id: "test".parse().unwrap(),
            data_dir: temp.path().into(),
            cache_dir: temp.path().into(),
            http: reqwest::Client::new(),
        };
        for input in [
            serde_json::json!({}),
            serde_json::json!({"token":"secret", "credential_file":"/tmp/file"}),
            serde_json::json!({"credential_file":"relative"}),
            serde_json::json!({"token":"secret", "unexpected":"public-secret"}),
            serde_json::json!({"token":"secret", "credential_file":42}),
        ] {
            assert_eq!(
                CodexProvider::default()
                    .validate_setup(&ctx, input.as_object().unwrap().clone())
                    .await
                    .unwrap_err()
                    .code,
                "invalid_input"
            );
        }
    }
    #[tokio::test]
    async fn application_renders_both_views_and_keeps_partial_results() {
        let server = MockServer::start_async().await;
        let good = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer good");
                then.status(200).json_body(fixture());
            })
            .await;
        let other = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer other");
                then.status(200).json_body(fixture());
            })
            .await;
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::App::new(crate::FileRegistry::new(crate::AppPaths::isolated(
            temp.path(),
        )))
        .unwrap();
        app.register_provider(std::sync::Arc::new(CodexProvider {
            endpoint_override: Some(server.url("/usage")),
        }));
        for (id, token) in [("personal", "good"), ("work", "other")] {
            app.add(crate::AddRequest {
                id: id.parse().unwrap(),
                provider: "codex".parse().unwrap(),
                name: None,
                description: None,
                input: serde_json::json!({"token": token})
                    .as_object()
                    .unwrap()
                    .clone(),
            })
            .await
            .unwrap();
        }
        for details in [false, true] {
            let result = app
                .usage(
                    &[],
                    crate::UsageOptions {
                        details,
                        ..crate::UsageOptions::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(result.data.trackers.len(), 2);
            assert_eq!(
                result.data.trackers[0].metrics.len(),
                if details { 4 } else { 3 }
            );
            let envelope =
                crate::presentation::Envelope::usage(result.data, result.warnings, vec![]);
            let json: Value =
                serde_json::from_str(&serde_json::to_string(&envelope).unwrap()).unwrap();
            assert_eq!(json["schema_version"], 2);
            assert_eq!(json["data"]["trackers"][0]["id"], "personal");
            assert_eq!(json["data"]["trackers"][1]["id"], "work");
            assert_eq!(json["data"]["trackers"][0]["metrics"][0]["remaining"], 75.0);
            let metrics = json["data"]["trackers"][0]["metrics"].as_array().unwrap();
            assert_eq!(metrics[0]["id"], "primary");
            assert_eq!(metrics[0]["label"], "5-hour limit");
            assert_eq!(metrics[1]["id"], "secondary");
            assert_eq!(metrics[1]["label"], "Weekly limit");
            let credit = metrics.iter().find(|m| m["id"] == "credits").unwrap();
            assert_eq!(credit["value"], 12.5);
            assert_eq!(credit["unit"], "credit");
            let human = crate::presentation::human::render(&envelope);
            assert!(human.stdout.contains("personal:"));
            assert!(human.stdout.contains("work:"));
            assert!(human.stdout.contains("25 of 100"));
            assert!(human.stdout.contains("5-hour limit:"));
            assert!(human.stdout.contains("Weekly limit:"));
            assert_eq!(human.stdout.contains("Code review"), details);
            assert!(human.stderr.is_empty());
            assert!(human.stdout.contains("Credit balance"));
        }
        other.delete_async().await;
        let failure = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer other");
                then.status(401).body("other");
            })
            .await;
        let result = app
            .usage(
                &[],
                crate::UsageOptions {
                    details: true,
                    cache: CachePolicy::Refresh,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.data.trackers.len(), 1);
        assert_eq!(result.data.trackers[0].id.as_str(), "personal");
        let envelope =
            crate::presentation::Envelope::usage(result.data, result.warnings, result.errors);
        assert!(envelope.partial);
        assert!(
            envelope.errors[0]
                .message
                .contains("Could not refresh Codex usage")
        );
        assert!(
            envelope.errors[0]
                .message
                .contains("expired or was rejected")
        );
        let human = crate::presentation::human::render(&envelope);
        assert!(human.stderr.contains("Could not refresh Codex usage"));
        assert!(!human.stdout.contains("work:"));
        assert_eq!(envelope.exit_code(), 2);
        assert_eq!(envelope.errors[0].tracker_id.as_deref(), Some("work"));
        failure.assert_async().await;
        assert_eq!(good.calls_async().await, 2);
    }

    #[test]
    fn rereads_refreshed_credentials_without_printing_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("credentials.json");
        let settings = serde_json::json!({"credential_file":file.to_str().unwrap()})
            .as_object()
            .unwrap()
            .clone();
        for token in ["first", "refreshed"] {
            let value = serde_json::json!({"tokens":{"access_token":token,"account_id":"account"}});
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                CodexProvider::credentials(&settings, &SecretMap::new())
                    .unwrap()
                    .0,
                token
            );
        }
        std::fs::write(file, "invalid sensitive contents").unwrap();
        let error = CodexProvider::credentials(&settings, &SecretMap::new()).unwrap_err();
        assert!(!format!("{error:?}").contains("sensitive"));
    }
}
