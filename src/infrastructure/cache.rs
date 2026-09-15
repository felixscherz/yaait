use std::{fs, path::PathBuf};

use chrono::{DateTime, Duration, Utc};
use tempfile::NamedTempFile;

use crate::UsageReport;

use super::registry::{set_private_dir, set_private_file};

/// Default freshness window for cached usage reports.
pub const DEFAULT_CACHE_TTL: Duration = Duration::minutes(5);

const CACHE_FILE: &str = "usage.json";

/// File-backed cache for one tracker's usage report.
///
/// All operations are best-effort: any I/O or decoding problem on read is a
/// cache miss, and a failed store is dropped so caching never breaks usage
/// collection.
#[derive(Clone, Debug)]
pub struct ReportCache {
    path: PathBuf,
}

impl ReportCache {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: cache_dir.into().join(CACHE_FILE),
        }
    }

    /// Returns the cached report when it exists and was observed within `ttl`.
    pub fn fresh(&self, ttl: Duration, now: DateTime<Utc>) -> Option<UsageReport> {
        let bytes = fs::read(&self.path).ok()?;
        let report: UsageReport = serde_json::from_slice(&bytes).ok()?;
        (now.signed_duration_since(report.observed_at) <= ttl).then_some(report)
    }

    /// Atomically replaces the cache entry with `report`.
    pub fn store(&self, report: &UsageReport) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let _ = set_private_dir(parent);
        let Ok(mut temp) = NamedTempFile::new_in(parent) else {
            return;
        };
        if serde_json::to_writer(temp.as_file_mut(), report).is_err()
            || temp.as_file().sync_all().is_err()
        {
            return;
        }
        if temp.persist(&self.path).is_ok() {
            let _ = set_private_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn report(observed_at: DateTime<Utc>) -> UsageReport {
        UsageReport {
            observed_at,
            identity: None,
            metrics: Vec::new(),
            attributes: Map::new(),
        }
    }

    #[test]
    fn a_missing_cache_file_is_a_miss() {
        let temp = tempfile::tempdir().unwrap();
        let cache = ReportCache::new(temp.path());
        assert_eq!(cache.fresh(DEFAULT_CACHE_TTL, Utc::now()), None);
    }

    #[test]
    fn a_report_is_fresh_until_the_ttl_passes() {
        let temp = tempfile::tempdir().unwrap();
        let cache = ReportCache::new(temp.path());
        let observed_at = Utc::now();
        cache.store(&report(observed_at));

        assert_eq!(
            cache.fresh(DEFAULT_CACHE_TTL, observed_at + Duration::minutes(4)),
            Some(report(observed_at))
        );
        assert_eq!(
            cache.fresh(DEFAULT_CACHE_TTL, observed_at + Duration::minutes(6)),
            None
        );
    }

    #[test]
    fn a_corrupt_cache_file_is_a_miss_and_can_be_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let cache = ReportCache::new(temp.path());
        fs::write(temp.path().join(CACHE_FILE), b"not json").unwrap();
        assert_eq!(cache.fresh(DEFAULT_CACHE_TTL, Utc::now()), None);

        let observed_at = Utc::now();
        cache.store(&report(observed_at));
        assert_eq!(
            cache.fresh(DEFAULT_CACHE_TTL, observed_at),
            Some(report(observed_at))
        );
    }
}
