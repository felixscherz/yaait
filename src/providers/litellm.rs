use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Local, Utc};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    Identity, MetricDescriptor, MetricKind, MetricTier, PreparedSetup, ProviderDescriptor,
    ProviderId, Secret, SecretMap, SetupContext, SetupField, SetupFieldKind, SetupInput,
    SetupSchema, TrackerContext, TrackerError, TrackerProvider, UsageMetric, UsageReport,
};

const BASE_URL: &str = "base_url";
const TOKEN: &str = "token";
const WINDOW: &str = "window";
const USER_ID: &str = "user_id";
const KEY_ALIAS: &str = "key_alias";
const TEAM_ID: &str = "team_id";
const TEAM_ALIAS: &str = "team_alias";
const DEFAULT_WINDOW: &str = "30d";

#[derive(Default)]
pub struct LiteLlmProvider {
    base_url_override: Option<String>,
}

impl LiteLlmProvider {
    async fn fetch_key_info(
        &self,
        http: &reqwest::Client,
        base_url: &str,
        token: &str,
    ) -> Result<KeyInfo, TrackerError> {
        let response = http
            .get(self.endpoint(base_url, "key/info"))
            .bearer_auth(token)
            .header("x-litellm-api-key", token)
            .send()
            .await
            .map_err(classify_transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_status(status));
        }
        let payload: KeyInfoResponse = response.json().await.map_err(|_| {
            TrackerError::new(
                "invalid_provider_response",
                "LiteLLM returned invalid key information",
            )
        })?;
        Ok(payload.info)
    }

    async fn fetch_activity(
        &self,
        http: &reqwest::Client,
        base_url: &str,
        token: &str,
        user_id: Option<&str>,
        window: ReportingWindow,
    ) -> Result<ActivityMetadata, TrackerError> {
        let range = DateRange::for_window(window);
        let mut query = vec![
            ("start_date", range.start_date),
            ("end_date", range.end_date),
            ("timezone", range.timezone_minutes.to_string()),
            ("include_current_utc_day", "true".into()),
        ];
        if let Some(user_id) = user_id {
            query.push((USER_ID, user_id.to_owned()));
        }
        let response = http
            .get(self.endpoint(base_url, "user/daily/activity/aggregated"))
            .bearer_auth(token)
            .header("x-litellm-api-key", token)
            .query(&query)
            .send()
            .await
            .map_err(classify_transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_status(status));
        }
        let payload: ActivityResponse = response.json().await.map_err(|_| {
            TrackerError::new(
                "invalid_provider_response",
                "LiteLLM returned invalid usage data",
            )
        })?;
        let metadata = payload.metadata.ok_or_else(|| {
            TrackerError::new(
                "invalid_provider_response",
                "LiteLLM usage data did not include totals",
            )
        })?;
        if !metadata.has_any_total() {
            return Err(TrackerError::new(
                "invalid_provider_response",
                "LiteLLM usage data did not include supported totals",
            ));
        }
        Ok(metadata)
    }

    async fn fetch_user_info(
        &self,
        http: &reqwest::Client,
        base_url: &str,
        token: &str,
        user_id: Option<&str>,
    ) -> Result<UserInfo, TrackerError> {
        let mut request = http
            .get(self.endpoint(base_url, "v2/user/info"))
            .bearer_auth(token)
            .header("x-litellm-api-key", token);
        if let Some(user_id) = user_id {
            request = request.query(&[(USER_ID, user_id)]);
        }
        let response = request.send().await.map_err(classify_transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(classify_status(status));
        }
        response.json().await.map_err(|_| {
            TrackerError::new(
                "invalid_provider_response",
                "LiteLLM returned invalid user information",
            )
        })
    }

    fn endpoint(&self, base_url: &str, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url_override.as_deref().unwrap_or(base_url),
            path
        )
    }

    #[cfg(test)]
    fn test_base_url(base_url: String) -> Self {
        Self {
            base_url_override: Some(base_url),
        }
    }
}

