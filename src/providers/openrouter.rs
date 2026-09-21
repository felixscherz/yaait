use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    CachePolicy, MetricDescriptor, MetricKind, MetricTier, PreparedSetup, ProviderDescriptor,
    ReportCache, Secret, SecretMap, SetupContext, SetupField, SetupFieldKind, SetupInput,
    SetupSchema, TrackerContext, TrackerError, TrackerProvider, UsageMetric, UsageReport,
};

#[derive(Default)]
pub struct OpenRouterProvider {
    endpoint_override: Option<String>,
}

impl OpenRouterProvider {
    async fn fetch(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<UsageReport, TrackerError> {
        let response = http
            .get(
                self.endpoint_override
                    .as_deref()
                    .unwrap_or("https://openrouter.ai/api/v1/key"),
            )
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    TrackerError::new("timeout", "OpenRouter request timed out")
                } else {
                    TrackerError::new("transport_error", "OpenRouter request failed")
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            let code = match status.as_u16() {
                401 => "authentication_failed",
                403 => "authorization_failed",
                429 => "rate_limited",
                _ => "invalid_provider_response",
            };
            return Err(TrackerError::new(code, "OpenRouter usage request failed"));
        }
        let payload: Payload = response.json().await.map_err(|_| invalid_response())?;
        let report = into_report(payload)?;
        crate::validate_report(&report)?;
        Ok(report)
    }
}

#[async_trait]
impl TrackerProvider for OpenRouterProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "openrouter".parse().expect("static provider ID is valid"),
            name: "OpenRouter".into(),
            description: "Reports API key spending and remaining key budget in USD.".into(),
            setup: SetupSchema {
                fields: vec![SetupField {
                    key: "token".into(),
                    label: "OpenRouter API key".into(),
                    description: "An API key for the account or subscription to track.".into(),
                    kind: SetupFieldKind::Secret,
                    required: true,
                    allowed_values: None,
                }],
            },
            metrics: metric_descriptors(),
        }
    }

    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError> {
        crate::validate_setup_input(&self.descriptor().setup, &input)?;
        let token = input
            .get("token")
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| TrackerError::invalid("token must be a nonempty string"))?;
        let report = self.fetch(&ctx.http, token).await?;
        let mut secrets = SecretMap::new();
        secrets.insert("token".into(), Secret::new(token));
        Ok(PreparedSetup {
            public_settings: Map::new(),
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
        let token = ctx.credentials.get("token").ok_or_else(|| {
            TrackerError::new("authentication_failed", "tracker credential is missing")
        })?;
        let report = self.fetch(&ctx.http, token.expose()).await?;
        cache.store(&report);
        Ok(report)
    }
}

fn invalid_response() -> TrackerError {
    TrackerError::new(
        "invalid_provider_response",
        "OpenRouter returned invalid usage data",
    )
}

fn metric(id: &str, label: &str, unit: &str, kind: MetricKind, tier: MetricTier) -> UsageMetric {
    UsageMetric {
        id: id.into(),
        label: label.into(),
        unit: unit.into(),
        kind,
        tier,
        used: None,
        remaining: None,
        limit: None,
        value: None,
        period: None,
        resets_at: None,
        attributes: Map::from_iter([("scope".into(), Value::String("key".into()))]),
    }
}

#[derive(Deserialize)]
struct Payload {
    data: KeyInfo,
}

#[derive(Deserialize)]
struct KeyInfo {
    usage: f64,
    limit: Option<f64>,
    limit_remaining: Option<f64>,
    limit_reset: Option<String>,
    usage_daily: Option<f64>,
    usage_weekly: Option<f64>,
    usage_monthly: Option<f64>,
    byok_usage: Option<f64>,
    include_byok_in_limit: Option<bool>,
    is_free_tier: Option<bool>,
    creator_user_id: Option<String>,
    organization_id: Option<String>,
}

