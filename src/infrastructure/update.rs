//! Updates installer-managed copies from the project's release archives.
use std::{
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use directories::BaseDirs;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::WriterLock;
use crate::TrackerError;

const RELEASE_API: &str = "https://api.github.com/repos/felixscherz/yaait/releases";
const MAX_ARCHIVE: u64 = 100 * 1024 * 1024;
const MAX_BINARY: u64 = 100 * 1024 * 1024;
const INSTALL_HELP: &str = "Self-update requires a matching shell or PowerShell installer receipt. Reinstall with the installer, or update using your original installation method.";

#[derive(Debug, Serialize)]
pub struct UpdateData {
    pub current_version: String,
    pub target_version: String,
    pub installation_method: &'static str,
    pub update_available: bool,
    pub updated: bool,
    pub message: String,
}

struct Installation {
    method: &'static str,
    receipt: Option<(PathBuf, Value)>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub async fn update(
    version: Option<&str>,
    check: bool,
) -> Result<(UpdateData, Vec<TrackerError>), TrackerError> {
    let requested = version.map(parse_version).transpose()?;
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|_| failure("could not locate the running executable"))?;
    let installation = detect_installation(&exe, &receipt_paths());
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("yaait/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| failure("could not initialize release client"))?;
    perform_update(
        &client,
        RELEASE_API,
        installation,
        requested,
        check,
        |path| self_replace::self_replace(path),
    )
    .await
}

async fn perform_update(
    client: &reqwest::Client,
    api: &str,
    installation: Installation,
    requested: Option<Version>,
    check: bool,
    replace: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(UpdateData, Vec<TrackerError>), TrackerError> {
    if !check && installation.receipt.is_none() {
        return Err(
            TrackerError::new("update_unsupported", help(installation.method))
                .detail("installation_method", installation.method),
        );
    }
    // Keep simultaneous updaters from replacing a copy based on stale metadata.
    let _lock = if !check {
        let (path, _) = installation.receipt.as_ref().unwrap();
        Some(WriterLock::acquire(path.parent().unwrap())?)
    } else {
        None
    };
    let release = fetch_release(client, api, requested.as_ref()).await?;
    let current = parse_version(env!("CARGO_PKG_VERSION"))?;
    let target = parse_version(&release.tag_name)?;
    let available = if requested.is_some() {
        current != target
    } else {
        current < target
    };
    let mut data = UpdateData {
        current_version: current.to_string(),
        target_version: target.to_string(),
        installation_method: installation.method,
        update_available: available,
        updated: false,
        message: if available {
            format!("yaait {target} is available (running {current}).")
        } else {
            format!("yaait {current} is already up to date for this request.")
        },
    };
    if check {
        if installation.receipt.is_none() {
            data.message.push(' ');
            data.message.push_str(help(installation.method));
        }
        return Ok((data, Vec::new()));
    }
    if !available {
        return Ok((data, Vec::new()));
    }
    let archive_name = archive_name()?;
    let bytes = download_archive(client, &release, &archive_name).await?;
    let temp = tempfile::tempdir().map_err(|_| failure("could not stage update"))?;
    let staged = temp.path().join(binary_name());
    extract_binary(&bytes, &archive_name, &staged)?;
    // Validate before touching the installed copy. Only the checksum-verified binary runs.
    let output = std::process::Command::new(&staged)
        .arg("--version")
        .output()
        .map_err(|_| failure("could not run the downloaded binary"))?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim() != format!("yaait {target}")
    {
        return Err(failure(
            "downloaded binary did not report the requested version",
        ));
    }
    replace(&staged).map_err(|_| {
        failure("could not replace executable; check installation directory permissions")
    })?;
    data.updated = true;
    data.message = format!("Updated yaait from {current} to {target}.");
    let (path, mut receipt) = installation.receipt.unwrap();
    receipt["version"] = Value::String(target.to_string());
    let warnings = if write_receipt(&path, &receipt).is_err() {
        vec![TrackerError::new(
            "update_receipt_warning",
            "The binary was updated, but its installation receipt could not be refreshed.",
        )]
    } else {
        Vec::new()
    };
    Ok((data, warnings))
}

fn parse_version(raw: &str) -> Result<Version, TrackerError> {
    Version::parse(raw.strip_prefix('v').unwrap_or(raw)).map_err(|_| {
        TrackerError::invalid(
            "version must be a complete semantic version, such as 0.2.4 or v0.2.4",
        )
    })
}

fn failure(message: &str) -> TrackerError {
    TrackerError::new("update_failed", message)
}

fn help(method: &str) -> &'static str {
    if method == "homebrew" {
        "Homebrew manages this installation. Run: brew upgrade felixscherz/tap/yaait"
    } else {
        INSTALL_HELP
    }
}

fn receipt_paths() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("AXOUPDATER_CONFIG_PATH") {
        return vec![PathBuf::from(path).join("yaait-receipt.json")];
    }
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(path).join("yaait/yaait-receipt.json"));
    }
    if cfg!(windows) {
        if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(path).join("yaait/yaait-receipt.json"));
        }
    } else if let Some(dirs) = BaseDirs::new() {
        paths.push(dirs.home_dir().join(".config/yaait/yaait-receipt.json"));
    }
    paths
}

