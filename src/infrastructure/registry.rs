use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{Secret, SecretMap, TrackerError, TrackerId, TrackerManifest};

const MANIFEST_FILE: &str = "tracker.json";
const CREDENTIALS_FILE: &str = "credentials.json";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_root: PathBuf,
    pub cache_root: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self, TrackerError> {
        let dirs = ProjectDirs::from("", "", "yaait")
            .ok_or_else(|| TrackerError::storage("could not resolve platform directories"))?;
        Ok(Self {
            data_root: dirs.data_dir().to_path_buf(),
            cache_root: dirs.cache_dir().to_path_buf(),
        })
    }

    pub fn isolated(root: impl AsRef<Path>) -> Self {
        Self {
            data_root: root.as_ref().join("data"),
            cache_root: root.as_ref().join("cache"),
        }
    }

    pub fn trackers_root(&self) -> PathBuf {
        self.data_root.join("trackers")
    }

    pub fn tracker_data(&self, id: &TrackerId) -> PathBuf {
        self.trackers_root().join(id.as_str())
    }

    pub fn tracker_cache(&self, id: &TrackerId) -> PathBuf {
        self.cache_root.join("trackers").join(id.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct FileRegistry {
    paths: AppPaths,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub trackers: Vec<TrackerManifest>,
    pub warnings: Vec<TrackerError>,
}

#[derive(Serialize, Deserialize)]
struct CredentialDocument {
    schema_version: u32,
    secrets: BTreeMap<String, String>,
}

impl FileRegistry {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn initialize(&self) -> Result<(), TrackerError> {
        for path in [
            self.paths.data_root.clone(),
            self.paths.trackers_root(),
            self.paths.cache_root.clone(),
            self.paths.cache_root.join("trackers"),
        ] {
            reject_symlink(&path)?;
            fs::create_dir_all(&path)
                .map_err(|_| TrackerError::storage("could not create application directory"))?;
            set_private_dir(&path)?;
        }
        Ok(())
    }

    pub fn discover(&self) -> Result<Discovery, TrackerError> {
        let root = self.paths.trackers_root();
        if !root.exists() {
            return Ok(Discovery::default());
        }
        reject_symlink(&root)?;
        let mut entries = fs::read_dir(&root)
            .map_err(|_| TrackerError::storage("could not scan tracker registry"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| TrackerError::storage("could not scan tracker registry"))?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut discovery = Discovery::default();
        for entry in entries {
            let display_name = entry.file_name().to_string_lossy().into_owned();
            if display_name.starts_with(".tmp-") {
                continue;
            }
            let id = match display_name.parse::<TrackerId>() {
                Ok(id) => id,
                Err(_) => {
                    discovery.warnings.push(
                        TrackerError::new(
                            "invalid_manifest",
                            "unsafe tracker directory was skipped",
                        )
                        .detail("entry", display_name),
                    );
                    continue;
                }
            };
            match self.load_manifest(&id) {
                Ok(manifest) => discovery.trackers.push(manifest),
                Err(error) => discovery.warnings.push(error.tracker(id.to_string())),
            }
        }
        discovery.trackers.sort_by(|a, b| a.id.cmp(&b.id));
        discovery.warnings.sort_by(|a, b| {
            a.tracker_id.cmp(&b.tracker_id).then_with(|| {
                a.details
                    .get("entry")
                    .map(ToString::to_string)
                    .cmp(&b.details.get("entry").map(ToString::to_string))
            })
        });
        Ok(discovery)
    }

    pub fn load_manifest(&self, id: &TrackerId) -> Result<TrackerManifest, TrackerError> {
        let dir = self.paths.tracker_data(id);
        ensure_real_dir(&dir).map_err(|error| error.tracker(id.to_string()))?;
        ensure_private_dir(&dir).map_err(|error| error.tracker(id.to_string()))?;
        let state = dir.join("state");
        if state.exists() {
            ensure_real_dir(&state).map_err(|error| error.tracker(id.to_string()))?;
            ensure_private_dir(&state).map_err(|error| error.tracker(id.to_string()))?;
        }
        let cache = self.paths.tracker_cache(id);
        if cache.exists() {
            ensure_real_dir(&cache).map_err(|error| error.tracker(id.to_string()))?;
            ensure_private_dir(&cache).map_err(|error| error.tracker(id.to_string()))?;
        }
        let path = dir.join(MANIFEST_FILE);
        ensure_real_file(&path).map_err(|error| error.tracker(id.to_string()))?;
        let bytes = fs::read(&path).map_err(|_| {
            TrackerError::new("invalid_manifest", "could not read tracker manifest")
                .tracker(id.to_string())
        })?;
        let manifest: TrackerManifest = serde_json::from_slice(&bytes).map_err(|_| {
            TrackerError::new("invalid_manifest", "tracker manifest is not valid JSON")
                .tracker(id.to_string())
        })?;
        if manifest.schema_version != 1 {
            return Err(TrackerError::new(
                "unsupported_schema",
                "tracker manifest schema is not supported",
            )
            .tracker(id.to_string())
            .detail("schema_version", manifest.schema_version));
        }
        if &manifest.id != id {
            return Err(TrackerError::new(
                "invalid_manifest",
                "tracker directory and manifest IDs differ",
            )
            .tracker(id.to_string()));
        }
        Ok(manifest)
    }

    pub fn load_secrets(&self, id: &TrackerId) -> Result<SecretMap, TrackerError> {
        let path = self.paths.tracker_data(id).join(CREDENTIALS_FILE);
        ensure_real_file(&path).map_err(|error| error.tracker(id.to_string()))?;
        ensure_private_file(&path).map_err(|error| error.tracker(id.to_string()))?;
        let bytes = fs::read(path).map_err(|_| {
            TrackerError::storage("could not read tracker credentials").tracker(id.to_string())
        })?;
        let document: CredentialDocument = serde_json::from_slice(&bytes).map_err(|_| {
            TrackerError::storage("tracker credentials are invalid").tracker(id.to_string())
        })?;
        if document.schema_version != 1 {
            return Err(TrackerError::new(
                "unsupported_schema",
                "credential schema is not supported",
            )
            .tracker(id.to_string()));
        }
        Ok(document
            .secrets
            .into_iter()
            .map(|(key, value)| (key, Secret::new(value)))
            .collect())
    }

    pub fn add(&self, manifest: &TrackerManifest, secrets: &SecretMap) -> Result<(), TrackerError> {
        self.initialize()?;
        let final_path = self.paths.tracker_data(&manifest.id);
        if fs::symlink_metadata(&final_path).is_ok() {
            return Err(
                TrackerError::new("tracker_exists", "tracker ID already exists")
                    .tracker(manifest.id.to_string()),
            );
        }
        let temp = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempdir_in(self.paths.trackers_root())
            .map_err(|_| TrackerError::storage("could not create temporary tracker directory"))?;
        set_private_dir(temp.path())?;
        let state = temp.path().join("state");
        fs::create_dir(&state)
            .map_err(|_| TrackerError::storage("could not create tracker state directory"))?;
        set_private_dir(&state)?;
        write_new_json(&temp.path().join(MANIFEST_FILE), manifest)?;
        write_new_json(
            &temp.path().join(CREDENTIALS_FILE),
            &credential_document(secrets),
        )?;
        fs::rename(temp.path(), &final_path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
            ) {
                TrackerError::new("tracker_exists", "tracker ID already exists")
                    .tracker(manifest.id.to_string())
            } else {
                TrackerError::storage("could not register tracker")
            }
        })?;
        let cache = self.paths.tracker_cache(&manifest.id);
        if fs::create_dir_all(&cache).is_ok() {
            let _ = set_private_dir(&cache);
        }
        Ok(())
    }

    pub fn replace_setup(
        &self,
        manifest: &TrackerManifest,
        secrets: &SecretMap,
    ) -> Result<(), TrackerError> {
        let dir = self.paths.tracker_data(&manifest.id);
        ensure_real_dir(&dir)?;
        atomic_replace_json(&dir.join(MANIFEST_FILE), manifest)?;
        atomic_replace_json(&dir.join(CREDENTIALS_FILE), &credential_document(secrets))
    }

    pub fn replace_manifest(&self, manifest: &TrackerManifest) -> Result<(), TrackerError> {
        let dir = self.paths.tracker_data(&manifest.id);
        ensure_real_dir(&dir)?;
        atomic_replace_json(&dir.join(MANIFEST_FILE), manifest)
    }

    pub fn remove(&self, id: &TrackerId) -> Result<bool, TrackerError> {
        let data = self.paths.tracker_data(id);
        let cache = self.paths.tracker_cache(id);
        let existed = fs::symlink_metadata(&data).is_ok() || fs::symlink_metadata(&cache).is_ok();
        if !existed {
            return Ok(false);
        }
        for path in [&data, &cache] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(TrackerError::new(
                        "insecure_permissions",
                        "refusing to remove a symlinked tracker path",
                    )
                    .tracker(id.to_string()));
                }
                Ok(_) => fs::remove_dir_all(path)
                    .map_err(|_| TrackerError::storage("could not remove tracker directory"))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(TrackerError::storage("could not inspect tracker directory")),
            }
        }
        Ok(true)
    }
}

