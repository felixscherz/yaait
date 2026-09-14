use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{TrackerError, TrackerManifest};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Quota,
    Balance,
    Counter,
    Gauge,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetricTier {
    Primary,
    #[default]
    Detail,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageMetric {
    pub id: String,
    pub label: String,
    pub kind: MetricKind,
    #[serde(default)]
    pub tier: MetricTier,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageReport {
    pub observed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    pub metrics: Vec<UsageMetric>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackerReport {
    pub id: super::TrackerId,
    pub provider: super::ProviderId,
    pub name: String,
    pub description: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub identity: Option<Identity>,
    pub metrics: Vec<UsageMetric>,
    pub attributes: Map<String, Value>,
}

impl TrackerReport {
    pub fn from_report(tracker: &TrackerManifest, report: UsageReport) -> Self {
        Self {
            id: tracker.id.clone(),
            provider: tracker.provider.clone(),
            name: tracker.name.clone(),
            description: tracker.description.clone(),
            observed_at: report.observed_at,
            identity: report.identity,
            metrics: report.metrics,
            attributes: report.attributes,
        }
    }
}

pub fn validate_report(report: &UsageReport) -> Result<(), TrackerError> {
    if report.identity.as_ref().is_some_and(|identity| {
        identity.account.is_none() && identity.organization.is_none() && identity.plan.is_none()
    }) {
        return Err(TrackerError::new(
            "invalid_provider_response",
            "provider returned an empty identity",
        ));
    }
    let mut ids = BTreeSet::new();
    for metric in &report.metrics {
        if !ids.insert(&metric.id) {
            return Err(TrackerError::new(
                "invalid_provider_response",
                "provider returned duplicate metric IDs",
            ));
        }
        if metric.id.parse::<super::ProviderId>().is_err()
            || metric.unit.parse::<super::ProviderId>().is_err()
        {
            return Err(TrackerError::new(
                "invalid_provider_response",
                "provider returned an invalid metric ID or unit",
            ));
        }
        let numbers = [metric.used, metric.remaining, metric.limit, metric.value];
        if numbers
            .into_iter()
            .flatten()
            .any(|number| !number.is_finite())
            || [metric.used, metric.remaining]
                .into_iter()
                .flatten()
                .any(|number| number < 0.0)
            || metric.limit.is_some_and(|number| number <= 0.0)
        {
            return Err(TrackerError::new(
                "invalid_provider_response",
                "provider returned invalid numeric metric data",
            ));
        }
        let valid_shape = match metric.kind {
            MetricKind::Quota => {
                (metric.used.is_some() || metric.remaining.is_some() || metric.limit.is_some())
                    && metric.value.is_none()
            }
            MetricKind::Balance => {
                metric.value.is_some()
                    && metric.used.is_none()
                    && metric.remaining.is_none()
                    && metric.limit.is_none()
                    && metric.period.is_none()
                    && metric.resets_at.is_none()
            }
            MetricKind::Counter => {
                metric.value.is_some()
                    && metric.used.is_none()
                    && metric.remaining.is_none()
                    && metric.limit.is_none()
            }
            MetricKind::Gauge => {
                metric.value.is_some()
                    && metric.used.is_none()
                    && metric.remaining.is_none()
                    && metric.limit.is_none()
                    && metric.period.is_none()
                    && metric.resets_at.is_none()
            }
        };
        if !valid_shape {
            return Err(TrackerError::new(
                "invalid_provider_response",
                "provider returned an invalid metric shape",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(id: &str) -> UsageMetric {
        UsageMetric {
            id: id.into(),
            label: "Quota".into(),
            kind: MetricKind::Quota,
            tier: MetricTier::Detail,
            unit: "request".into(),
            used: None,
            remaining: Some(2.0),
            limit: Some(10.0),
            value: None,
            period: Some("month".into()),
            resets_at: None,
            attributes: Map::new(),
        }
    }

    #[test]
    fn rejects_duplicate_metrics() {
        let report = UsageReport {
            observed_at: Utc::now(),
            identity: None,
            metrics: vec![quota("chat"), quota("chat")],
            attributes: Map::new(),
        };
        assert_eq!(
            validate_report(&report).unwrap_err().code,
            "invalid_provider_response"
        );
    }
}
