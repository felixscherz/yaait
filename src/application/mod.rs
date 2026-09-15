use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use serde::Serialize;
use serde_json::Value;

use crate::{
    CachePolicy, MetricTier, ProviderDescriptor, ProviderId, ProviderSummary, SetupContext,
    SetupInput, TrackerContext, TrackerDetail, TrackerError, TrackerId, TrackerManifest,
    TrackerProvider, TrackerReport, TrackerSummary,
    infrastructure::{FileRegistry, WriterLock, build_http_client},
    validate_report, validate_setup_input,
};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub concurrency: usize,
    pub tracker_timeout: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            concurrency: 8,
            tracker_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AddRequest {
    pub id: TrackerId,
    pub provider: ProviderId,
    pub name: Option<String>,
    pub description: Option<String>,
    pub input: SetupInput,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UsageOptions {
    pub details: bool,
    pub cache: CachePolicy,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServiceResult<T: Serialize> {
    pub data: T,
    pub warnings: Vec<TrackerError>,
}

impl<T: Serialize> ServiceResult<T> {
    fn clean(data: T) -> Self {
        Self {
            data,
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProvidersData {
    pub providers: Vec<ProviderSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderData {
    pub provider: ProviderDescriptor,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackersData {
    pub trackers: Vec<TrackerSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackerData {
    pub tracker: TrackerDetail,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemovedData {
    pub removed: Option<RemovedTracker>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemovedTracker {
    pub id: TrackerId,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageData {
    pub trackers: Vec<TrackerReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageResult {
    pub data: UsageData,
    pub warnings: Vec<TrackerError>,
    pub errors: Vec<TrackerError>,
}

pub struct App {
    registry: FileRegistry,
    providers: BTreeMap<ProviderId, Arc<dyn TrackerProvider>>,
    http: reqwest::Client,
    config: AppConfig,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl App {
    pub fn new(registry: FileRegistry) -> Result<Self, TrackerError> {
        Ok(Self {
            registry,
            providers: BTreeMap::new(),
            http: build_http_client()?,
            config: AppConfig::default(),
            clock: Arc::new(Utc::now),
        })
    }

    pub fn with_config(mut self, config: AppConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub fn register_provider(&mut self, provider: Arc<dyn TrackerProvider>) {
        let id = provider.descriptor().id;
        self.providers.insert(id, provider);
    }

    pub fn providers(&self) -> ServiceResult<ProvidersData> {
        let providers = self
            .providers
            .values()
            .map(|provider| ProviderSummary::from(&provider.descriptor()))
            .collect();
        ServiceResult::clean(ProvidersData { providers })
    }

    pub fn describe_provider(
        &self,
        id: &ProviderId,
    ) -> Result<ServiceResult<ProviderData>, TrackerError> {
        let provider = self.providers.get(id).ok_or_else(|| {
            TrackerError::new("unknown_provider", "provider is not available")
                .provider(id.to_string())
        })?;
        Ok(ServiceResult::clean(ProviderData {
            provider: provider.descriptor(),
        }))
    }

    pub async fn add(
        &self,
        request: AddRequest,
    ) -> Result<ServiceResult<TrackerData>, TrackerError> {
        let _lock = WriterLock::acquire(&self.registry.paths().data_root)?;
        let provider = self.providers.get(&request.provider).ok_or_else(|| {
            TrackerError::new("unknown_provider", "provider is not available")
                .provider(request.provider.to_string())
        })?;
        validate_setup_input(&provider.descriptor().setup, &request.input)?;
        let context = SetupContext {
            tracker_id: request.id.clone(),
            data_dir: self.registry.paths().tracker_data(&request.id),
            cache_dir: self.registry.paths().tracker_cache(&request.id),
            http: self.http.clone(),
        };
        let prepared = provider
            .validate_setup(&context, request.input)
            .await
            .map_err(|error| {
                error
                    .tracker(request.id.to_string())
                    .provider(request.provider.to_string())
            })?;
        let now = (self.clock)();
        let manifest = TrackerManifest {
            schema_version: 1,
            id: request.id.clone(),
            provider: request.provider,
            name: request.name.unwrap_or_else(|| request.id.to_string()),
            description: request.description,
            enabled: true,
            created_at: now,
            updated_at: now,
            settings: prepared.public_settings,
            extensions: BTreeMap::new(),
        };
        self.registry.add(&manifest, &prepared.secrets)?;
        Ok(ServiceResult::clean(TrackerData {
            tracker: (&manifest).into(),
        }))
    }

    pub fn list(&self) -> Result<ServiceResult<TrackersData>, TrackerError> {
        let mut discovery = self.registry.discover()?;
        discovery.trackers.retain(|tracker| {
            if self.providers.contains_key(&tracker.provider) {
                true
            } else {
                discovery.warnings.push(
                    TrackerError::new("unknown_provider", "tracker provider is not available")
                        .tracker(tracker.id.to_string())
                        .provider(tracker.provider.to_string()),
                );
                false
            }
        });
        sort_warnings(&mut discovery.warnings);
        Ok(ServiceResult {
            data: TrackersData {
                trackers: discovery
                    .trackers
                    .iter()
                    .map(TrackerSummary::from)
                    .collect(),
            },
            warnings: discovery.warnings,
        })
    }

    pub fn show(&self, id: &TrackerId) -> Result<ServiceResult<TrackerData>, TrackerError> {
        let manifest = self.registry.load_manifest(id)?;
        self.ensure_provider(&manifest)?;
        Ok(ServiceResult::clean(TrackerData {
            tracker: (&manifest).into(),
        }))
    }

    pub async fn setup(
        &self,
        id: &TrackerId,
        input: SetupInput,
    ) -> Result<ServiceResult<TrackerData>, TrackerError> {
        let _lock = WriterLock::acquire(&self.registry.paths().data_root)?;
        let mut manifest = self.registry.load_manifest(id)?;
        let provider = self.ensure_provider(&manifest)?;
        validate_setup_input(&provider.descriptor().setup, &input)?;
        let context = SetupContext {
            tracker_id: id.clone(),
            data_dir: self.registry.paths().tracker_data(id),
            cache_dir: self.registry.paths().tracker_cache(id),
            http: self.http.clone(),
        };
        let prepared = provider
            .validate_setup(&context, input)
            .await
            .map_err(|error| {
                error
                    .tracker(id.to_string())
                    .provider(manifest.provider.to_string())
            })?;
        manifest.settings = prepared.public_settings;
        manifest.updated_at = (self.clock)();
        self.registry.replace_setup(&manifest, &prepared.secrets)?;
        Ok(ServiceResult::clean(TrackerData {
            tracker: (&manifest).into(),
        }))
    }

    pub fn set_enabled(
        &self,
        id: &TrackerId,
        enabled: bool,
    ) -> Result<ServiceResult<TrackerData>, TrackerError> {
        let _lock = WriterLock::acquire(&self.registry.paths().data_root)?;
        let mut manifest = self.registry.load_manifest(id)?;
        self.ensure_provider(&manifest)?;
        if manifest.enabled != enabled {
            manifest.enabled = enabled;
            manifest.updated_at = (self.clock)();
            self.registry.replace_manifest(&manifest)?;
        }
        Ok(ServiceResult::clean(TrackerData {
            tracker: (&manifest).into(),
        }))
    }

    pub fn remove(&self, id: &TrackerId) -> Result<ServiceResult<RemovedData>, TrackerError> {
        let _lock = WriterLock::acquire(&self.registry.paths().data_root)?;
        if !self.registry.remove(id)? {
            return Err(
                TrackerError::new("tracker_not_found", "tracker does not exist")
                    .tracker(id.to_string()),
            );
        }
        Ok(ServiceResult::clean(RemovedData {
            removed: Some(RemovedTracker { id: id.clone() }),
        }))
    }

    pub async fn usage(
        &self,
        filters: &[TrackerId],
        options: UsageOptions,
    ) -> Result<UsageResult, TrackerError> {
        let mut discovery = self.registry.discover()?;
        let selected = if filters.is_empty() {
            let mut selected = Vec::new();
            for tracker in discovery.trackers {
                if self.providers.contains_key(&tracker.provider) {
                    if tracker.enabled {
                        selected.push(tracker);
                    }
                } else {
                    discovery.warnings.push(
                        TrackerError::new("unknown_provider", "tracker provider is not available")
                            .tracker(tracker.id.to_string())
                            .provider(tracker.provider.to_string()),
                    );
                }
            }
            selected
        } else {
            let requested: BTreeSet<_> = filters.iter().cloned().collect();
            let mut selected = Vec::with_capacity(requested.len());
            for id in requested {
                let tracker = self.registry.load_manifest(&id)?;
                self.ensure_provider(&tracker)?;
                if !tracker.enabled {
                    return Err(TrackerError::new("tracker_disabled", "tracker is disabled")
                        .tracker(id.to_string())
                        .provider(tracker.provider.to_string()));
                }
                selected.push(tracker);
            }
            discovery.warnings.clear();
            selected
        };

        let jobs = selected.into_iter().map(|tracker| async move {
            let id = tracker.id.clone();
            let provider_id = tracker.provider.clone();
            let result = async {
                let provider = self.ensure_provider(&tracker)?;
                let credentials = self.registry.load_secrets(&id)?;
                let context = TrackerContext {
                    tracker: tracker.clone(),
                    data_dir: self.registry.paths().tracker_data(&id),
                    cache_dir: self.registry.paths().tracker_cache(&id),
                    credentials,
                    http: self.http.clone(),
                };
                let report = provider.collect(&context, options.cache).await?;
                validate_report(&report)?;
                let mut report = TrackerReport::from_report(&tracker, report);
                if !options.details {
                    report
                        .metrics
                        .retain(|metric| metric.tier == MetricTier::Primary);
                }
                Ok(report)
            };
            let result = match tokio::time::timeout(self.config.tracker_timeout, result).await {
                Ok(result) => result,
                Err(_) => Err(TrackerError::new("timeout", "tracker request timed out")),
            };
            result.map_err(|error| {
                error
                    .tracker(id.to_string())
                    .provider(provider_id.to_string())
            })
        });

        let outcomes = stream::iter(jobs)
            .buffer_unordered(self.config.concurrency.max(1))
            .collect::<Vec<_>>()
            .await;
        let mut reports = Vec::new();
        let mut errors = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(report) => reports.push(report),
                Err(error) => errors.push(error),
            }
        }
        reports.sort_by(|a, b| a.id.cmp(&b.id));
        errors.sort_by(|a, b| a.tracker_id.cmp(&b.tracker_id));
        sort_warnings(&mut discovery.warnings);
        Ok(UsageResult {
            data: UsageData { trackers: reports },
            warnings: discovery.warnings,
            errors,
        })
    }

    fn ensure_provider(
        &self,
        tracker: &TrackerManifest,
    ) -> Result<&Arc<dyn TrackerProvider>, TrackerError> {
        self.providers.get(&tracker.provider).ok_or_else(|| {
            TrackerError::new("unknown_provider", "tracker provider is not available")
                .tracker(tracker.id.to_string())
                .provider(tracker.provider.to_string())
        })
    }
}

fn sort_warnings(warnings: &mut [TrackerError]) {
    warnings.sort_by(|a, b| {
        a.tracker_id.cmp(&b.tracker_id).then_with(|| {
            a.details
                .get("entry")
                .map(Value::to_string)
                .cmp(&b.details.get("entry").map(Value::to_string))
        })
    });
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::Map;

    use crate::{
        MetricKind, MetricTier, PreparedSetup, Secret, SetupField, SetupFieldKind, SetupSchema,
        UsageMetric, UsageReport,
    };

    use super::*;

    #[derive(Default)]
    struct FakeProvider {
        policies: std::sync::Mutex<Vec<CachePolicy>>,
    }

    #[async_trait]
    impl TrackerProvider for FakeProvider {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: "fake".parse().unwrap(),
                name: "Fake".into(),
                description: "Test provider".into(),
                setup: SetupSchema {
                    fields: vec![SetupField {
                        key: "token".into(),
                        label: "Token".into(),
                        description: "A token".into(),
                        kind: SetupFieldKind::Secret,
                        required: true,
                        allowed_values: None,
                    }],
                },
                metrics: vec![],
            }
        }

        async fn validate_setup(
            &self,
            _ctx: &SetupContext,
            mut input: SetupInput,
        ) -> Result<crate::PreparedSetup, TrackerError> {
            let token = input.remove("token").unwrap().as_str().unwrap().to_owned();
            let mut secrets = crate::SecretMap::new();
            secrets.insert("token".into(), Secret::new(token));
            Ok(PreparedSetup {
                public_settings: Map::new(),
                secrets,
            })
        }

        async fn collect(
            &self,
            ctx: &TrackerContext,
            policy: CachePolicy,
        ) -> Result<UsageReport, TrackerError> {
            self.policies.lock().unwrap().push(policy);
            if ctx.credentials["token"].expose() == "bad" {
                return Err(TrackerError::new("authentication_failed", "token rejected"));
            }
            Ok(UsageReport {
                observed_at: Utc::now(),
                identity: None,
                metrics: vec![
                    UsageMetric {
                        id: "requests".into(),
                        label: "Requests".into(),
                        kind: MetricKind::Counter,
                        tier: MetricTier::Primary,
                        unit: "request".into(),
                        used: None,
                        remaining: None,
                        limit: None,
                        value: Some(1.0),
                        period: None,
                        resets_at: None,
                        attributes: Map::new(),
                    },
                    UsageMetric {
                        id: "tokens".into(),
                        label: "Tokens".into(),
                        kind: MetricKind::Counter,
                        tier: MetricTier::Detail,
                        unit: "token".into(),
                        used: None,
                        remaining: None,
                        limit: None,
                        value: Some(10.0),
                        period: None,
                        resets_at: None,
                        attributes: Map::new(),
                    },
                ],
                attributes: Map::new(),
            })
        }
    }

    fn input(token: &str) -> SetupInput {
        let mut input = Map::new();
        input.insert("token".into(), Value::String(token.into()));
        input
    }

    #[tokio::test]
    async fn usage_keeps_success_when_another_instance_fails() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(crate::AppPaths::isolated(temp.path()));
        let mut app = App::new(registry).unwrap();
        app.register_provider(Arc::new(FakeProvider::default()));
        for (id, token) in [("personal", "good"), ("work", "bad")] {
            app.add(AddRequest {
                id: id.parse().unwrap(),
                provider: "fake".parse().unwrap(),
                name: None,
                description: None,
                input: input(token),
            })
            .await
            .unwrap();
        }

        let result = app.usage(&[], UsageOptions::default()).await.unwrap();
        assert_eq!(result.data.trackers.len(), 1);
        assert_eq!(result.data.trackers[0].id.as_str(), "personal");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].tracker_id.as_deref(), Some("work"));
    }

    #[tokio::test]
    async fn setup_required_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(crate::AppPaths::isolated(temp.path()));
        let mut app = App::new(registry.clone()).unwrap();
        app.register_provider(Arc::new(FakeProvider::default()));
        let error = app
            .add(AddRequest {
                id: "work".parse().unwrap(),
                provider: "fake".parse().unwrap(),
                name: None,
                description: None,
                input: Map::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "setup_required");
        assert!(
            !registry
                .paths()
                .tracker_data(&"work".parse().unwrap())
                .exists()
        );
    }

    #[tokio::test]
    async fn reports_are_sorted_independently_of_registration_order() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(crate::AppPaths::isolated(temp.path()));
        let mut app = App::new(registry).unwrap();
        app.register_provider(Arc::new(FakeProvider::default()));
        for id in ["zulu", "alpha"] {
            app.add(AddRequest {
                id: id.parse().unwrap(),
                provider: "fake".parse().unwrap(),
                name: None,
                description: None,
                input: input("good"),
            })
            .await
            .unwrap();
        }

        let reports = app
            .usage(&[], UsageOptions::default())
            .await
            .unwrap()
            .data
            .trackers;
        assert_eq!(reports[0].id.as_str(), "alpha");
        assert_eq!(reports[1].id.as_str(), "zulu");
    }

    #[tokio::test]
    async fn usage_returns_only_primary_metrics_unless_details_are_requested() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(crate::AppPaths::isolated(temp.path()));
        let mut app = App::new(registry).unwrap();
        app.register_provider(Arc::new(FakeProvider::default()));
        app.add(AddRequest {
            id: "work".parse().unwrap(),
            provider: "fake".parse().unwrap(),
            name: None,
            description: None,
            input: input("good"),
        })
        .await
        .unwrap();

        let summary = app.usage(&[], UsageOptions::default()).await.unwrap();
        assert_eq!(summary.data.trackers[0].metrics.len(), 1);
        assert_eq!(summary.data.trackers[0].metrics[0].id, "requests");

        let details = app
            .usage(
                &[],
                UsageOptions {
                    details: true,
                    ..UsageOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(details.data.trackers[0].metrics.len(), 2);
        assert_eq!(details.data.trackers[0].metrics[1].id, "tokens");
    }

    #[tokio::test]
    async fn usage_passes_the_cache_policy_to_providers() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(crate::AppPaths::isolated(temp.path()));
        let mut app = App::new(registry).unwrap();
        let provider = Arc::new(FakeProvider::default());
        app.register_provider(provider.clone());
        app.add(AddRequest {
            id: "work".parse().unwrap(),
            provider: "fake".parse().unwrap(),
            name: None,
            description: None,
            input: input("good"),
        })
        .await
        .unwrap();

        app.usage(&[], UsageOptions::default()).await.unwrap();
        app.usage(
            &[],
            UsageOptions {
                cache: CachePolicy::Refresh,
                ..UsageOptions::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            *provider.policies.lock().unwrap(),
            vec![CachePolicy::Cached, CachePolicy::Refresh]
        );
    }
}