fn detect_installation(exe: &Path, paths: &[PathBuf]) -> Installation {
    // current_exe is canonicalized so Homebrew's bin symlinks resolve into Cellar.
    if exe.components().any(|part| part.as_os_str() == "Cellar") {
        return Installation {
            method: "homebrew",
            receipt: None,
        };
    }
    for path in paths {
        let Some(receipt) = fs::read(path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        else {
            continue;
        };
        if receipt_matches(exe, &receipt) {
            return Installation {
                method: "installer",
                receipt: Some((path.clone(), receipt)),
            };
        }
    }
    Installation {
        method: "unmanaged",
        receipt: None,
    }
}

fn receipt_matches(exe: &Path, receipt: &Value) -> bool {
    if receipt["provider"]["source"] != "cargo-dist"
        || receipt["source"]["release_type"] != "github"
        || receipt["source"]["owner"] != "felixscherz"
        || receipt["source"]["name"] != "yaait"
        || receipt["source"]["app_name"] != "yaait"
    {
        return false;
    }
    let Some(prefix) = receipt["install_prefix"].as_str() else {
        return false;
    };
    let Some(binaries) = receipt["binaries"].as_array() else {
        return false;
    };
    if !binaries
        .iter()
        .any(|name| name.as_str() == Some(binary_name()))
    {
        return false;
    }
    // cargo-dist receipts have used both the install root and the bin directory.
    [
        Path::new(prefix).join(binary_name()),
        Path::new(prefix).join("bin").join(binary_name()),
    ]
    .iter()
    .any(|candidate| fs::canonicalize(candidate).is_ok_and(|candidate| candidate == exe))
}

async fn fetch_release(
    client: &reqwest::Client,
    api: &str,
    requested: Option<&Version>,
) -> Result<Release, TrackerError> {
    let suffix =
        requested.map_or_else(|| "latest".to_owned(), |version| format!("tags/v{version}"));
    let response = client
        .get(format!("{api}/{suffix}"))
        .send()
        .await
        .map_err(|_| failure("could not fetch release information"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(failure("requested release was not found"));
    }
    let release: Release = response
        .error_for_status()
        .map_err(|_| failure("release service returned an error; try again later"))?
        .json()
        .await
        .map_err(|_| failure("invalid release information"))?;
    let version = parse_version(&release.tag_name)?;
    if release.draft
        || (requested.is_none() && (release.prerelease || !version.pre.is_empty()))
        || requested.is_some_and(|requested| requested != &version)
    {
        return Err(failure("release does not match the requested version"));
    }
    Ok(release)
}

fn binary_name() -> &'static str {
    if cfg!(windows) { "yaait.exe" } else { "yaait" }
}

fn archive_name() -> Result<String, TrackerError> {
    let target = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "linux") if cfg!(target_env = "gnu") => "aarch64-unknown-linux-gnu",
        ("x86_64", "linux") if cfg!(target_env = "gnu") => "x86_64-unknown-linux-gnu",
        ("aarch64", "windows") if cfg!(target_env = "msvc") => "aarch64-pc-windows-msvc",
        ("x86_64", "windows") if cfg!(target_env = "msvc") => "x86_64-pc-windows-msvc",
        _ => {
            return Err(TrackerError::new(
                "update_unsupported",
                "No release archive is available for this platform. Update from source.",
            ));
        }
    };
    let extension = if cfg!(windows) { "zip" } else { "tar.xz" };
    Ok(format!("yaait-{target}.{extension}"))
}