fn credential_document(secrets: &SecretMap) -> CredentialDocument {
    CredentialDocument {
        schema_version: 1,
        secrets: secrets
            .iter()
            .map(|(key, value)| (key.clone(), value.expose().to_owned()))
            .collect(),
    }
}

fn reject_symlink(path: &Path) -> Result<(), TrackerError> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        Err(TrackerError::new(
            "insecure_permissions",
            "storage path must not be a symlink",
        ))
    } else {
        Ok(())
    }
}

fn ensure_real_dir(path: &Path) -> Result<(), TrackerError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TrackerError::new("tracker_not_found", "tracker does not exist")
        } else {
            TrackerError::storage("could not inspect tracker directory")
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        Err(TrackerError::new(
            "insecure_permissions",
            "tracker path is not a real directory",
        ))
    } else {
        Ok(())
    }
}

fn ensure_real_file(path: &Path) -> Result<(), TrackerError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| TrackerError::new("invalid_manifest", "required tracker file is missing"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        Err(TrackerError::new(
            "insecure_permissions",
            "tracker file must be a regular file",
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path) -> Result<(), TrackerError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|_| TrackerError::storage("could not inspect directory permissions"))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 == 0 && mode & 0o700 == 0o700 {
        Ok(())
    } else {
        Err(TrackerError::new(
            "insecure_permissions",
            "tracker directories must be owner-only",
        )
        .detail("mode", format!("{mode:04o}")))
    }
}

