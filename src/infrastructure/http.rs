use std::time::Duration;

use crate::TrackerError;

pub fn build_http_client() -> Result<reqwest::Client, TrackerError> {
    client_builder(true)
}

/// Allows plaintext transport for a LiteLLM origin explicitly configured as HTTP.
pub(crate) fn build_http_client_allow_http() -> Result<reqwest::Client, TrackerError> {
    client_builder(false)
}

fn client_builder(https_only: bool) -> Result<reqwest::Client, TrackerError> {
    reqwest::Client::builder()
        .https_only(https_only)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("yaait/0.1.0")
        .build()
        .map_err(|_| TrackerError::new("storage_error", "could not initialize HTTP client"))
}