async fn download(client: &reqwest::Client, url: &str, max: u64) -> Result<Vec<u8>, TrackerError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| failure("could not download release asset"))?
        .error_for_status()
        .map_err(|_| failure("release asset download failed"))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("release asset download was interrupted"))?
    {
        if bytes.len() as u64 + chunk.len() as u64 > max {
            return Err(failure("release asset exceeds the size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn download_archive(
    client: &reqwest::Client,
    release: &Release,
    name: &str,
) -> Result<Vec<u8>, TrackerError> {
    let find = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| failure("release is missing the platform archive or checksum"))
    };
    let archive = find(name)?;
    let checksum = find(&format!("{name}.sha256"))?;
    let checksum = download(client, &checksum.browser_download_url, 4096).await?;
    let bytes = download(client, &archive.browser_download_url, MAX_ARCHIVE).await?;
    verify_checksum(&bytes, &checksum, name)?;
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], checksum: &[u8], name: &str) -> Result<(), TrackerError> {
    let checksum =
        std::str::from_utf8(checksum).map_err(|_| failure("invalid archive checksum"))?;
    let valid = checksum.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next().unwrap_or_default();
        let filename = fields.next().unwrap_or_default().trim_start_matches('*');
        filename == name
            && fields.next().is_none()
            && hash.eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(bytes)))
    });
    if !valid {
        return Err(failure("archive checksum verification failed"));
    }
    Ok(())
}

fn extract_binary(bytes: &[u8], name: &str, destination: &Path) -> Result<(), TrackerError> {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut output = fs::File::create(destination)?;
        let mut found = false;
        if name.ends_with(".zip") {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                if entry.is_file()
                    && entry
                        .enclosed_name()
                        .is_some_and(|path| is_binary_path(&path))
                {
                    if found {
                        return Err("duplicate binary".into());
                    }
                    copy_binary(&mut entry, &mut output)?;
                    found = true;
                }
            }
        } else {
            let mut archive = tar::Archive::new(xz2::read::XzDecoder::new(bytes));
            for entry in archive.entries()? {
                let mut entry = entry?;
                if entry.header().entry_type().is_file() && is_binary_path(&entry.path()?) {
                    if found {
                        return Err("duplicate binary".into());
                    }
                    copy_binary(&mut entry, &mut output)?;
                    found = true;
                }
            }
        }
        if !found {
            return Err("missing binary".into());
        }
        output.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    result.map_err(|_| {
        failure("release archive contains no valid executable or could not be extracted")
    })
}

fn is_binary_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == binary_name())
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        && path.components().count() <= 2
}

fn copy_binary(input: &mut impl Read, output: &mut fs::File) -> io::Result<()> {
    let count = io::copy(&mut input.take(MAX_BINARY + 1), output)?;
    if count == 0 || count > MAX_BINARY {
        return Err(io::Error::other("invalid binary size"));
    }
    Ok(())
}