#[cfg(not(unix))]
fn ensure_private_dir(_path: &Path) -> Result<(), TrackerError> {
    Ok(())
}

fn write_new_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TrackerError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| TrackerError::storage("could not serialize tracker data"))?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| TrackerError::storage("could not create tracker file"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| TrackerError::storage("could not persist tracker file"))?;
    set_private_file(path)
}

fn atomic_replace_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TrackerError> {
    if path.exists() {
        ensure_real_file(path)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| TrackerError::storage("tracker file has no parent directory"))?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|_| TrackerError::storage("could not create temporary tracker file"))?;
    set_private_file(temp.path())?;
    serde_json::to_writer_pretty(temp.as_file_mut(), value)
        .map_err(|_| TrackerError::storage("could not serialize tracker data"))?;
    temp.as_file()
        .sync_all()
        .map_err(|_| TrackerError::storage("could not persist tracker file"))?;
    temp.persist(path)
        .map_err(|_| TrackerError::storage("could not replace tracker file"))?;
    set_private_file(path)
}

#[cfg(unix)]
pub(crate) fn set_private_dir(path: &Path) -> Result<(), TrackerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| TrackerError::storage("could not secure directory permissions"))
}

#[cfg(not(unix))]
pub(crate) fn set_private_dir(_path: &Path) -> Result<(), TrackerError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_private_file(path: &Path) -> Result<(), TrackerError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| TrackerError::storage("could not secure file permissions"))
}

#[cfg(not(unix))]
pub(crate) fn set_private_file(_path: &Path) -> Result<(), TrackerError> {
    Ok(())
}

#[cfg(unix)]
fn ensure_private_file(path: &Path) -> Result<(), TrackerError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|_| TrackerError::storage("could not inspect credential permissions"))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 == 0 && mode & 0o600 == 0o600 {
        Ok(())
    } else {
        Err(TrackerError::new(
            "insecure_permissions",
            "credential file must be owner read/write only",
        )
        .detail("mode", format!("{mode:04o}")))
    }
}

#[cfg(not(unix))]
fn ensure_private_file(_path: &Path) -> Result<(), TrackerError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::Utc;
    use serde_json::Map;

    use super::*;
    use crate::ProviderId;

    fn manifest(id: &str) -> TrackerManifest {
        let now = Utc::now();
        TrackerManifest {
            schema_version: 1,
            id: TrackerId::from_str(id).unwrap(),
            provider: ProviderId::from_str("github-copilot").unwrap(),
            name: id.into(),
            description: None,
            enabled: true,
            created_at: now,
            updated_at: now,
            settings: Map::new(),
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn stores_two_instances_independently_and_rejects_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(AppPaths::isolated(temp.path()));
        let mut work_secrets = SecretMap::new();
        work_secrets.insert("token".into(), Secret::new("work-token"));
        let mut personal_secrets = SecretMap::new();
        personal_secrets.insert("token".into(), Secret::new("personal-token"));

        registry.add(&manifest("work"), &work_secrets).unwrap();
        registry
            .add(&manifest("personal"), &personal_secrets)
            .unwrap();

        assert_eq!(
            registry.load_secrets(&"work".parse().unwrap()).unwrap()["token"].expose(),
            "work-token"
        );
        assert_eq!(
            registry.load_secrets(&"personal".parse().unwrap()).unwrap()["token"].expose(),
            "personal-token"
        );
        assert_eq!(
            registry
                .add(&manifest("work"), &work_secrets)
                .unwrap_err()
                .code,
            "tracker_exists"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_credentials_with_group_or_world_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new(AppPaths::isolated(temp.path()));
        let manifest = manifest("work");
        let mut secrets = SecretMap::new();
        secrets.insert("token".into(), Secret::new("secret"));
        registry.add(&manifest, &secrets).unwrap();
        let credentials = registry
            .paths()
            .tracker_data(&manifest.id)
            .join(CREDENTIALS_FILE);
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o644)).unwrap();

        let error = registry.load_secrets(&manifest.id).unwrap_err();
        assert_eq!(error.code, "insecure_permissions");
        assert_eq!(error.tracker_id.as_deref(), Some("work"));
    }
}
