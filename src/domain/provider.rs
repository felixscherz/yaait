use std::collections::BTreeSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{ProviderId, SecretMap, SetupContext, TrackerContext, TrackerError, UsageReport};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupFieldKind {
    String,
    Secret,
    Path,
    Boolean,
    Choice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupField {
    pub key: String,
    pub label: String,
    pub description: String,
    pub kind: SetupFieldKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupSchema {
    pub fields: Vec<SetupField>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub kind: super::MetricKind,
    #[serde(default)]
    pub tier: super::MetricTier,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub name: String,
    pub description: String,
    pub setup: SetupSchema,
    pub metrics: Vec<MetricDescriptor>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderSummary {
    pub id: ProviderId,
    pub name: String,
    pub description: String,
}

impl From<&ProviderDescriptor> for ProviderSummary {
    fn from(value: &ProviderDescriptor) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            description: value.description.clone(),
        }
    }
}

pub type SetupInput = Map<String, Value>;

#[derive(Clone, Debug)]
pub struct PreparedSetup {
    pub public_settings: Map<String, Value>,
    pub secrets: SecretMap,
}

/// How a tracker should treat its cached usage report during `collect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CachePolicy {
    /// Reuse the cached report while it is still fresh.
    #[default]
    Cached,
    /// Ignore any cached report and query the provider API.
    Refresh,
}

pub fn validate_setup_input(schema: &SetupSchema, input: &SetupInput) -> Result<(), TrackerError> {
    let known: BTreeSet<_> = schema
        .fields
        .iter()
        .map(|field| field.key.as_str())
        .collect();
    if let Some(key) = input.keys().find(|key| !known.contains(key.as_str())) {
        return Err(
            TrackerError::invalid("setup input contains an unknown field")
                .detail("field", key.clone()),
        );
    }

    for field in &schema.fields {
        let Some(value) = input.get(&field.key) else {
            continue;
        };
        let valid_type = match field.kind {
            SetupFieldKind::String | SetupFieldKind::Secret | SetupFieldKind::Path => {
                value.is_string()
            }
            SetupFieldKind::Boolean => value.is_boolean(),
            SetupFieldKind::Choice => value.is_string(),
        };
        if !valid_type {
            return Err(TrackerError::invalid("setup input has the wrong JSON type")
                .detail("field", field.key.clone()));
        }
        if matches!(field.kind, SetupFieldKind::String | SetupFieldKind::Secret)
            && field.required
            && value.as_str().is_some_and(str::is_empty)
        {
            return Err(
                TrackerError::invalid("required setup input cannot be empty")
                    .detail("field", field.key.clone()),
            );
        }
        if field.kind == SetupFieldKind::Choice
            && field.allowed_values.as_ref().is_some_and(|allowed| {
                value
                    .as_str()
                    .is_some_and(|candidate| !allowed.iter().any(|item| item == candidate))
            })
        {
            return Err(TrackerError::invalid("setup choice is not allowed")
                .detail("field", field.key.clone()));
        }
    }

    let missing: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.required && !input.contains_key(&field.key))
        .map(|field| Value::String(field.key.clone()))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(
            TrackerError::new("setup_required", "required setup input is missing")
                .detail("fields", Value::Array(missing)),
        )
    }
}

#[async_trait]
pub trait TrackerProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError>;

    async fn collect(
        &self,
        ctx: &TrackerContext,
        policy: CachePolicy,
    ) -> Result<UsageReport, TrackerError>;
}