#[async_trait]
impl TrackerProvider for LiteLlmProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from_str("litellm").expect("static provider ID is valid"),
            name: "LiteLLM".into(),
            description: "Reports spend, token counts, request counts, and key or user budget state from a LiteLLM proxy.".into(),
            setup: SetupSchema {
                fields: vec![
                    SetupField {
                        key: BASE_URL.into(),
                        label: "LiteLLM base URL".into(),
                        description: "HTTPS origin of the LiteLLM proxy, for example https://ai.example.com.".into(),
                        kind: SetupFieldKind::String,
                        required: true,
                        allowed_values: None,
                    },
                    SetupField {
                        key: TOKEN.into(),
                        label: "LiteLLM virtual key".into(),
                        description: "A virtual key allowed to read its own key and spend data.".into(),
                        kind: SetupFieldKind::Secret,
                        required: true,
                        allowed_values: None,
                    },
                    SetupField {
                        key: WINDOW.into(),
                        label: "Reporting window".into(),
                        description: "Recent period included in cumulative usage metrics; defaults to 30d.".into(),
                        kind: SetupFieldKind::Choice,
                        required: false,
                        allowed_values: Some(vec!["7d".into(), "30d".into(), "90d".into()]),
                    },
                ],
            },
            metrics: metric_descriptors(),
        }
    }

    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        mut input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError> {
        let base_url = input
            .remove(BASE_URL)
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or_else(|| TrackerError::invalid("base_url must be a string"))?;
        let base_url = normalize_base_url(&base_url)?;
        let token = input
            .remove(TOKEN)
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or_else(|| TrackerError::invalid("token must be a string"))?;
        let window = ReportingWindow::parse(
            input
                .remove(WINDOW)
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_WINDOW),
        )?;

        let key_info = self.fetch_key_info(&ctx.http, &base_url, &token).await?;
        self.fetch_activity(
            &ctx.http,
            &base_url,
            &token,
            key_info.user_id.as_deref(),
            window,
        )
        .await?;
        if !key_info.has_budget() {
            self.fetch_user_info(&ctx.http, &base_url, &token, key_info.user_id.as_deref())
                .await?;
        }

        let mut public_settings = Map::new();
        public_settings.insert(BASE_URL.into(), Value::String(base_url));
        public_settings.insert(WINDOW.into(), Value::String(window.as_str().into()));
        insert_optional_string(&mut public_settings, USER_ID, key_info.user_id);
        insert_optional_string(&mut public_settings, KEY_ALIAS, key_info.key_alias);
        insert_optional_string(&mut public_settings, TEAM_ID, key_info.team_id);
        insert_optional_string(&mut public_settings, TEAM_ALIAS, key_info.team_alias);
        let mut secrets = SecretMap::new();
        secrets.insert(TOKEN.into(), Secret::new(token));
        Ok(PreparedSetup {
            public_settings,
            secrets,
        })
    }

    async fn collect(&self, ctx: &TrackerContext) -> Result<UsageReport, TrackerError> {
        let base_url = setting_string(&ctx.tracker.settings, BASE_URL)?;
        let window = ReportingWindow::parse(setting_string(&ctx.tracker.settings, WINDOW)?)?;
        let token = ctx.credentials.get(TOKEN).ok_or_else(|| {
            TrackerError::new("authentication_failed", "tracker credential is missing")
        })?;
        let key_info = self
            .fetch_key_info(&ctx.http, base_url, token.expose())
            .await?;
        let user_id = key_info
            .user_id
            .as_deref()
            .or_else(|| ctx.tracker.settings.get(USER_ID).and_then(Value::as_str));
        let activity = self
            .fetch_activity(&ctx.http, base_url, token.expose(), user_id, window)
            .await?;
        let user_info = if key_info.has_budget() {
            None
        } else {
            Some(
                self.fetch_user_info(&ctx.http, base_url, token.expose(), user_id)
                    .await?,
            )
        };
        Ok(into_report(key_info, user_info, activity, window))
    }
}