fn write_receipt(path: &Path, receipt: &Value) -> io::Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temp.write_all(&serde_json::to_vec_pretty(receipt)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn receipt(prefix: &Path) -> Value {
        json!({
            "install_prefix": prefix,
            "binaries": [binary_name()],
            "version": "0.1.0",
            "provider": {"source": "cargo-dist", "version": "0.32.0"},
            "source": {"release_type": "github", "owner": "felixscherz", "name": "yaait", "app_name": "yaait"},
            "modify_path": false,
            "future_field": "preserve me"
        })
    }

    #[test]
    fn versions_require_semver_and_accept_a_tag_prefix() {
        assert_eq!(
            parse_version("v0.2.4").unwrap(),
            parse_version("0.2.4").unwrap()
        );
        assert!(parse_version("0.3.0-beta.1").is_ok());
        for invalid in ["latest", "0.2", "../../bad", "0.2.4;echo secret"] {
            assert_eq!(parse_version(invalid).unwrap_err().code, "invalid_input");
        }
    }

    #[test]
    fn receipts_must_identify_this_binary_and_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("bin")).unwrap();
        let exe = root.join("bin").join(binary_name());
        fs::write(&exe, b"binary").unwrap();
        let exe = fs::canonicalize(exe).unwrap();
        assert!(receipt_matches(&exe, &receipt(root)));
        assert!(receipt_matches(&exe, &receipt(&root.join("bin"))));
        assert!(!receipt_matches(&exe, &receipt(&root.join("other"))));
        let mut other_project = receipt(root);
        other_project["source"]["owner"] = json!("other");
        assert!(!receipt_matches(&exe, &other_project));
        other_project = receipt(root);
        other_project["binaries"] = json!([]);
        assert!(!receipt_matches(&exe, &other_project));
    }

    #[test]
    fn homebrew_takes_precedence_over_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("Cellar/yaait/0.2.5/bin");
        fs::create_dir_all(&prefix).unwrap();
        let exe = prefix.join(binary_name());
        fs::write(&exe, b"binary").unwrap();
        let path = temp.path().join("yaait-receipt.json");
        fs::write(&path, serde_json::to_vec(&receipt(&prefix)).unwrap()).unwrap();
        let detected = detect_installation(&fs::canonicalize(exe).unwrap(), &[path]);
        assert_eq!(detected.method, "homebrew");
        assert!(detected.receipt.is_none());
        assert!(help(detected.method).contains("brew upgrade"));
    }

    #[test]
    fn corrupt_or_stale_receipts_do_not_authorize_updates() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join(binary_name());
        fs::write(&exe, b"binary").unwrap();
        let exe = fs::canonicalize(exe).unwrap();
        let path = temp.path().join("receipt.json");
        fs::write(&path, b"not json").unwrap();
        assert!(
            detect_installation(&exe, std::slice::from_ref(&path))
                .receipt
                .is_none()
        );
        fs::write(
            &path,
            serde_json::to_vec(&receipt(&temp.path().join("old"))).unwrap(),
        )
        .unwrap();
        assert!(detect_installation(&exe, &[path]).receipt.is_none());
    }

    #[test]
    fn checksum_requires_the_matching_archive_and_digest() {
        let name = "yaait-target.tar.xz";
        let checksum = format!("{:x}  {name}\n", Sha256::digest(b"archive"));
        verify_checksum(b"archive", checksum.as_bytes(), name).unwrap();
        assert!(verify_checksum(b"tampered", checksum.as_bytes(), name).is_err());
        assert!(verify_checksum(b"archive", checksum.as_bytes(), "other.tar.xz").is_err());
        assert!(verify_checksum(b"archive", b"invalid", name).is_err());
    }

    fn tar_archive(paths: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = xz2::write::XzEncoder::new(Vec::new(), 1);
        let mut builder = tar::Builder::new(encoder);
        for (path, bytes) in paths {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extraction_only_writes_the_binary_to_staging() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        let path = format!("yaait-target/{}", binary_name());
        let bytes = tar_archive(&[
            ("yaait-target/README.md", b"ignored"),
            (&path, b"executable"),
        ]);
        extract_binary(&bytes, "archive.tar.xz", &staged).unwrap();
        assert_eq!(fs::read(&staged).unwrap(), b"executable");
        assert!(!temp.path().join("README.md").exists());
        let missing = tar_archive(&[("README.md", b"missing")]);
        assert!(extract_binary(&missing, "archive.tar.xz", &staged).is_err());
        let duplicate = tar_archive(&[(&path, b"one"), (binary_name(), b"two")]);
        assert!(extract_binary(&duplicate, "archive.tar.xz", &staged).is_err());
        assert!(!is_binary_path(Path::new("../yaait")));
    }

    #[test]
    fn zip_extraction_supports_flat_windows_archives() {
        let temp = tempfile::tempdir().unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(binary_name(), zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"executable").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let staged = temp.path().join("staged");
        extract_binary(&bytes, "archive.zip", &staged).unwrap();
        assert_eq!(fs::read(staged).unwrap(), b"executable");
    }

    #[test]
    fn receipt_refresh_preserves_unknown_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("receipt.json");
        let mut value = receipt(temp.path());
        value["version"] = json!("0.2.4");
        write_receipt(&path, &value).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["future_field"], "preserve me");
        assert_eq!(saved["modify_path"], false);
        assert_eq!(saved["version"], "0.2.4");
    }

    #[tokio::test]
    async fn release_lookup_uses_latest_stable_or_the_exact_tag() {
        let server = MockServer::start_async().await;
        let latest = server.mock(|when, then| {
            when.method(GET).path("/latest");
            then.json_body(json!({"tag_name": "v0.3.0", "assets": []}));
        });
        let pinned = server.mock(|when, then| {
            when.method(GET).path("/tags/v0.2.4");
            then.json_body(json!({"tag_name": "v0.2.4", "assets": []}));
        });
        let client = reqwest::Client::new();
        assert_eq!(
            fetch_release(&client, &server.base_url(), None)
                .await
                .unwrap()
                .tag_name,
            "v0.3.0"
        );
        assert_eq!(
            fetch_release(
                &client,
                &server.base_url(),
                Some(&parse_version("0.2.4").unwrap())
            )
            .await
            .unwrap()
            .tag_name,
            "v0.2.4"
        );
        latest.assert();
        pinned.assert();
        assert!(
            fetch_release(
                &client,
                &server.base_url(),
                Some(&parse_version("9.0.0").unwrap())
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn mismatched_and_unrequested_prereleases_are_rejected() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(GET);
            then.json_body(json!({"tag_name": "v0.3.0-beta.1", "prerelease": true, "assets": []}));
        });
        let client = reqwest::Client::new();
        assert!(
            fetch_release(&client, &server.base_url(), None)
                .await
                .is_err()
        );
        assert!(
            fetch_release(
                &client,
                &server.base_url(),
                Some(&parse_version("0.2.4").unwrap())
            )
            .await
            .is_err()
        );
        assert!(
            fetch_release(
                &client,
                &server.base_url(),
                Some(&parse_version("0.3.0-beta.1").unwrap())
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn downloads_are_verified_and_http_errors_are_failures() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(GET).path("/archive");
            then.body("archive");
        });
        server.mock(|when, then| {
            when.method(GET).path("/checksum");
            then.body(format!(
                "{:x}  archive.tar.xz\n",
                Sha256::digest(b"archive")
            ));
        });
        let release = Release {
            tag_name: "v0.3.0".into(),
            draft: false,
            prerelease: false,
            assets: vec![
                Asset {
                    name: "archive.tar.xz".into(),
                    browser_download_url: server.url("/archive"),
                },
                Asset {
                    name: "archive.tar.xz.sha256".into(),
                    browser_download_url: server.url("/checksum"),
                },
            ],
        };
        let client = reqwest::Client::new();
        assert_eq!(
            download_archive(&client, &release, "archive.tar.xz")
                .await
                .unwrap(),
            b"archive"
        );
        assert!(
            download_archive(&client, &release, "missing.tar.xz")
                .await
                .is_err()
        );
        assert!(download(&client, &server.url("/archive"), 1).await.is_err());
        assert!(
            download(&client, &server.url("/missing"), 100)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn checks_and_same_version_requests_never_replace_the_binary() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(GET);
            then.json_body(
                json!({"tag_name": format!("v{}", env!("CARGO_PKG_VERSION")), "assets": []}),
            );
        });
        let client = reqwest::Client::new();
        let (checked, _) = perform_update(
            &client,
            &server.base_url(),
            Installation {
                method: "homebrew",
                receipt: None,
            },
            None,
            true,
            |_| panic!("check attempted replacement"),
        )
        .await
        .unwrap();
        assert!(!checked.updated);
        assert!(!checked.update_available);
        assert!(checked.message.contains("brew upgrade"));
        let envelope = crate::presentation::Envelope::success("update", checked, vec![]);
        let rendered = crate::presentation::human::render(&envelope);
        assert!(rendered.stdout.contains("brew upgrade"));
        assert!(rendered.stderr.is_empty());
        assert_eq!(envelope.data["installation_method"], "homebrew");
        assert_eq!(envelope.schema_version, 2);
        let temp = tempfile::tempdir().unwrap();
        let (data, _) = perform_update(
            &client,
            &server.base_url(),
            Installation {
                method: "installer",
                receipt: Some((temp.path().join("receipt.json"), receipt(temp.path()))),
            },
            Some(parse_version(env!("CARGO_PKG_VERSION")).unwrap()),
            false,
            |_| panic!("same version attempted replacement"),
        )
        .await
        .unwrap();
        assert!(!data.updated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_downgrade_is_staged_validated_and_replaces_only_after_verification() {
        let server = MockServer::start_async().await;
        let temp = tempfile::tempdir().unwrap();
        let installed = temp.path().join("installed");
        fs::write(&installed, b"original").unwrap();
        let path = temp.path().join("receipt.json");
        let original_receipt = receipt(temp.path());
        write_receipt(&path, &original_receipt).unwrap();
        let name = archive_name().unwrap();
        let bytes = tar_archive(&[(binary_name(), b"#!/bin/sh\necho 'yaait 0.2.4'\n")]);
        let checksum = format!("{:x}  {name}\n", Sha256::digest(&bytes));
        server.mock(|when, then| {
            when.method(GET).path("/tags/v0.2.4");
            then.json_body(json!({"tag_name": "v0.2.4", "assets": [
                {"name": name, "browser_download_url": server.url("/archive")},
                {"name": format!("{name}.sha256"), "browser_download_url": server.url("/checksum")}
            ]}));
        });
        server.mock(|when, then| {
            when.method(GET).path("/archive");
            then.body(bytes);
        });
        let mut digest = server.mock(|when, then| {
            when.method(GET).path("/checksum");
            then.body(checksum);
        });
        let client = reqwest::Client::new();
        // Read the staged fixture instead of replacing the test runner executable.
        let (data, warnings) = perform_update(
            &client,
            &server.base_url(),
            Installation {
                method: "installer",
                receipt: Some((path.clone(), original_receipt.clone())),
            },
            Some(parse_version("0.2.4").unwrap()),
            false,
            |staged| {
                fs::copy(staged, &installed)?;
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(data.updated);
        assert!(warnings.is_empty());
        assert_eq!(data.target_version, "0.2.4");
        assert!(fs::read(&installed).unwrap().starts_with(b"#!/bin/sh"));
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], "0.2.4");
        assert_eq!(saved["future_field"], "preserve me");
        digest.delete();
        server.mock(|when, then| {
            when.method(GET).path("/checksum");
            then.body("invalid");
        });
        fs::write(&installed, b"original").unwrap();
        let failed = perform_update(
            &client,
            &server.base_url(),
            Installation {
                method: "installer",
                receipt: Some((path, original_receipt)),
            },
            Some(parse_version("0.2.4").unwrap()),
            false,
            |_| panic!("invalid checksum attempted replacement"),
        )
        .await;
        assert!(failed.is_err());
        assert_eq!(fs::read(installed).unwrap(), b"original");
    }
}
