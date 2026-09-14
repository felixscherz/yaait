use serde::Serialize;
use serde_json::Value;

use crate::TrackerError;

#[derive(Clone, Debug, Serialize)]
pub struct Envelope {
    pub schema_version: u32,
    pub command: String,
    pub ok: bool,
    pub partial: bool,
    pub data: Value,
    pub warnings: Vec<TrackerError>,
    pub errors: Vec<TrackerError>,
}

impl Envelope {
    pub fn success<T: Serialize>(
        command: impl Into<String>,
        data: T,
        warnings: Vec<TrackerError>,
    ) -> Self {
        Self {
            schema_version: 1,
            command: command.into(),
            ok: true,
            partial: false,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
            warnings,
            errors: Vec::new(),
        }
    }

    pub fn failure(command: impl Into<String>, error: TrackerError) -> Self {
        Self {
            schema_version: 1,
            command: command.into(),
            ok: false,
            partial: false,
            data: Value::Null,
            warnings: Vec::new(),
            errors: vec![error],
        }
    }

    pub fn usage<T: Serialize>(
        data: T,
        warnings: Vec<TrackerError>,
        errors: Vec<TrackerError>,
    ) -> Self {
        let reports_present = serde_json::to_value(&data)
            .ok()
            .and_then(|value| {
                value
                    .get("trackers")?
                    .as_array()
                    .map(|items| !items.is_empty())
            })
            .unwrap_or(false);
        let failed = !errors.is_empty();
        Self {
            schema_version: 1,
            command: "usage".into(),
            ok: !failed,
            partial: failed && reports_present,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
            warnings,
            errors,
        }
    }

    pub fn exit_code(&self) -> u8 {
        if self.partial {
            2
        } else if self.ok {
            0
        } else {
            1
        }
    }
}
