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
pub struct DeepSeekProvider {
    endpoint_override: Option<String>,
}

impl DeepSeekProvider {
    async fn fetch(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<UsageReport, TrackerError> {
        let response = http
            .get(
                self.endpoint_override
                    .as_deref()
                    .unwrap_or("https://api.deepseek.com/user/balance"),
            )
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    TrackerError::new("timeout", "DeepSeek request timed out")
                } else {
                    TrackerError::new("transport_error", "DeepSeek request failed")
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
            return Err(TrackerError::new(code, "DeepSeek usage request failed"));
        }
        let payload: Payload = response.json().await.map_err(|_| invalid_response())?;
        let report = into_report(payload)?;
        crate::validate_report(&report)?;
        Ok(report)
    }
}

#[async_trait]
impl TrackerProvider for DeepSeekProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "deepseek".parse().expect("static provider ID is valid"),
            name: "DeepSeek".into(),
            description: "Reports API account balances separately by currency.".into(),
            setup: SetupSchema {
                fields: vec![SetupField {
                    key: "token".into(),
                    label: "DeepSeek API key".into(),
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
        "DeepSeek returned invalid usage data",
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
        attributes: Map::from_iter([("scope".into(), Value::String("account".into()))]),
    }
}

#[derive(Deserialize)]
struct Payload {
    is_available: bool,
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    granted_balance: Option<String>,
    topped_up_balance: Option<String>,
}

fn metric_descriptors() -> Vec<MetricDescriptor> {
    ["usd", "cny"]
        .into_iter()
        .flat_map(|unit| {
            [
                ("balance", "Available balance", MetricTier::Primary),
                ("granted-balance", "Granted balance", MetricTier::Detail),
                ("topped-up-balance", "Topped-up balance", MetricTier::Detail),
            ]
            .into_iter()
            .map(move |(id, label, tier)| MetricDescriptor {
                id: format!("{id}-{unit}"),
                label: format!("{label} ({})", unit.to_uppercase()),
                kind: MetricKind::Balance,
                tier,
                unit: unit.into(),
                description: format!("Account {label} in {}.", unit.to_uppercase()),
            })
        })
        .collect()
}

fn into_report(payload: Payload) -> Result<UsageReport, TrackerError> {
    if payload.balance_infos.is_empty() {
        return Err(invalid_response());
    }
    let mut metrics = Vec::new();
    for balance in payload.balance_infos {
        let unit = match balance.currency.as_str() {
            "USD" => "usd",
            "CNY" => "cny",
            _ => return Err(invalid_response()),
        };
        for (id, label, amount, tier) in [
            (
                "balance",
                "Available balance",
                Some(balance.total_balance),
                MetricTier::Primary,
            ),
            (
                "granted-balance",
                "Granted balance",
                balance.granted_balance,
                MetricTier::Detail,
            ),
            (
                "topped-up-balance",
                "Topped-up balance",
                balance.topped_up_balance,
                MetricTier::Detail,
            ),
        ] {
            if let Some(amount) = amount {
                let value: f64 = amount.parse().map_err(|_| invalid_response())?;
                if !value.is_finite() {
                    return Err(invalid_response());
                }
                // Preserve negative account balances as debt rather than hiding them.
                let mut item = metric(
                    &format!("{id}-{unit}"),
                    &format!("{label} ({})", balance.currency),
                    unit,
                    MetricKind::Balance,
                    tier,
                );
                item.value = Some(value);
                metrics.push(item);
            }
        }
    }
    Ok(UsageReport {
        observed_at: Utc::now(),
        identity: None,
        metrics,
        attributes: Map::from_iter([("is_available".into(), Value::Bool(payload.is_available))]),
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
        json!({"is_available":true,"balance_infos":[{"currency":"USD","total_balance":amount.to_string(),"granted_balance":"2.00","topped_up_balance":(amount-2.0).to_string()}]})
    }

    fn context(root: &std::path::Path, id: &str, token: &str) -> TrackerContext {
        TrackerContext {
            tracker: TrackerManifest {
                schema_version: 1,
                id: id.parse().unwrap(),
                provider: "deepseek".parse().unwrap(),
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
                    .path("/user/balance")
                    .header("authorization", "Bearer test-secret");
                then.status(200).json_body(body(12.0));
            })
            .await;
        let provider = DeepSeekProvider {
            endpoint_override: Some(format!("{}/user/balance", server.base_url())),
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
                    .path("/user/balance")
                    .header("authorization", "Bearer personal-secret");
                then.status(200).json_body(body(12.0));
            })
            .await;
        let work_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/user/balance")
                    .header("authorization", "Bearer work-secret");
                then.status(200).json_body(body(34.0));
            })
            .await;
        let provider = DeepSeekProvider {
            endpoint_override: Some(format!("{}/user/balance", server.base_url())),
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
                    .path("/user/balance")
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
            assert!(rendered.stdout.contains("12"));
        }
        let envelope = Envelope::usage(
            json!({"trackers":[TrackerReport::from_report(&personal.tracker, first)]}),
            vec![],
            vec![error.tracker("work").provider("deepseek")],
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
            let provider = DeepSeekProvider {
                endpoint_override: Some(server.url("/user/balance")),
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
            let provider = DeepSeekProvider {
                endpoint_override: Some(server.url("/user/balance")),
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
mod balance_tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<UsageReport, TrackerError> {
        let report = into_report(serde_json::from_value(value).map_err(|_| invalid_response())?)?;
        crate::validate_report(&report)?;
        Ok(report)
    }

    #[test]
    fn keeps_currencies_separate_and_optional_amounts_unknown() {
        let report = parse(json!({"is_available":false,"balance_infos":[
            {"currency":"USD","total_balance":"0.00"},
            {"currency":"CNY","total_balance":"-1.25","granted_balance":"0"}
        ]}))
        .unwrap();
        assert_eq!(report.metrics.len(), 3);
        assert_eq!(report.metrics[0].id, "balance-usd");
        assert_eq!(report.metrics[0].value, Some(0.0));
        assert_eq!(report.metrics[1].id, "balance-cny");
        assert_eq!(report.metrics[1].value, Some(-1.25));
        assert_eq!(report.attributes["is_available"], false);
        assert!(report.identity.is_none());
        let envelope = crate::presentation::Envelope::usage(
            json!({"trackers":[{"id":"work","metrics":report.metrics}]}),
            vec![],
            vec![],
        );
        let rendered = crate::presentation::human::render(&envelope);
        assert!(rendered.stdout.contains("-1.25 CNY"));
        assert!(!rendered.stdout.contains("cnys"));
        assert!(
            report
                .metrics
                .iter()
                .all(|m| m.limit.is_none() && m.resets_at.is_none())
        );
    }

    #[test]
    fn rejects_empty_duplicate_unsupported_or_invalid_balances() {
        assert!(parse(json!({"is_available":true,"balance_infos":[]})).is_err());
        for amount in ["NaN", "inf", "1e999", "bad", ""] {
            assert!(parse(json!({"is_available":true,"balance_infos":[{"currency":"USD","total_balance":amount}]})).is_err());
        }
        for currency in ["EUR", "", "usd"] {
            assert!(parse(json!({"is_available":true,"balance_infos":[{"currency":currency,"total_balance":"1"}]})).is_err());
        }
        let balance = json!({"currency":"USD","total_balance":"1"});
        assert!(
            parse(json!({"is_available":true,"balance_infos":[balance.clone(),balance]})).is_err()
        );
    }
}