fn metric_descriptors() -> Vec<MetricDescriptor> {
    [
        ("spend", "Spend", MetricKind::Counter, "usd"),
        (
            "prompt-tokens",
            "Prompt tokens",
            MetricKind::Counter,
            "token",
        ),
        (
            "completion-tokens",
            "Completion tokens",
            MetricKind::Counter,
            "token",
        ),
        (
            "cache-read-tokens",
            "Cache read tokens",
            MetricKind::Counter,
            "token",
        ),
        (
            "cache-creation-tokens",
            "Cache creation tokens",
            MetricKind::Counter,
            "token",
        ),
        ("total-tokens", "Total tokens", MetricKind::Counter, "token"),
        ("requests", "API requests", MetricKind::Counter, "request"),
        (
            "successful-requests",
            "Successful requests",
            MetricKind::Counter,
            "request",
        ),
        (
            "failed-requests",
            "Failed requests",
            MetricKind::Counter,
            "request",
        ),
        ("budget", "Budget", MetricKind::Quota, "usd"),
    ]
    .into_iter()
    .map(|(id, label, kind, unit)| MetricDescriptor {
        id: id.into(),
        label: label.into(),
        description: match id {
            "budget" => "Current spend against the applicable key or user budget.".into(),
            _ => format!(
                "Cumulative {label_lower} during the configured reporting window.",
                label_lower = label.to_lowercase()
            ),
        },
        kind,
        tier: if id == "budget" {
            MetricTier::Primary
        } else {
            MetricTier::Detail
        },
        unit: unit.into(),
    })
    .collect()
}

