use std::time::Duration;

use crate::TrackerError;

pub fn build_http_client() -> Result<reqwest::Client, TrackerError> {
    reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("yaait/0.1.0")
        .build()
        .map_err(|_| TrackerError::new("storage_error", "could not initialize HTTP client"))
}