fn metric_descriptors() -> Vec<MetricDescriptor> {
    [
        (
            "key-budget",
            "Key budget",
            MetricKind::Quota,
            MetricTier::Primary,
        ),
        (
            "spend",
            "Total spend",
            MetricKind::Counter,
            MetricTier::Primary,
        ),
        (
            "spend-daily",
            "Daily spend",
            MetricKind::Counter,
            MetricTier::Detail,
        ),
        (
            "spend-weekly",
            "Weekly spend",
            MetricKind::Counter,
            MetricTier::Detail,
        ),
        (
            "spend-monthly",
            "Monthly spend",
            MetricKind::Counter,
            MetricTier::Detail,
        ),
        (
            "byok-spend",
            "External BYOK spend",
            MetricKind::Counter,
            MetricTier::Detail,
        ),
    ]
    .into_iter()
    .map(|(id, label, kind, tier)| MetricDescriptor {
        id: id.into(),
        label: label.into(),
        kind,
        tier,
        unit: "usd".into(),
        description: format!("{label} for this API key in USD."),
    })
    .collect()
}

fn into_report(payload: Payload) -> Result<UsageReport, TrackerError> {
    let data = payload.data;
    if [
        Some(data.usage),
        data.limit,
        data.usage_daily,
        data.usage_weekly,
        data.usage_monthly,
        data.byok_usage,
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.is_finite() || value < 0.0)
        || data.limit_remaining.is_some_and(|value| !value.is_finite())
    {
        return Err(invalid_response());
    }
    let mut metrics = Vec::new();
    // Lifetime usage does not necessarily cover the key's current budget period,
    // and BYOK may count toward the limit. Use the authoritative remaining value.
    if data.limit.is_some() || data.limit_remaining.is_some() {
        let mut budget = metric(
            "key-budget",
            "Key budget",
            "usd",
            MetricKind::Quota,
            MetricTier::Primary,
        );
        budget.limit = data.limit.filter(|limit| *limit > 0.0);
        budget.remaining = data.limit_remaining.map(|value| value.max(0.0));
        if data.limit == Some(0.0) {
            budget.remaining = Some(0.0);
            budget
                .attributes
                .insert("configured_limit".into(), Value::from(0));
        }
        budget.period = data.limit_reset;
        if let Some(include) = data.include_byok_in_limit {
            budget
                .attributes
                .insert("include_byok_in_limit".into(), Value::Bool(include));
        }
        metrics.push(budget);
    }
    for (id, label, value, period) in [
        ("spend", "Total spend", Some(data.usage), "lifetime"),
        ("spend-daily", "Daily spend", data.usage_daily, "utc-day"),
        (
            "spend-weekly",
            "Weekly spend",
            data.usage_weekly,
            "utc-week",
        ),
        (
            "spend-monthly",
            "Monthly spend",
            data.usage_monthly,
            "utc-month",
        ),
        (
            "byok-spend",
            "External BYOK spend",
            data.byok_usage,
            "lifetime",
        ),
    ] {
        if let Some(value) = value {
            let mut spend = metric(id, label, "usd", MetricKind::Counter, MetricTier::Detail);
            if id == "spend" {
                spend.tier = MetricTier::Primary;
            }
            spend.value = Some(value);
            spend.period = Some(period.into());
            metrics.push(spend);
        }
    }
    let account = data.creator_user_id.filter(|s| !s.is_empty());
    let organization = data.organization_id.filter(|s| !s.is_empty());
    let identity = if account.is_some() || organization.is_some() {
        Some(crate::Identity {
            account,
            organization,
            plan: None,
        })
    } else {
        None
    };
    let mut attributes = Map::new();
    if let Some(free) = data.is_free_tier {
        attributes.insert("is_free_tier".into(), Value::Bool(free));
    }
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
    use crate::{
        TrackerManifest, TrackerReport,
        presentation::{Envelope, human},
    };
    use httpmock::{Method::GET, MockServer};
    use serde_json::json;
    use std::{collections::BTreeMap, path::PathBuf};

    fn body(amount: f64) -> Value {
        json!({"data":{"usage":99.0,"limit":100.0,"limit_remaining":amount,"limit_reset":"monthly","usage_daily":1.0,"byok_usage":4.0,"include_byok_in_limit":true,"label":"secret-key-prefix"}})
    }

    fn context(root: &std::path::Path, id: &str, token: &str) -> TrackerContext {
        TrackerContext {
            tracker: TrackerManifest {
                schema_version: 1,
                id: id.parse().unwrap(),
                provider: "openrouter".parse().unwrap(),
                name: id.into(),
                description: None,
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                settings: Map::new(),
                extensions: BTreeMap::new(),
            },
            data_dir: root.join(id).join("data"),
            cache_dir: root.join(id).join("cache"),
            credentials: SecretMap::from_iter([("token".into(), Secret::new(token))]),
            http: reqwest::Client::new(),
        }
    }

    #[tokio::test]
    async fn validates_setup_without_persisting_or_displaying_secrets() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/key")
                    .header("authorization", "Bearer test-secret");
                then.status(200).json_body(body(12.0));
            })
            .await;
        let provider = OpenRouterProvider {
            endpoint_override: Some(format!("{}/api/v1/key", server.base_url())),
        };
        let ctx = SetupContext {
            tracker_id: "personal".parse().unwrap(),
            data_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            http: reqwest::Client::new(),
        };
        let prepared = provider
            .validate_setup(
                &ctx,
                json!({"token":"test-secret"}).as_object().unwrap().clone(),
            )
            .await
            .unwrap();
        assert!(prepared.public_settings.is_empty());
        assert_eq!(prepared.secrets["token"].expose(), "test-secret");
        assert!(!format!("{prepared:?}").contains("test-secret"));
        crate::validate_report(prepared.initial_report.as_ref().unwrap()).unwrap();
        for input in [
            json!({}),
            json!({"token":true}),
            json!({"token":" "}),
            json!({"token":"x","unexpected":true}),
        ] {
            assert!(
                provider
                    .validate_setup(&ctx, input.as_object().unwrap().clone())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn isolates_credentials_and_cache_and_preserves_fetch_time() {
        let server = MockServer::start_async().await;
        let personal_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/key")
                    .header("authorization", "Bearer personal-secret");
                then.status(200).json_body(body(12.0));
            })
            .await;
        let work_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/key")
                    .header("authorization", "Bearer work-secret");
                then.status(200).json_body(body(34.0));
            })
            .await;
        let provider = OpenRouterProvider {
            endpoint_override: Some(format!("{}/api/v1/key", server.base_url())),
        };
        let temp = tempfile::tempdir().unwrap();
        let personal = context(temp.path(), "personal", "personal-secret");
        let work = context(temp.path(), "work", "work-secret");
        let first = provider
            .collect(&personal, CachePolicy::Cached)
            .await
            .unwrap();
        let second = provider.collect(&work, CachePolicy::Cached).await.unwrap();
        assert_ne!(first.metrics, second.metrics);
        assert_eq!(
            provider
                .collect(&personal, CachePolicy::Cached)
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            provider.collect(&work, CachePolicy::Cached).await.unwrap(),
            second
        );
        assert_eq!(personal_mock.calls_async().await, 1);
        assert_eq!(work_mock.calls_async().await, 1);
        provider
            .collect(&personal, CachePolicy::Refresh)
            .await
            .unwrap();
        assert_eq!(personal_mock.calls_async().await, 2);
        work_mock.delete_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/key")
                    .header("authorization", "Bearer work-secret");
                then.status(401).body("work-secret");
            })
            .await;
        let error = provider
            .collect(&work, CachePolicy::Refresh)
            .await
            .unwrap_err();
        assert_eq!(error.code, "authentication_failed");
        assert_eq!(
            provider.collect(&work, CachePolicy::Cached).await.unwrap(),
            second
        );
        for details in [false, true] {
            let reports: Vec<_> = [(&personal, first.clone()), (&work, second.clone())]
                .into_iter()
                .map(|(ctx, mut report)| {
                    if !details {
                        report.metrics.retain(|m| m.tier == MetricTier::Primary);
                    }
                    TrackerReport::from_report(&ctx.tracker, report)
                })
                .collect();
            assert!(reports.iter().all(|r| !r.metrics.is_empty()));
            let envelope = Envelope::usage(json!({"trackers":reports}), vec![], vec![]);
            let encoded = serde_json::to_string(&envelope).unwrap();
            let decoded: Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded["schema_version"], 2);
            assert_eq!(decoded["data"]["trackers"][0]["id"], "personal");
            assert_eq!(decoded["data"]["trackers"][1]["id"], "work");
            assert!(!encoded.contains("secret"));
            let rendered = human::render(&envelope);
            assert!(rendered.stderr.is_empty());
            assert!(rendered.stdout.contains("personal:"));
            assert!(rendered.stdout.contains("work:"));
            assert!(rendered.stdout.contains("used: $88.00 of $100.00"));
            assert_eq!(
                decoded["data"]["trackers"][0]["metrics"][0]["remaining"],
                12.0
            );
        }
        let envelope = Envelope::usage(
            json!({"trackers":[TrackerReport::from_report(&personal.tracker, first)]}),
            vec![],
            vec![error.tracker("work").provider("openrouter")],
        );
        assert_eq!(envelope.exit_code(), 2);
        assert!(human::render(&envelope).stderr.contains("work"));
    }

    #[tokio::test]
    async fn classifies_http_errors_without_response_bodies() {
        for (status, code) in [
            (401, "authentication_failed"),
            (403, "authorization_failed"),
            (429, "rate_limited"),
            (500, "invalid_provider_response"),
        ] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(status).body("sensitive-body");
                })
                .await;
            let provider = OpenRouterProvider {
                endpoint_override: Some(server.url("/api/v1/key")),
            };
            let error = provider
                .fetch(&reqwest::Client::new(), "secret-token")
                .await
                .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!format!("{error:?}").contains("sensitive-body"));
            assert!(!format!("{error:?}").contains("secret-token"));
        }
    }

    #[tokio::test]
    async fn rejects_malformed_success_payloads() {
        for body in [json!({}), json!({"data":{}}), json!({"balance_infos":[]})] {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(200).json_body(body);
                })
                .await;
            let provider = OpenRouterProvider {
                endpoint_override: Some(server.url("/api/v1/key")),
            };
            assert_eq!(
                provider
                    .fetch(&reqwest::Client::new(), "secret")
                    .await
                    .unwrap_err()
                    .code,
                "invalid_provider_response"
            );
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use serde_json::json;

    fn report(data: Value) -> UsageReport {
        let report = into_report(serde_json::from_value(json!({"data":data})).unwrap()).unwrap();
        crate::validate_report(&report).unwrap();
        report
    }

    #[test]
    fn lifetime_spend_is_not_used_as_current_budget_spend() {
        let report = report(
            json!({"usage":900,"limit":100,"limit_remaining":12,"limit_reset":"monthly","include_byok_in_limit":true}),
        );
        let budget = &report.metrics[0];
        assert_eq!(budget.remaining, Some(12.0));
        assert_eq!(budget.used, None);
        assert_eq!(budget.limit, Some(100.0));
        assert_eq!(budget.period.as_deref(), Some("monthly"));
        assert_eq!(budget.resets_at, None);
        assert_eq!(budget.attributes["include_byok_in_limit"], true);
    }

    #[test]
    fn unknown_and_zero_limits_remain_distinct() {
        let unknown = report(json!({"usage":3,"limit":null,"limit_remaining":null}));
        assert!(unknown.metrics.iter().all(|m| m.id != "key-budget"));
        assert_eq!(unknown.metrics[0].tier, MetricTier::Primary);
        let zero = report(json!({"usage":3,"limit":0,"limit_remaining":null}));
        assert_eq!(zero.metrics[0].remaining, Some(0.0));
        assert_eq!(zero.metrics[0].limit, None);
        assert_eq!(zero.metrics[0].attributes["configured_limit"], 0);
        let missing_remaining = report(json!({"usage":3,"limit":10}));
        assert_eq!(missing_remaining.metrics[0].remaining, None);
        let overspent = report(json!({"usage":3,"limit":1,"limit_remaining":-2}));
        assert_eq!(overspent.metrics[0].remaining, Some(0.0));
    }

    #[test]
    fn rejects_negative_spend_and_limits() {
        for data in [
            json!({"usage":-1}),
            json!({"usage":1,"limit":-2}),
            json!({"usage":1,"usage_daily":-3}),
        ] {
            assert!(into_report(serde_json::from_value(json!({"data":data})).unwrap()).is_err());
        }
    }
}
