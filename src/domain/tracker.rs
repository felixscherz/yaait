use std::{collections::BTreeMap, fmt, path::PathBuf, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::TrackerError;

fn valid_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

macro_rules! slug_id {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = TrackerError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if valid_slug(value) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(TrackerError::invalid(concat!(
                        $label,
                        " must match [a-z][a-z0-9-]{0,62}"
                    )))
                }
            }
        }
    };
}

slug_id!(TrackerId, "tracker ID");
slug_id!(ProviderId, "provider ID");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrackerManifest {
    pub schema_version: u32,
    pub id: TrackerId,
    pub provider: ProviderId,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub settings: Map<String, Value>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackerSummary {
    pub id: TrackerId,
    pub provider: ProviderId,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackerDetail {
    #[serde(flatten)]
    pub summary: TrackerSummary,
    pub settings: Map<String, Value>,
}

impl From<&TrackerManifest> for TrackerSummary {
    fn from(value: &TrackerManifest) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.provider.clone(),
            name: value.name.clone(),
            description: value.description.clone(),
            enabled: value.enabled,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&TrackerManifest> for TrackerDetail {
    fn from(value: &TrackerManifest) -> Self {
        Self {
            summary: value.into(),
            settings: value.settings.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

pub type SecretMap = BTreeMap<String, Secret>;

#[derive(Clone, Debug)]
pub struct SetupContext {
    pub tracker_id: TrackerId,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub http: reqwest::Client,
}

#[derive(Clone, Debug)]
pub struct TrackerContext {
    pub tracker: TrackerManifest,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub credentials: SecretMap,
    pub http: reqwest::Client,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ids_and_redacts_secrets() {
        assert!("work-2".parse::<TrackerId>().is_ok());
        for invalid in ["", "2work", "Work", "work/account", &"a".repeat(64)] {
            assert!(invalid.parse::<TrackerId>().is_err(), "accepted {invalid}");
        }
        assert_eq!(
            format!("{:?}", Secret::new("very-secret")),
            "Secret([REDACTED])"
        );
    }
}