fn normalize_base_url(value: &str) -> Result<String, TrackerError> {
    let value = value.trim();
    let mut url = reqwest::Url::parse(value).map_err(|_| {
        TrackerError::invalid("base_url must be a valid HTTPS origin").detail("field", BASE_URL)
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(
            TrackerError::invalid("base_url must be a valid HTTPS origin")
                .detail("field", BASE_URL),
        );
    }
    url.set_path("");
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn classify_transport(error: reqwest::Error) -> TrackerError {
    if error.is_timeout() {
        TrackerError::new("timeout", "LiteLLM request timed out")
    } else {
        TrackerError::new("transport_error", "LiteLLM request failed")
    }
}

fn classify_status(status: StatusCode) -> TrackerError {
    match status {
        StatusCode::UNAUTHORIZED => TrackerError::new(
            "authentication_failed",
            "LiteLLM rejected the tracker credential",
        ),
        StatusCode::FORBIDDEN => TrackerError::new(
            "authorization_failed",
            "LiteLLM denied access to usage data",
        ),
        StatusCode::TOO_MANY_REQUESTS => {
            TrackerError::new("rate_limited", "LiteLLM rate limit was reached")
        }
        _ => TrackerError::new(
            "invalid_provider_response",
            "LiteLLM returned an unsuccessful response",
        )
        .detail("http_status", u64::from(status.as_u16())),
    }
}

fn setting_string<'a>(
    settings: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, TrackerError> {
    settings.get(key).and_then(Value::as_str).ok_or_else(|| {
        TrackerError::new(
            "invalid_provider_response",
            "LiteLLM tracker settings are incomplete",
        )
        .detail("field", key)
    })
}

fn insert_optional_string(settings: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        settings.insert(key.into(), Value::String(value));
    }
}

#[derive(Clone, Copy)]
enum ReportingWindow {
    Seven,
    Thirty,
    Ninety,
}

impl ReportingWindow {
    fn parse(value: &str) -> Result<Self, TrackerError> {
        match value {
            "7d" => Ok(Self::Seven),
            "30d" => Ok(Self::Thirty),
            "90d" => Ok(Self::Ninety),
            _ => Err(TrackerError::invalid("window is not supported").detail("field", WINDOW)),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Seven => "7d",
            Self::Thirty => "30d",
            Self::Ninety => "90d",
        }
    }

    fn days(self) -> i64 {
        match self {
            Self::Seven => 7,
            Self::Thirty => 30,
            Self::Ninety => 90,
        }
    }
}

struct DateRange {
    start_date: String,
    end_date: String,
    timezone_minutes: i32,
}

impl DateRange {
    fn for_window(window: ReportingWindow) -> Self {
        let now = Local::now();
        let end = now.date_naive();
        let start = end - Duration::days(window.days() - 1);
        Self {
            start_date: start.format("%Y-%m-%d").to_string(),
            end_date: end.format("%Y-%m-%d").to_string(),
            timezone_minutes: -now.offset().local_minus_utc() / 60,
        }
    }
}

#[derive(Deserialize)]
struct KeyInfoResponse {
    info: KeyInfo,
}

#[derive(Deserialize)]
struct KeyInfo {
    user_id: Option<String>,
    key_alias: Option<String>,
    team_id: Option<String>,
    team_alias: Option<String>,
    spend: Option<f64>,
    max_budget: Option<f64>,
    budget_duration: Option<String>,
    budget_reset_at: Option<String>,
}

impl KeyInfo {
    fn has_budget(&self) -> bool {
        self.max_budget.is_some_and(|limit| limit > 0.0)
    }
}

#[derive(Deserialize)]
struct UserInfo {
    spend: Option<f64>,
    max_budget: Option<f64>,
    budget_duration: Option<String>,
    budget_reset_at: Option<String>,
}

#[derive(Deserialize)]
struct ActivityResponse {
    metadata: Option<ActivityMetadata>,
}

#[derive(Deserialize)]
struct ActivityMetadata {
    total_spend: Option<f64>,
    total_prompt_tokens: Option<f64>,
    total_completion_tokens: Option<f64>,
    total_tokens: Option<f64>,
    total_api_requests: Option<f64>,
    total_successful_requests: Option<f64>,
    total_failed_requests: Option<f64>,
    total_cache_read_input_tokens: Option<f64>,
    total_cache_creation_input_tokens: Option<f64>,
}

impl ActivityMetadata {
    fn has_any_total(&self) -> bool {
        [
            self.total_spend,
            self.total_prompt_tokens,
            self.total_completion_tokens,
            self.total_tokens,
            self.total_api_requests,
            self.total_successful_requests,
            self.total_failed_requests,
            self.total_cache_read_input_tokens,
            self.total_cache_creation_input_tokens,
        ]
        .into_iter()
        .any(|value| value.is_some())
    }
}

fn into_report(
    key_info: KeyInfo,
    user_info: Option<UserInfo>,
    activity: ActivityMetadata,
    window: ReportingWindow,
) -> UsageReport {
    let mut metrics = Vec::new();
    let counters = [
        ("spend", "Spend", "usd", activity.total_spend),
        (
            "prompt-tokens",
            "Prompt tokens",
            "token",
            activity.total_prompt_tokens,
        ),
        (
            "completion-tokens",
            "Completion tokens",
            "token",
            activity.total_completion_tokens,
        ),
        (
            "cache-read-tokens",
            "Cache read tokens",
            "token",
            activity.total_cache_read_input_tokens,
        ),
        (
            "cache-creation-tokens",
            "Cache creation tokens",
            "token",
            activity.total_cache_creation_input_tokens,
        ),
        (
            "total-tokens",
            "Total tokens",
            "token",
            activity.total_tokens,
        ),
        (
            "requests",
            "API requests",
            "request",
            activity.total_api_requests,
        ),
        (
            "successful-requests",
            "Successful requests",
            "request",
            activity.total_successful_requests,
        ),
        (
            "failed-requests",
            "Failed requests",
            "request",
            activity.total_failed_requests,
        ),
    ];
    for (id, label, unit, value) in counters {
        if let Some(value) = value {
            metrics.push(counter_metric(id, label, unit, value, window.as_str()));
        }
    }
    let budget = key_info
        .max_budget
        .filter(|limit| *limit > 0.0)
        .map(|limit| {
            (
                "Key budget",
                "key",
                limit,
                key_info.spend,
                key_info.budget_duration.as_deref(),
                key_info.budget_reset_at.as_deref(),
            )
        })
        .or_else(|| {
            let user_info = user_info.as_ref()?;
            user_info
                .max_budget
                .filter(|limit| *limit > 0.0)
                .map(|limit| {
                    (
                        "User budget",
                        "user",
                        limit,
                        user_info.spend,
                        user_info.budget_duration.as_deref(),
                        user_info.budget_reset_at.as_deref(),
                    )
                })
        });
    if let Some((label, scope, limit, spend, period, resets_at)) = budget {
        let used = spend.filter(|spend| *spend >= 0.0);
        let mut attributes = Map::new();
        attributes.insert("scope".into(), Value::String(scope.into()));
        metrics.push(UsageMetric {
            id: "budget".into(),
            label: label.into(),
            kind: MetricKind::Quota,
            tier: MetricTier::Primary,
            unit: "usd".into(),
            used,
            remaining: used.map(|spend| (limit - spend).max(0.0)),
            limit: Some(limit),
            value: None,
            period: period.map(ToOwned::to_owned),
            resets_at: resets_at.and_then(parse_date_time),
            attributes,
        });
    }
    let identity = identity(&key_info);
    let mut attributes = Map::new();
    insert_optional_string(&mut attributes, USER_ID, key_info.user_id);
    insert_optional_string(&mut attributes, TEAM_ID, key_info.team_id);
    UsageReport {
        observed_at: Utc::now(),
        identity,
        metrics,
        attributes,
    }
}

fn identity(key_info: &KeyInfo) -> Option<Identity> {
    let account = key_info
        .key_alias
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| key_info.user_id.clone().filter(|value| !value.is_empty()));
    let organization = key_info
        .team_alias
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| key_info.team_id.clone().filter(|value| !value.is_empty()));
    if account.is_none() && organization.is_none() {
        None
    } else {
        Some(Identity {
            account,
            organization,
            plan: None,
        })
    }
}

