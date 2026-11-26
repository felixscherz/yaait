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
pub struct KimiProvider {
    endpoint_override: Option<String>,
    #[cfg(test)]
    http_override: Option<reqwest::Client>,
}

#[derive(Clone, Copy)]
enum Region {
    International,
    China,
}

impl Region {
    fn from_settings(settings: &Map<String, Value>) -> Result<Self, TrackerError> {
        match settings.get("region") {
            None => Ok(Self::International),
            Some(value) if value.as_str() == Some("international") => Ok(Self::International),
            Some(value) if value.as_str() == Some("china") => Ok(Self::China),
            _ => Err(TrackerError::invalid(
                "region must be international or china",
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::International => "international",
            Self::China => "china",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::International => "https://api.moonshot.ai/v1/users/me/balance",
            Self::China => "https://api.moonshot.cn/v1/users/me/balance",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::International => "usd",
            Self::China => "cny",
        }
    }
}

impl KimiProvider {
    async fn fetch(
        &self,
        http: &reqwest::Client,
        token: &str,
        region: Region,
    ) -> Result<UsageReport, TrackerError> {
        #[cfg(test)]
        let http = self.http_override.as_ref().unwrap_or(http);
        let response = http
            .get(
                self.endpoint_override
                    .as_deref()
                    .unwrap_or(region.endpoint()),
            )
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    TrackerError::new("timeout", "Kimi balance request timed out")
                } else {
                    TrackerError::new("transport_error", "Kimi balance request failed")
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
            return Err(TrackerError::new(code, "Kimi balance request failed"));
        }
        let payload: Payload = response.json().await.map_err(|_| invalid_response())?;
        let report = into_report(payload, region)?;
        crate::validate_report(&report)?;
        Ok(report)
    }
}

#[async_trait]
impl TrackerProvider for KimiProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "kimi".parse().expect("static provider ID is valid"),
            name: "Kimi (Moonshot AI)".into(),
            description: "Reports pay-as-you-go API account balances in USD or CNY; not Kimi Code subscription quotas.".into(),
            setup: SetupSchema {
                fields: vec![
                    SetupField {
                        key: "token".into(),
                        label: "Kimi / Moonshot API key".into(),
                        description: "An API key from the Kimi Open Platform, not a Kimi Code key.".into(),
                        kind: SetupFieldKind::Secret,
                        required: true,
                        allowed_values: None,
                    },
                    SetupField {
                        key: "region".into(),
                        label: "Kimi platform region".into(),
                        description: "international (default): api.moonshot.ai, USD; china: api.moonshot.cn, CNY. Keys are not interchangeable.".into(),
                        kind: SetupFieldKind::Choice,
                        required: false,
                        allowed_values: Some(vec!["international".into(), "china".into()]),
                    },
                ],
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
        let region = Region::from_settings(&input)?;
        let report = self.fetch(&ctx.http, token, region).await?;
        Ok(PreparedSetup {
            public_settings: Map::from_iter([(
                "region".into(),
                Value::String(region.as_str().into()),
            )]),
            secrets: SecretMap::from_iter([("token".into(), Secret::new(token))]),
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
        let region = Region::from_settings(&ctx.tracker.settings)?;
        let token = ctx.credentials.get("token").ok_or_else(|| {
            TrackerError::new("authentication_failed", "tracker credential is missing")
        })?;
        let report = self.fetch(&ctx.http, token.expose(), region).await?;
        cache.store(&report);
        Ok(report)
    }
}

#[derive(Deserialize)]
struct Payload {
    code: i64,
    status: bool,
    data: Balance,
}

#[derive(Deserialize)]
struct Balance {
    available_balance: f64,
    voucher_balance: Option<f64>,
    cash_balance: Option<f64>,
}

fn invalid_response() -> TrackerError {
    TrackerError::new(
        "invalid_provider_response",
        "Kimi returned invalid balance data",
    )
}

fn metric_definitions() -> [(&'static str, &'static str, MetricTier); 3] {
    [
        ("balance", "Available balance", MetricTier::Primary),
        ("voucher-balance", "Voucher balance", MetricTier::Detail),
        ("cash-balance", "Cash balance", MetricTier::Detail),
    ]
}

fn metric_descriptors() -> Vec<MetricDescriptor> {
    ["usd", "cny"]
        .into_iter()
        .flat_map(|unit| {
            metric_definitions()
                .into_iter()
                .map(move |(id, label, tier)| MetricDescriptor {
                    id: format!("{id}-{unit}"),
                    label: format!("{label} ({})", unit.to_uppercase()),
                    description: format!("Account {label} in {}.", unit.to_uppercase()),
                    kind: MetricKind::Balance,
                    tier,
                    unit: unit.into(),
                })
        })
        .collect()
}

fn into_report(payload: Payload, region: Region) -> Result<UsageReport, TrackerError> {
    if payload.code != 0
        || !payload.status
        || payload
            .data
            .voucher_balance
            .is_some_and(|value| value < 0.0)
    {
        return Err(invalid_response());
    }
    let unit = region.unit();
    let amounts = [
        Some(payload.data.available_balance),
        payload.data.voucher_balance,
        payload.data.cash_balance,
    ];
    let mut metrics = Vec::new();
    for ((id, label, tier), amount) in metric_definitions().into_iter().zip(amounts) {
        if let Some(value) = amount {
            if !value.is_finite() {
                return Err(invalid_response());
            }
            // Cash and available balances may be negative. Preserve debt as reported.
            metrics.push(UsageMetric {
                id: format!("{id}-{unit}"),
                label: format!("{label} ({})", unit.to_uppercase()),
                kind: MetricKind::Balance,
                tier,
                unit: unit.into(),
                value: Some(value),
                used: None,
                remaining: None,
                limit: None,
                period: None,
                resets_at: None,
                attributes: Map::from_iter([("scope".into(), Value::String("account".into()))]),
            });
        }
    }
    Ok(UsageReport {
        observed_at: Utc::now(),
        identity: None,
        metrics,
        attributes: Map::from_iter([
            ("region".into(), Value::String(region.as_str().into())),
            (
                "is_available".into(),
                Value::Bool(payload.data.available_balance > 0.0),
            ),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AddRequest, App, AppPaths, FileRegistry, TrackerManifest, UsageOptions,
        presentation::{Envelope, human},
    };
    use httpmock::{Method::GET, MockServer};
    use serde_json::json;
    use std::{path::Path, sync::Arc, time::Duration};

    fn body(available: f64, voucher: f64, cash: f64) -> Value {
        json!({"code": 0, "status": true, "scode": "0x0", "data": {
            "available_balance": available, "voucher_balance": voucher, "cash_balance": cash
        }})
    }

    fn provider(server: &MockServer) -> KimiProvider {
        KimiProvider {
            endpoint_override: Some(server.url("/v1/users/me/balance")),
            ..Default::default()
        }
    }

    fn context(root: &Path, id: &str, token: &str) -> TrackerContext {
        TrackerContext {
            tracker: TrackerManifest {
                schema_version: 1,
                id: id.parse().unwrap(),
                provider: "kimi".parse().unwrap(),
                name: id.into(),
                description: None,
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                settings: json!({"region": "international"})
                    .as_object()
                    .unwrap()
                    .clone(),
                extensions: Default::default(),
            },
            data_dir: root.join(id).join("data"),
            cache_dir: root.join(id).join("cache"),
            credentials: SecretMap::from_iter([("token".into(), Secret::new(token))]),
            http: reqwest::Client::new(),
        }
    }

    #[test]
    fn regions_select_fixed_endpoints_and_currency_specific_metrics() {
        assert_eq!(
            Region::International.endpoint(),
            "https://api.moonshot.ai/v1/users/me/balance"
        );
        assert_eq!(
            Region::China.endpoint(),
            "https://api.moonshot.cn/v1/users/me/balance"
        );
        for (region, unit) in [(Region::International, "usd"), (Region::China, "cny")] {
            let report = into_report(
                serde_json::from_value(body(4.0, 5.0, -1.0)).unwrap(),
                region,
            )
            .unwrap();
            crate::validate_report(&report).unwrap();
            assert_eq!(report.metrics.len(), 3);
            assert_eq!(report.metrics[0].id, format!("balance-{unit}"));
            assert_eq!(report.metrics[0].tier, MetricTier::Primary);
            assert_eq!(report.metrics[1].tier, MetricTier::Detail);
            assert_eq!(report.metrics[2].value, Some(-1.0));
            assert!(report.metrics.iter().all(|m| m.unit == unit
                && m.limit.is_none()
                && m.remaining.is_none()
                && m.period.is_none()
                && m.resets_at.is_none()));
            assert!(report.identity.is_none());
            assert_eq!(report.attributes["region"], region.as_str());
            assert_eq!(report.attributes["is_available"], true);
            for metric in report.metrics {
                assert!(
                    metric_descriptors()
                        .iter()
                        .any(|d| d.id == metric.id && d.tier == metric.tier)
                );
            }
        }
        for amount in [0.0, -1.0] {
            let report = into_report(
                serde_json::from_value(json!({"code": 0, "status": true,
                "data": {"available_balance": amount}}))
                .unwrap(),
                Region::International,
            )
            .unwrap();
            assert_eq!(report.metrics.len(), 1);
            assert_eq!(report.metrics[0].value, Some(amount));
            assert_eq!(report.attributes["is_available"], false);
        }
    }

    #[tokio::test]
    async fn validates_setup_and_keeps_credentials_out_of_public_data() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v1/users/me/balance")
                    .header("authorization", "Bearer setup-secret");
                then.status(200).json_body(body(12.0, 2.0, 10.0));
            })
            .await;
        let temp = tempfile::tempdir().unwrap();
        let ctx = SetupContext {
            tracker_id: "personal".parse().unwrap(),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            http: reqwest::Client::new(),
        };
        let provider = provider(&server);
        for (region, unit) in [(None, "usd"), (Some("china"), "cny")] {
            let mut input = json!({"token": "setup-secret"})
                .as_object()
                .unwrap()
                .clone();
            if let Some(region) = region {
                input.insert("region".into(), json!(region));
            }
            let prepared = provider.validate_setup(&ctx, input).await.unwrap();
            assert_eq!(prepared.public_settings.len(), 1);
            assert_eq!(
                prepared.public_settings["region"],
                if region.is_some() {
                    "china"
                } else {
                    "international"
                }
            );
            assert_eq!(prepared.secrets["token"].expose(), "setup-secret");
            assert!(!format!("{prepared:?}").contains("setup-secret"));
            let report = prepared.initial_report.unwrap();
            assert_eq!(report.metrics[0].unit, unit);
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("setup-secret")
            );
        }
        for input in [
            json!({}),
            json!({"token": true}),
            json!({"token": " "}),
            json!({"token": "x", "region": "invalid"}),
            json!({"token": "x", "region": null}),
            json!({"token": "x", "unexpected": true}),
        ] {
            assert!(
                provider
                    .validate_setup(&ctx, input.as_object().unwrap().clone())
                    .await
                    .is_err()
            );
        }
        assert_eq!(mock.calls_async().await, 2);
        assert!(!ctx.data_dir.exists());
        assert!(!ctx.cache_dir.exists());
    }

    #[tokio::test]
    async fn caches_accounts_independently_and_preserves_observation_times() {
        let server = MockServer::start_async().await;
        let personal = server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer personal-secret");
                then.status(200).json_body(body(12.0, 2.0, 10.0));
            })
            .await;
        let work = server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer work-secret");
                then.status(200).json_body(body(34.0, 2.0, 32.0));
            })
            .await;
        let provider = provider(&server);
        let temp = tempfile::tempdir().unwrap();
        let personal_ctx = context(temp.path(), "personal", "personal-secret");
        let work_ctx = context(temp.path(), "work", "work-secret");
        let first = provider
            .collect(&personal_ctx, CachePolicy::Cached)
            .await
            .unwrap();
        let second = provider
            .collect(&work_ctx, CachePolicy::Cached)
            .await
            .unwrap();
        assert_ne!(first.metrics, second.metrics);
        for (ctx, report) in [(&personal_ctx, &first), (&work_ctx, &second)] {
            assert_eq!(
                &provider.collect(ctx, CachePolicy::Cached).await.unwrap(),
                report
            );
        }
        assert_eq!(personal.calls_async().await, 1);
        assert_eq!(work.calls_async().await, 1);
        let refreshed = provider
            .collect(&personal_ctx, CachePolicy::Refresh)
            .await
            .unwrap();
        assert!(refreshed.observed_at >= first.observed_at);
        assert_eq!(personal.calls_async().await, 2);
        work.delete_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer work-secret");
                then.status(401).body("work-secret");
            })
            .await;
        assert_eq!(
            provider
                .collect(&work_ctx, CachePolicy::Refresh)
                .await
                .unwrap_err()
                .code,
            "authentication_failed"
        );
        assert_eq!(
            provider
                .collect(&work_ctx, CachePolicy::Cached)
                .await
                .unwrap(),
            second
        );
        let mut missing = context(temp.path(), "missing", "unused");
        missing.credentials.clear();
        assert_eq!(
            provider
                .collect(&missing, CachePolicy::Refresh)
                .await
                .unwrap_err()
                .code,
            "authentication_failed"
        );
        missing
            .tracker
            .settings
            .insert("region".into(), json!("invalid"));
        assert_eq!(
            provider
                .collect(&missing, CachePolicy::Refresh)
                .await
                .unwrap_err()
                .code,
            "invalid_input"
        );
    }

    #[tokio::test]
    async fn application_keeps_both_views_and_successful_accounts_on_partial_failure() {
        let server = MockServer::start_async().await;
        let personal = server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer personal-secret");
                then.status(200).json_body(body(12.0, 2.0, 10.0));
            })
            .await;
        let work = server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer work-secret");
                then.status(200).json_body(body(34.0, 2.0, 32.0));
            })
            .await;
        let temp = tempfile::tempdir().unwrap();
        let mut app = App::new(FileRegistry::new(AppPaths::isolated(temp.path()))).unwrap();
        app.register_provider(Arc::new(KimiProvider {
            http_override: Some(reqwest::Client::new()),
            ..provider(&server)
        }));
        for (id, token, region) in [
            ("personal", "personal-secret", "international"),
            ("work", "work-secret", "china"),
        ] {
            app.add(AddRequest {
                id: id.parse().unwrap(),
                provider: "kimi".parse().unwrap(),
                name: None,
                description: None,
                input: json!({"token": token, "region": region})
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
                    UsageOptions {
                        details,
                        ..UsageOptions::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(result.data.trackers.len(), 2);
            assert_eq!(
                result.data.trackers[0].metrics.len(),
                if details { 3 } else { 1 }
            );
            let envelope = Envelope::usage(result.data, result.warnings, result.errors);
            assert_eq!(envelope.exit_code(), 0);
            let encoded = serde_json::to_string(&envelope).unwrap();
            let decoded: Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded["schema_version"], 2);
            assert_eq!(decoded["data"]["trackers"][0]["id"], "personal");
            assert_eq!(
                decoded["data"]["trackers"][0]["metrics"][0]["id"],
                "balance-usd"
            );
            assert_eq!(decoded["data"]["trackers"][1]["id"], "work");
            assert_eq!(
                decoded["data"]["trackers"][1]["metrics"][0]["id"],
                "balance-cny"
            );
            assert!(!encoded.contains("secret"));
            let rendered = human::render(&envelope);
            assert!(rendered.stderr.is_empty());
            assert!(rendered.stdout.contains("personal:"));
            assert!(rendered.stdout.contains("work:"));
            assert!(rendered.stdout.contains("$12.00"));
            assert!(rendered.stdout.contains("34 CNY"));
            assert_eq!(rendered.stdout.contains("Voucher balance"), details);
            assert_eq!(rendered.stdout.contains("Cash balance"), details);
        }
        // Setup seeds each tracker cache; neither default report refetches data.
        assert_eq!(personal.calls_async().await, 1);
        assert_eq!(work.calls_async().await, 1);
        work.delete_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .header("authorization", "Bearer work-secret");
                then.status(401).body("work-secret");
            })
            .await;
        let result = app
            .usage(
                &[],
                UsageOptions {
                    details: true,
                    cache: CachePolicy::Refresh,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.data.trackers.len(), 1);
        assert_eq!(result.data.trackers[0].id.as_str(), "personal");
        let envelope = Envelope::usage(result.data, result.warnings, result.errors);
        assert_eq!(envelope.exit_code(), 2);
        assert_eq!(envelope.errors[0].tracker_id.as_deref(), Some("work"));
        assert_eq!(envelope.errors[0].provider.as_deref(), Some("kimi"));
        assert!(human::render(&envelope).stderr.contains("work"));
        assert!(!serde_json::to_string(&envelope).unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn rejects_http_errors_and_malformed_payloads_without_leaking_secrets() {
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
            let error = provider(&server)
                .fetch(
                    &reqwest::Client::new(),
                    "secret-token",
                    Region::International,
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, code);
            assert!(!format!("{error:?}").contains("sensitive-body"));
            assert!(!format!("{error:?}").contains("secret-token"));
        }
        let mut invalid = vec![
            json!({}),
            json!({"code": 0, "status": true, "data": {}}),
            json!({"code": 0, "status": true, "data": {"available_balance": null}}),
            json!({"code": 0, "status": true, "data": {"available_balance": "1"}}),
            body(1.0, -1.0, 2.0),
        ];
        let mut failed_code = body(1.0, 0.0, 1.0);
        failed_code["code"] = json!(1);
        invalid.push(failed_code);
        let mut failed_status = body(1.0, 0.0, 1.0);
        failed_status["status"] = json!(false);
        invalid.push(failed_status);
        for value in invalid {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(200).json_body(value);
                })
                .await;
            assert_eq!(
                provider(&server)
                    .fetch(
                        &reqwest::Client::new(),
                        "secret-token",
                        Region::International
                    )
                    .await
                    .unwrap_err()
                    .code,
                "invalid_provider_response"
            );
        }
        for value in [f64::NAN, f64::INFINITY] {
            for index in 0..3 {
                let mut payload: Payload = serde_json::from_value(body(1.0, 0.0, 1.0)).unwrap();
                match index {
                    0 => payload.data.available_balance = value,
                    1 => payload.data.voucher_balance = Some(value),
                    _ => payload.data.cash_balance = Some(value),
                }
                assert!(into_report(payload, Region::International).is_err());
            }
        }
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200).body("sensitive-invalid-json");
            })
            .await;
        let error = provider(&server)
            .fetch(
                &reqwest::Client::new(),
                "secret-token",
                Region::International,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_provider_response");
        assert!(!format!("{error:?}").contains("sensitive-invalid-json"));
    }

    #[tokio::test]
    async fn classifies_timeouts_and_transport_errors() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(200)
                    .delay(Duration::from_millis(100))
                    .json_body(body(1.0, 0.0, 1.0));
            })
            .await;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(10))
            .build()
            .unwrap();
        assert_eq!(
            provider(&server)
                .fetch(&http, "secret", Region::International)
                .await
                .unwrap_err()
                .code,
            "timeout"
        );
        let provider = KimiProvider {
            endpoint_override: Some("http://127.0.0.1:0/balance".into()),
            ..Default::default()
        };
        assert_eq!(
            provider
                .fetch(&http, "secret", Region::International)
                .await
                .unwrap_err()
                .code,
            "transport_error"
        );
    }
}
