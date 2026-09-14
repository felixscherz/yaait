use std::{collections::BTreeMap, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    MetricDescriptor, MetricKind, MetricTier, PreparedSetup, ProviderDescriptor, ProviderId,
    Secret, SecretMap, SetupContext, SetupField, SetupFieldKind, SetupInput, SetupSchema,
    TrackerContext, TrackerError, TrackerProvider, UsageMetric, UsageReport,
};

const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";
const ENTERPRISE_URL: &str = "enterprise_url";

#[derive(Default)]
pub struct GitHubCopilotProvider {
    endpoint_override: Option<String>,
}

impl GitHubCopilotProvider {
    async fn fetch(
        &self,
        http: &reqwest::Client,
        endpoint: &str,
        token: &str,
    ) -> Result<UsageReport, TrackerError> {
        let response = http
            .get(endpoint)
            .header(header::AUTHORIZATION, format!("token {token}"))
            .header(header::ACCEPT, "application/json")
            .header("Editor-Version", "vscode/1.96.2")
            .header("Editor-Plugin-Version", "copilot-chat/0.26.7")
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .header("X-GitHub-Api-Version", "2025-04-01")
            .send()
            .await
            .map_err(classify_transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_status(status, response.headers()));
        }
        let payload: CopilotResponse = response.json().await.map_err(|_| {
            TrackerError::new(
                "invalid_provider_response",
                "GitHub Copilot returned an invalid response",
            )
        })?;
        Ok(payload.into_report())
    }

    #[cfg(test)]
    fn test_endpoint(endpoint: String) -> Self {
        Self {
            endpoint_override: Some(endpoint),
        }
    }

    fn endpoint(&self, enterprise_url: Option<&str>) -> Result<String, TrackerError> {
        if let Some(endpoint) = &self.endpoint_override {
            return Ok(endpoint.clone());
        }
        enterprise_url.map_or_else(|| Ok(ENDPOINT.into()), enterprise_endpoint)
    }
}

#[async_trait]
impl TrackerProvider for GitHubCopilotProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from_str("github-copilot").expect("static provider ID is valid"),
            name: "GitHub Copilot".into(),
            description: "Reports Copilot request quota for the account represented by a supplied GitHub token, including accounts hosted on GitHub Enterprise.".into(),
            setup: SetupSchema {
                fields: vec![
                    SetupField {
                        key: "token".into(),
                        label: "GitHub token".into(),
                        description: "A GitHub token authorized for the Copilot usage endpoint."
                            .into(),
                        kind: SetupFieldKind::Secret,
                        required: true,
                        allowed_values: None,
                    },
                    SetupField {
                        key: ENTERPRISE_URL.into(),
                        label: "GitHub Enterprise URL or domain".into(),
                        description: "The GitHub Enterprise domain or HTTPS URL, for example octocorp.ghe.com or https://octocorp.ghe.com. Omit for GitHub.com.".into(),
                        kind: SetupFieldKind::String,
                        required: false,
                        allowed_values: None,
                    },
                ],
            },
            metrics: vec![
                MetricDescriptor {
                    id: "chat".into(),
                    label: "Chat requests".into(),
                    description: "Monthly Copilot chat request quota.".into(),
                    kind: MetricKind::Quota,
                    tier: MetricTier::Detail,
                    unit: "request".into(),
                },
                MetricDescriptor {
                    id: "premium-interactions".into(),
                    label: "Premium interactions".into(),
                    description: "Monthly Copilot premium interaction quota.".into(),
                    kind: MetricKind::Quota,
                    tier: MetricTier::Primary,
                    unit: "request".into(),
                },
            ],
        }
    }

    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        mut input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError> {
        let token = input
            .remove("token")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or_else(|| TrackerError::invalid("token must be a string"))?;
        let enterprise_url = input
            .remove(ENTERPRISE_URL)
            .and_then(|value| value.as_str().map(ToOwned::to_owned));
        let enterprise_url = enterprise_url
            .as_deref()
            .map(normalize_enterprise_url)
            .transpose()?;
        let endpoint = self.endpoint(enterprise_url.as_deref())?;
        self.fetch(&ctx.http, &endpoint, &token).await?;
        let mut public_settings = Map::new();
        if let Some(enterprise_url) = enterprise_url {
            public_settings.insert(ENTERPRISE_URL.into(), Value::String(enterprise_url));
        }
        let mut secrets = SecretMap::new();
        secrets.insert("token".into(), Secret::new(token));
        Ok(PreparedSetup {
            public_settings,
            secrets,
        })
    }

    async fn collect(&self, ctx: &TrackerContext) -> Result<UsageReport, TrackerError> {
        let token = ctx.credentials.get("token").ok_or_else(|| {
            TrackerError::new("authentication_failed", "tracker credential is missing")
        })?;
        let enterprise_url = ctx
            .tracker
            .settings
            .get(ENTERPRISE_URL)
            .and_then(Value::as_str);
        let endpoint = self.endpoint(enterprise_url)?;
        self.fetch(&ctx.http, &endpoint, token.expose()).await
    }
}