fn counter_metric(id: &str, label: &str, unit: &str, value: f64, period: &str) -> UsageMetric {
    UsageMetric {
        id: id.into(),
        label: label.into(),
        kind: MetricKind::Counter,
        tier: MetricTier::Detail,
        unit: unit.into(),
        used: None,
        remaining: None,
        limit: None,
        value: Some(value),
        period: Some(period.into()),
        resets_at: None,
        attributes: Map::new(),
    }
}

fn parse_date_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|date| date.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use chrono::TimeZone;
    use httpmock::{Method::GET, MockServer};

    use super::*;
    use crate::{TrackerId, TrackerManifest};

    fn setup_context() -> SetupContext {
        SetupContext {
            tracker_id: "work".parse().unwrap(),
            data_dir: PathBuf::from("data"),
            cache_dir: PathBuf::from("cache"),
            http: reqwest::Client::new(),
        }
    }

    fn setup_input() -> SetupInput {
        let mut input = Map::new();
        input.insert(
            BASE_URL.into(),
            Value::String("https://ai.example.com/".into()),
        );
        input.insert(TOKEN.into(), Value::String("sk-user-secret".into()));
        input
    }

    fn key_info_body() -> Value {
        serde_json::json!({
            "key": "redacted",
            "info": {
                "user_id": "user-7",
                "key_alias": "work-key",
                "team_id": "team-3",
                "team_alias": "platform",
                "spend": 4.25,
                "max_budget": 10.0,
                "budget_duration": "30d",
                "budget_reset_at": "2026-10-01T00:00:00Z"
            }
        })
    }

    fn activity_body() -> Value {
        serde_json::json!({
            "results": [],
            "metadata": {
                "total_spend": 3.5,
                "total_prompt_tokens": 1000,
                "total_completion_tokens": 200,
                "total_cache_read_input_tokens": 700,
                "total_cache_creation_input_tokens": 50,
                "total_tokens": 1200,
                "total_api_requests": 8,
                "total_successful_requests": 7,
                "total_failed_requests": 1
            }
        })
    }

    fn key_info_without_budget_body() -> Value {
        serde_json::json!({
            "key": "redacted",
            "info": {
                "user_id": "user-7",
                "key_alias": "management-key",
                "team_id": "team-3",
                "spend": 0.0,
                "max_budget": null,
                "budget_duration": null,
                "budget_reset_at": null
            }
        })
    }

    fn tracker_context() -> TrackerContext {
        let mut settings = Map::new();
        settings.insert(
            BASE_URL.into(),
            Value::String("https://ai.example.com".into()),
        );
        settings.insert(WINDOW.into(), Value::String("30d".into()));
        settings.insert(USER_ID.into(), Value::String("user-7".into()));
        let mut credentials = BTreeMap::new();
        credentials.insert(TOKEN.into(), Secret::new("sk-user-secret"));
        TrackerContext {
            tracker: TrackerManifest {
                schema_version: 1,
                id: TrackerId::from_str("work").unwrap(),
                provider: ProviderId::from_str("litellm").unwrap(),
                name: "Work".into(),
                description: None,
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                settings,
                extensions: BTreeMap::new(),
            },
            data_dir: PathBuf::from("data"),
            cache_dir: PathBuf::from("cache"),
            credentials,
            http: reqwest::Client::new(),
        }
    }

    async fn mock_success(server: &MockServer) {
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/key/info")
                    .header("authorization", "Bearer sk-user-secret")
                    .header("x-litellm-api-key", "sk-user-secret");
                then.status(200).json_body(key_info_body());
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/user/daily/activity/aggregated")
                    .query_param(USER_ID, "user-7")
                    .query_param("include_current_utc_day", "true")
                    .header("authorization", "Bearer sk-user-secret");
                then.status(200).json_body(activity_body());
            })
            .await;
    }

    async fn mock_user_budget_success(server: &MockServer) {
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/key/info")
                    .header("authorization", "Bearer sk-user-secret");
                then.status(200).json_body(key_info_without_budget_body());
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/user/daily/activity/aggregated")
                    .query_param(USER_ID, "user-7")
                    .header("authorization", "Bearer sk-user-secret");
                then.status(200).json_body(activity_body());
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/v2/user/info")
                    .query_param(USER_ID, "user-7")
                    .header("authorization", "Bearer sk-user-secret")
                    .header("x-litellm-api-key", "sk-user-secret");
                then.status(200).json_body(serde_json::json!({
                    "user_id": "user-7",
                    "spend": 3.78752364,
                    "max_budget": 40.0,
                    "budget_duration": "30d",
                    "budget_reset_at": "2026-10-01T00:00:00Z"
                }));
            })
            .await;
    }

    async fn mock_key_info(server: &MockServer, status: u16, body: Value) {
        server
            .mock_async(move |when, then| {
                when.method(GET).path("/key/info");
                then.status(status).json_body(body);
            })
            .await;
    }

    #[test]
    fn normalizes_only_https_origins() {
        assert_eq!(
            normalize_base_url(" https://ai.example.com/ ").unwrap(),
            "https://ai.example.com"
        );
        for invalid in [
            "http://ai.example.com",
            "https://user@ai.example.com",
            "https://ai.example.com/ui",
            "https://ai.example.com?token=secret",
        ] {
            let error = normalize_base_url(invalid).unwrap_err();
            assert_eq!(error.code, "invalid_input");
            assert_eq!(error.details["field"], BASE_URL);
        }
    }

    #[tokio::test]
    async fn validates_and_redacts_setup() {
        let server = MockServer::start_async().await;
        mock_success(&server).await;
        let provider = LiteLlmProvider::test_base_url(server.base_url());

        let prepared = provider
            .validate_setup(&setup_context(), setup_input())
            .await
            .unwrap();

        assert_eq!(prepared.public_settings[BASE_URL], "https://ai.example.com");
        assert_eq!(prepared.public_settings[WINDOW], DEFAULT_WINDOW);
        assert_eq!(prepared.public_settings[USER_ID], "user-7");
        assert_eq!(prepared.public_settings[KEY_ALIAS], "work-key");
        assert_eq!(prepared.secrets[TOKEN].expose(), "sk-user-secret");
        assert!(!format!("{prepared:?}").contains("sk-user-secret"));
    }

    #[tokio::test]
    async fn setup_classifies_an_upstream_authentication_failure() {
        let server = MockServer::start_async().await;
        mock_key_info(
            &server,
            StatusCode::UNAUTHORIZED.as_u16(),
            serde_json::json!({"error": {"message": "secret body"}}),
        )
        .await;
        let provider = LiteLlmProvider::test_base_url(server.base_url());

        let error = provider
            .validate_setup(&setup_context(), setup_input())
            .await
            .unwrap_err();

        assert_eq!(error.code, "authentication_failed");
        assert!(!error.to_string().contains("secret body"));
    }

    #[tokio::test]
    async fn setup_rejects_key_info_without_the_expected_shape() {
        let server = MockServer::start_async().await;
        mock_key_info(
            &server,
            StatusCode::OK.as_u16(),
            serde_json::json!({"unexpected": true}),
        )
        .await;
        let provider = LiteLlmProvider::test_base_url(server.base_url());

        let error = provider
            .validate_setup(&setup_context(), setup_input())
            .await
            .unwrap_err();

        assert_eq!(error.code, "invalid_provider_response");
    }

    #[tokio::test]
    async fn collects_usage_and_budget_metrics() {
        let server = MockServer::start_async().await;
        mock_success(&server).await;
        let provider = LiteLlmProvider::test_base_url(server.base_url());
        let context = tracker_context();

        let report = provider.collect(&context).await.unwrap();

        crate::validate_report(&report).unwrap();
        assert_eq!(
            report.identity.as_ref().unwrap().account.as_deref(),
            Some("work-key")
        );
        assert_eq!(report.metrics.len(), 10);
        assert_eq!(report.metrics[0].id, "spend");
        assert_eq!(report.metrics[0].value, Some(3.5));
        let budget = report
            .metrics
            .iter()
            .find(|metric| metric.id == "budget")
            .unwrap();
        assert_eq!(budget.used, Some(4.25));
        assert_eq!(budget.tier, MetricTier::Primary);
        assert_eq!(budget.remaining, Some(5.75));
        assert_eq!(budget.limit, Some(10.0));
        assert_eq!(budget.attributes["scope"], "key");
        assert_eq!(
            budget.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap())
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_user_budget_when_the_key_has_none() {
        let server = MockServer::start_async().await;
        mock_user_budget_success(&server).await;
        let provider = LiteLlmProvider::test_base_url(server.base_url());

        let report = provider.collect(&tracker_context()).await.unwrap();

        crate::validate_report(&report).unwrap();
        let budget = report
            .metrics
            .iter()
            .find(|metric| metric.id == "budget")
            .unwrap();
        assert_eq!(budget.label, "User budget");
        assert_eq!(budget.used, Some(3.78752364));
        assert_eq!(budget.remaining, Some(36.21247636));
        assert_eq!(budget.limit, Some(40.0));
        assert_eq!(budget.period.as_deref(), Some("30d"));
        assert_eq!(budget.attributes["scope"], "user");
        assert_eq!(
            budget.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn omits_a_budget_without_a_positive_limit() {
        let key_info: KeyInfo = serde_json::from_value(serde_json::json!({
            "spend": 2.0,
            "max_budget": null
        }))
        .unwrap();
        let activity: ActivityMetadata = serde_json::from_value(serde_json::json!({
            "total_spend": 2.0
        }))
        .unwrap();

        let report = into_report(key_info, None, activity, ReportingWindow::Seven);

        assert_eq!(report.metrics.len(), 1);
        assert_eq!(report.metrics[0].period.as_deref(), Some("7d"));
    }

    #[test]
    fn classifies_status_without_exposing_a_body() {
        for (status, code) in [
            (StatusCode::UNAUTHORIZED, "authentication_failed"),
            (StatusCode::FORBIDDEN, "authorization_failed"),
            (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            (StatusCode::BAD_GATEWAY, "invalid_provider_response"),
        ] {
            let error = classify_status(status);
            assert_eq!(error.code, code);
            assert!(!error.to_string().contains("secret"));
        }
    }
}
