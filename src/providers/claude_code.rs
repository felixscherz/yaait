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
pub struct ClaudeCodeProvider {
    endpoint_override: Option<String>,
}

fn invalid() -> TrackerError {
    TrackerError::new(
        "invalid_provider_response",
        "Claude Code returned invalid usage data",
    )
}
fn authentication() -> TrackerError {
    TrackerError::new(
        "authentication_failed",
        "Claude Code credential is unavailable or rejected; sign in again and update the tracker credential",
    )
}

impl ClaudeCodeProvider {
    /// Suggest an existing upstream credential file during interactive onboarding.
    /// The caller persists the selected absolute path; collection never rediscovers it.
    pub fn discover_credential_file() -> Option<std::path::PathBuf> {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        let configured = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from);
        Self::credential_path(&home, configured.as_deref()).filter(|path| path.is_file())
    }

    fn credential_path(
        home: &std::path::Path,
        configured: Option<&std::path::Path>,
    ) -> Option<std::path::PathBuf> {
        let directory =
            configured.map_or_else(|| home.join(".claude"), std::path::Path::to_path_buf);
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir().ok()?.join(directory)
        };
        Some(directory.join(".credentials.json"))
    }

    fn credentials(
        settings: &Map<String, Value>,
        secrets: &SecretMap,
    ) -> Result<(String, Option<String>), TrackerError> {
        if let Some(path) = settings.get("credential_file").and_then(Value::as_str) {
            let bytes = std::fs::read(path).map_err(|_| authentication())?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| authentication())?;
            let token = value
                .pointer("/claudeAiOauth/accessToken")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(authentication)?;
            let account = value
                .pointer("/claudeAiOauth/subscriptionType")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Ok((token.to_owned(), account))
        } else {
            let token = secrets
                .get("token")
                .filter(|s| !s.expose().trim().is_empty())
                .ok_or_else(authentication)?;
            Ok((token.expose().to_owned(), None))
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
                    .unwrap_or("https://api.anthropic.com/api/oauth/usage"),
            )
            .bearer_auth(token);
        request = request.header("anthropic-beta", "oauth-2025-04-20");
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                TrackerError::new("timeout", "Claude Code usage request timed out")
            } else {
                TrackerError::new("transport_error", "Claude Code usage request failed")
            }
        })?;
        match response.status().as_u16() {
            200..=299 => {}
            401 => {
                return Err(TrackerError::new(
                    "authentication_failed",
                    "Could not refresh Claude Code usage: the access token expired or was rejected. Sign in with `claude auth login` for this subscription, then retry `yaait usage --refresh`. For a supplied token, replace it with `yaait setup <TRACKER_ID>`.",
                ));
            }
            403 => {
                return Err(TrackerError::new(
                    "authentication_failed",
                    "Could not refresh Claude Code usage: access was denied. Check the credential for this subscription.",
                ));
            }
            429 => {
                return Err(TrackerError::new(
                    "rate_limited",
                    "Claude Code usage endpoint is rate limited",
                ));
            }
            status => return Err(invalid().detail("http_status", u64::from(status))),
        }
        let payload: Value = response.json().await.map_err(|_| invalid())?;
        let mut report = parse_report(payload)?;
        if let Some(plan) = account {
            report
                .identity
                .get_or_insert_with(Identity::default)
                .plan
                .get_or_insert_with(|| plan.into());
        }
        crate::validate_report(&report)?;
        Ok(report)
    }
}