fn normalize_enterprise_url(value: &str) -> Result<String, TrackerError> {
    let value = value.trim();
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let mut url = reqwest::Url::parse(&candidate).map_err(|_| {
        TrackerError::invalid("enterprise_url must be a valid domain or HTTPS URL")
            .detail("field", ENTERPRISE_URL)
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(
            TrackerError::invalid("enterprise_url must be a valid domain or HTTPS URL")
                .detail("field", ENTERPRISE_URL),
        );
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn enterprise_endpoint(value: &str) -> Result<String, TrackerError> {
    let normalized = normalize_enterprise_url(value)?;
    let mut url =
        reqwest::Url::parse(&normalized).expect("a normalized GitHub Enterprise URL remains valid");
    let api_host = format!(
        "api.{}",
        url.host_str()
            .expect("a normalized GitHub Enterprise URL has a host")
    );
    url.set_host(Some(&api_host)).map_err(|_| {
        TrackerError::invalid("enterprise_url must use a DNS hostname")
            .detail("field", ENTERPRISE_URL)
    })?;
    url.set_path("/copilot_internal/user");
    Ok(url.into())
}

fn classify_transport(error: reqwest::Error) -> TrackerError {
    if error.is_timeout() {
        TrackerError::new("timeout", "GitHub Copilot request timed out")
    } else {
        TrackerError::new("transport_error", "GitHub Copilot request failed")
    }
}

fn classify_status(status: StatusCode, headers: &header::HeaderMap) -> TrackerError {
    if status == StatusCode::UNAUTHORIZED
        || (status == StatusCode::FORBIDDEN
            && headers.get(header::RETRY_AFTER).is_none()
            && headers
                .get("X-RateLimit-Remaining")
                .is_none_or(|value| value != "0"))
    {
        TrackerError::new(
            "authentication_failed",
            "GitHub Copilot rejected the tracker credential",
        )
    } else if status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && (headers.get(header::RETRY_AFTER).is_some()
                || headers
                    .get("X-RateLimit-Remaining")
                    .is_some_and(|value| value == "0")))
    {
        TrackerError::new("rate_limited", "GitHub Copilot rate limit was reached")
    } else {
        TrackerError::new(
            "invalid_provider_response",
            "GitHub Copilot returned an unsuccessful response",
        )
        .detail("http_status", u64::from(status.as_u16()))
    }
}

#[derive(Deserialize)]
struct CopilotResponse {
    copilot_plan: Option<String>,
    quota_reset_date: Option<String>,
    #[serde(default)]
    quota_snapshots: BTreeMap<String, QuotaSnapshot>,
}

#[derive(Deserialize)]
struct QuotaSnapshot {
    remaining: Option<f64>,
    entitlement: Option<f64>,
    percent_remaining: Option<f64>,
}

impl CopilotResponse {
    fn into_report(self) -> UsageReport {
        let resets_at = self.quota_reset_date.as_deref().and_then(parse_reset);
        let metrics = [
            ("chat", "Chat requests", "chat"),
            (
                "premium-interactions",
                "Premium interactions",
                "premium_interactions",
            ),
        ]
        .into_iter()
        .filter_map(|(id, label, source)| {
            self.quota_snapshots
                .get(source)
                .map(|snapshot| quota_metric(id, label, snapshot, resets_at))
        })
        .collect();
        UsageReport {
            observed_at: Utc::now(),
            identity: self.copilot_plan.map(|plan| crate::Identity {
                account: None,
                organization: None,
                plan: Some(plan),
            }),
            metrics,
            attributes: Map::new(),
        }
    }
}

fn quota_metric(
    id: &str,
    label: &str,
    snapshot: &QuotaSnapshot,
    resets_at: Option<DateTime<Utc>>,
) -> UsageMetric {
    let mut attributes = Map::new();
    if let Some(percent) = snapshot.percent_remaining {
        attributes.insert("percent_remaining".into(), Value::from(percent));
    }
    UsageMetric {
        id: id.into(),
        label: label.into(),
        kind: MetricKind::Quota,
        tier: if id == "premium-interactions" {
            MetricTier::Primary
        } else {
            MetricTier::Detail
        },
        unit: "request".into(),
        used: None,
        remaining: snapshot.remaining,
        // GitHub uses zero to represent plans with no entitlement for this quota.
        // The core schema requires any reported limit to be positive, while a
        // quota containing only `remaining: 0` is still valid and useful.
        limit: snapshot.entitlement.filter(|limit| *limit != 0.0),
        value: None,
        period: Some("month".into()),
        resets_at,
        attributes,
    }
}

fn parse_reset(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use httpmock::{Method::GET, MockServer};

    use super::*;

    #[test]
    fn parses_supported_quota_fields() {
        let payload: CopilotResponse = serde_json::from_value(serde_json::json!({
            "copilot_plan": "business",
            "quota_reset_date": "2026-10-01",
            "quota_snapshots": {
                "chat": {"remaining": 12, "entitlement": 20, "percent_remaining": 60},
                "premium_interactions": {"remaining": 0, "entitlement": 0, "percent_remaining": 0}
            }
        }))
        .unwrap();
        let report = payload.into_report();
        assert_eq!(
            report.identity.as_ref().unwrap().plan.as_deref(),
            Some("business")
        );
        assert_eq!(report.metrics.len(), 2);
        assert_eq!(report.metrics[0].remaining, Some(12.0));
        assert_eq!(report.metrics[0].tier, MetricTier::Detail);
        assert_eq!(report.metrics[1].id, "premium-interactions");
        assert_eq!(report.metrics[1].tier, MetricTier::Primary);
        assert_eq!(report.metrics[1].remaining, Some(0.0));
        assert_eq!(report.metrics[1].limit, None);
        crate::validate_report(&report).unwrap();
    }

    #[test]
    fn status_errors_never_include_bodies_or_secrets() {
        let error = classify_status(StatusCode::UNAUTHORIZED, &header::HeaderMap::new());
        assert_eq!(error.code, "authentication_failed");
        assert!(error.details.is_empty());
    }

    #[test]
    fn test_endpoint_constructor_is_available_for_adapter_tests() {
        let provider = GitHubCopilotProvider::test_endpoint("http://127.0.0.1:9".into());
        assert_eq!(provider.endpoint(None).unwrap(), "http://127.0.0.1:9");
    }

    #[test]
    fn builds_the_usage_endpoint_for_github_enterprise() {
        assert_eq!(
            GitHubCopilotProvider::default().endpoint(None).unwrap(),
            ENDPOINT
        );
        assert_eq!(
            enterprise_endpoint("https://octocorp.ghe.com/login?source=test#fragment").unwrap(),
            "https://api.octocorp.ghe.com/copilot_internal/user"
        );
        assert_eq!(
            enterprise_endpoint("octocorp.ghe.com").unwrap(),
            "https://api.octocorp.ghe.com/copilot_internal/user"
        );
    }

    #[test]
    fn rejects_insecure_enterprise_urls() {
        let error = enterprise_endpoint("http://github.example.com").unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert_eq!(error.details["field"], ENTERPRISE_URL);
    }

    #[tokio::test]
    async fn validates_each_supplied_token_against_the_usage_endpoint() {
        let server = MockServer::start_async().await;
        let request = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/copilot_internal/user")
                    .header("authorization", "token account-specific-token");
                then.status(200).json_body(serde_json::json!({
                    "copilot_plan": "individual",
                    "quota_snapshots": {"chat": {"remaining": 7, "entitlement": 10}}
                }));
            })
            .await;
        let provider = GitHubCopilotProvider::test_endpoint(server.url("/copilot_internal/user"));
        let temp = tempfile::tempdir().unwrap();
        let context = SetupContext {
            tracker_id: "personal".parse().unwrap(),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            http: reqwest::Client::new(),
        };
        let mut input = Map::new();
        input.insert(
            "token".into(),
            Value::String("account-specific-token".into()),
        );
        input.insert(
            ENTERPRISE_URL.into(),
            Value::String("https://octocorp.ghe.com/login".into()),
        );

        let prepared = provider.validate_setup(&context, input).await.unwrap();

        request.assert_async().await;
        assert_eq!(prepared.secrets["token"].expose(), "account-specific-token");
        assert_eq!(
            prepared.public_settings[ENTERPRISE_URL],
            "https://octocorp.ghe.com"
        );
    }
}