#[async_trait]
impl TrackerProvider for ClaudeCodeProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        let mut descriptor = ProviderDescriptor {
            id: "claude-code".parse().expect("static provider ID"), name: "Claude Code".into(),
            description: "Subscription usage percentages and reset windows.".into(),
            setup: SetupSchema { fields: vec![
                SetupField { key: "token".into(), label: "Subscription access token".into(), description: "OAuth access token, not an API key. Supply either this or a credential file.".into(), kind: SetupFieldKind::Secret, required: false, allowed_values: None },
                SetupField { key: "credential_file".into(), label: "Credential file".into(), description: "Absolute path to this subscription's Claude Code .credentials.json. Read again on each fresh fetch; the upstream CLI manages token refresh.".into(), kind: SetupFieldKind::Path, required: false, allowed_values: None },

            ] },
            metrics: [("five-hour", "Five-hour window", MetricTier::Primary), ("seven-day", "Weekly window", MetricTier::Primary), ("seven-day-sonnet", "Weekly Sonnet", MetricTier::Detail), ("seven-day-opus", "Weekly Opus", MetricTier::Detail), ("seven-day-oauth-apps", "Weekly OAuth apps", MetricTier::Detail)].into_iter().map(|(id,label,tier)| MetricDescriptor { id: id.into(), label: label.into(), description: "Percentage of the subscription window used.".into(), kind: MetricKind::Quota, tier, unit: "percent".into() }).collect(),
        };
        descriptor.metrics.push(MetricDescriptor { id: "extra-usage".into(), label: "Extra usage".into(), description: "Monthly extra usage budget. USD cents are converted to dollars when the response identifies USD; otherwise values retain their credit units.".into(), kind: MetricKind::Quota, tier: MetricTier::Primary, unit: "credit".into() });
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
        || !["five_hour", "seven_day", "extra_usage"]
            .iter()
            .any(|key| payload.get(key).is_some())
    {
        return Err(invalid());
    }
    let mut metrics = vec![];
    for (id, label, tier) in [
        ("five-hour", "Five-hour window", MetricTier::Primary),
        ("seven-day", "Weekly window", MetricTier::Primary),
        ("seven-day-sonnet", "Weekly Sonnet", MetricTier::Detail),
        ("seven-day-opus", "Weekly Opus", MetricTier::Detail),
        (
            "seven-day-oauth-apps",
            "Weekly OAuth apps",
            MetricTier::Detail,
        ),
    ] {
        let source = id.replace('-', "_");
        let Some(window) = payload.get(&source).filter(|v| !v.is_null()) else {
            continue;
        };
        let used = window
            .get("utilization")
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?;
        let reset = window
            .get("resets_at")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                    .ok_or_else(invalid)
            })
            .transpose()?;
        metrics.push(metric(
            id,
            label,
            tier,
            used,
            Some(
                if id == "five-hour" {
                    "18000s"
                } else {
                    "604800s"
                }
                .into(),
            ),
            reset,
        )?);
    }
    let mut attributes = Map::new();
    if let Some(extra) = payload.get("extra_usage").filter(|v| !v.is_null()) {
        let object = extra.as_object().ok_or_else(invalid)?;
        let enabled = object
            .get("is_enabled")
            .filter(|v| !v.is_null())
            .map(|v| v.as_bool().ok_or_else(invalid))
            .transpose()?;
        let mut metadata = Map::new();
        if let Some(enabled) = enabled {
            metadata.insert("is_enabled".into(), Value::Bool(enabled));
        }
        if !metadata.is_empty() {
            attributes.insert("extra_usage".into(), Value::Object(metadata));
        }
        if enabled == Some(true) {
            let currency = object
                .get("currency")
                .filter(|v| !v.is_null())
                .map(|v| v.as_str().ok_or_else(invalid))
                .transpose()?;
            let usd = currency.is_some_and(|c| c.eq_ignore_ascii_case("usd"));
            let amount = |key: &str| -> Result<Option<f64>, TrackerError> {
                object
                    .get(key)
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        let value = v
                            .as_f64()
                            .filter(|n| n.is_finite() && *n >= 0.0)
                            .ok_or_else(invalid)?;
                        Ok(if usd { value / 100.0 } else { value })
                    })
                    .transpose()
            };
            let used = amount("used_credits")?;
            let limit = amount("monthly_limit")?;
            if used.is_some() || limit.is_some() {
                let mut metric_attributes = Map::new();
                if let Some(currency) = currency {
                    metric_attributes.insert("currency".into(), Value::String(currency.into()));
                }
                if limit == Some(0.0) {
                    metric_attributes.insert("monthly_limit".into(), Value::from(0));
                }
                metrics.push(UsageMetric {
                    id: "extra-usage".into(),
                    label: "Extra usage".into(),
                    kind: MetricKind::Quota,
                    tier: MetricTier::Primary,
                    unit: if usd { "usd" } else { "credit" }.into(),
                    used,
                    remaining: limit.zip(used).map(|(l, u)| (l - u).max(0.0)),
                    limit: limit.filter(|l| *l > 0.0),
                    value: None,
                    period: Some("month".into()),
                    resets_at: None,
                    attributes: metric_attributes,
                });
            }
        }
    }
    let plan = payload
        .get("plan")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let account = payload
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let identity = if plan.is_some() || account.is_some() {
        Some(Identity {
            plan,
            account,
            organization: None,
        })
    } else {
        None
    };
    Ok(UsageReport {
        observed_at: Utc::now(),
        identity,
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
        serde_json::json!({"five_hour":{"utilization":25,"resets_at":"2027-01-15T08:00:00Z"},"seven_day":{"utilization":70,"resets_at":"2027-01-20T08:00:00Z"},"seven_day_sonnet":{"utilization":5,"resets_at":null},"seven_day_opus":null,"extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":1234,"currency":"usd"}})
    }
    fn context(id: &str, token: &str, cache_dir: std::path::PathBuf) -> TrackerContext {
        let now = Utc::now();
        let mut credentials = SecretMap::new();
        credentials.insert("token".into(), Secret::new(token));
        TrackerContext {
            tracker: crate::TrackerManifest {
                schema_version: 1,
                id: id.parse().unwrap(),
                provider: "claude-code".parse().unwrap(),
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
        assert_eq!(report.metrics[0].used, Some(25.0));
        assert_eq!(report.metrics[0].remaining, Some(75.0));
        assert_eq!(report.metrics[0].unit, "percent");
        assert!(report.metrics[0].resets_at.is_some());
        assert_eq!(report.metrics[1].remaining, Some(30.0));
        assert_eq!(report.metrics[2].tier, MetricTier::Detail);
        assert_eq!(report.metrics[2].resets_at, None);
    }
    #[test]
    fn credential_discovery_respects_each_cli_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            ClaudeCodeProvider::credential_path(temp.path(), None).unwrap(),
            temp.path().join(".claude/.credentials.json")
        );
        let other = temp.path().join("work-subscription");
        assert_eq!(
            ClaudeCodeProvider::credential_path(temp.path(), Some(&other)).unwrap(),
            other.join(".credentials.json")
        );
    }

    #[test]
    fn utilization_is_percentage_even_below_one() {
        for percent in [0.0, 0.5, 1.0, 100.0] {
            let report =
                parse_report(serde_json::json!({"five_hour":{"utilization":percent}})).unwrap();
            assert_eq!(report.metrics[0].used, Some(percent));
            assert_eq!(report.metrics[0].remaining, Some(100.0 - percent));
        }
    }

    #[test]
    fn extra_usage_preserves_missing_limits_usage_and_currency() {
        let report = parse_report(fixture()).unwrap();
        let extra = report
            .metrics
            .iter()
            .find(|m| m.id == "extra-usage")
            .unwrap();
        assert_eq!(extra.unit, "usd");
        assert_eq!(extra.used, Some(12.34));
        assert_eq!(extra.limit, Some(50.0));
        assert!((extra.remaining.unwrap() - 37.66).abs() < 0.0001);
        crate::validate_report(&report).unwrap();
        for (extra, expected_used, expected_limit) in [
            (
                serde_json::json!({"is_enabled":true,"used_credits":1200,"currency":"usd"}),
                Some(12.0),
                None,
            ),
            (
                serde_json::json!({"is_enabled":true,"monthly_limit":5000,"currency":"usd"}),
                None,
                Some(50.0),
            ),
            (
                serde_json::json!({"is_enabled":true,"used_credits":1200,"monthly_limit":5000}),
                Some(1200.0),
                Some(5000.0),
            ),
        ] {
            let report = parse_report(serde_json::json!({"extra_usage":extra})).unwrap();
            let metric = &report.metrics[0];
            assert_eq!(metric.used, expected_used);
            assert_eq!(metric.limit, expected_limit);
            if expected_used.is_none() || expected_limit.is_none() {
                assert_eq!(metric.remaining, None);
            }
            if metric.unit == "credit" {
                assert!(metric.attributes.get("currency").is_none());
            }
            crate::validate_report(&report).unwrap();
        }
        for extra in [
            serde_json::json!({"is_enabled":true}),
            serde_json::json!({"is_enabled":false,"monthly_limit":5000}),
            serde_json::json!({"monthly_limit":5000}),
        ] {
            assert!(
                parse_report(serde_json::json!({"extra_usage":extra}))
                    .unwrap()
                    .metrics
                    .is_empty()
            );
        }
        let report = parse_report(serde_json::json!({"extra_usage":{"is_enabled":true,"used_credits":0,"monthly_limit":0,"currency":"usd"}})).unwrap();
        assert_eq!(report.metrics[0].remaining, Some(0.0));
        assert_eq!(report.metrics[0].limit, None);
        crate::validate_report(&report).unwrap();
    }

    #[test]
    fn unavailable_windows_remain_unknown() {
        let report = parse_report(serde_json::json!({"five_hour":null,"seven_day":null})).unwrap();
        assert!(report.metrics.is_empty());
        crate::validate_report(&report).unwrap();
    }

    #[test]
    fn rejects_unrelated_and_invalid_payloads_without_leaking_data() {
        for payload in [
            serde_json::json!({"secret":"sensitive"}),
            serde_json::json!([]),
            serde_json::json!({"five_hour":{"utilization":-1}}),
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
                    .header("authorization", "Bearer personal-token")
                    .header("anthropic-beta", "oauth-2025-04-20");
                then.status(200).json_body(fixture());
            })
            .await;
        let work = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/usage")
                    .header("authorization", "Bearer work-token");
                let mut payload = fixture();
                payload["five_hour"]["utilization"] = Value::from(90);
                then.status(200).json_body(payload);
            })
            .await;
        let provider = ClaudeCodeProvider {
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
        let provider = ClaudeCodeProvider {
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
                assert!(error.message.contains("claude auth login"));
            }
            assert!(!format!("{error:?}").contains("secret-token"));
            mock.delete_async().await;
        }
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
                ClaudeCodeProvider::default()
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
        app.register_provider(std::sync::Arc::new(ClaudeCodeProvider {
            endpoint_override: Some(server.url("/usage")),
        }));
        for (id, token) in [("personal", "good"), ("work", "other")] {
            app.add(crate::AddRequest {
                id: id.parse().unwrap(),
                provider: "claude-code".parse().unwrap(),
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
            let credit = metrics.iter().find(|m| m["id"] == "extra-usage").unwrap();
            assert_eq!(credit["used"], 12.34);
            assert_eq!(credit["limit"], 50.0);
            assert_eq!(credit["unit"], "usd");
            let human = crate::presentation::human::render(&envelope);
            assert!(human.stdout.contains("personal:"));
            assert!(human.stdout.contains("work:"));
            assert!(human.stdout.contains("25 of 100"));
            assert_eq!(human.stdout.contains("Weekly Sonnet"), details);
            assert!(human.stderr.is_empty());
            assert!(human.stdout.contains("Extra usage"));
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
                .contains("Could not refresh Claude Code usage")
        );
        let human = crate::presentation::human::render(&envelope);
        assert!(human.stderr.contains("expired or was rejected"));
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
            let value =
                serde_json::json!({"claudeAiOauth":{"accessToken":token,"subscriptionType":"max"}});
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                ClaudeCodeProvider::credentials(&settings, &SecretMap::new())
                    .unwrap()
                    .0,
                token
            );
        }
        std::fs::write(file, "invalid sensitive contents").unwrap();
        let error = ClaudeCodeProvider::credentials(&settings, &SecretMap::new()).unwrap_err();
        assert!(!format!("{error:?}").contains("sensitive"));
    }
}
