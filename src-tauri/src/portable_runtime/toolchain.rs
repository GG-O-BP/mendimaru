use super::archive;
use super::store::{create_private_file, read_json, write_json, RuntimeLayout};
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TOOLCHAIN_MARKER: &str = "toolchain.json";
const OVERRIDE_VERSION_MARKER: &str = "mendimaru-mxbuild-version";
const TOOLCHAIN_SOURCE_BASIS: &str = "Mendix MxBuild and Portable Runtime documentation, 2026-06";

#[derive(Debug, Clone)]
pub(super) struct Toolchain {
    pub(super) version: String,
    pub(super) mxbuild: PathBuf,
    pub(super) mx: PathBuf,
    pub(super) source_url: String,
    pub(super) archive_sha256: String,
}

#[derive(Debug, Clone)]
pub(super) struct JavaRuntime {
    pub(super) home: PathBuf,
    pub(super) executable: PathBuf,
    pub(super) major: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolchainMarker {
    version: String,
    platform: String,
    source_url: String,
    archive_sha256: String,
    mxbuild_relative_path: String,
    mx_relative_path: String,
    capability_basis: String,
}

pub(super) fn capability_basis() -> &'static str {
    TOOLCHAIN_SOURCE_BASIS
}

pub(super) fn portable_version_supported(version: &str) -> bool {
    let Some((major, minor, patch, _build)) = numeric_version(version) else {
        return false;
    };
    match (major, minor) {
        (10, 24) => patch >= 19,
        (11, 6) => patch >= 5,
        (11, 9..) => true,
        _ => false,
    }
}

pub(super) async fn ensure(
    layout: &RuntimeLayout,
    version: &str,
    project_path: &Path,
) -> Result<Toolchain, String> {
    validate_exact_version(version)?;
    if !portable_version_supported(version) {
        return Err(
            "the exact project version is outside the documented Portable Runtime support policy"
                .to_string(),
        );
    }
    if let Some(override_root) = std::env::var_os("MENDIMARU_MXBUILD_HOME") {
        let toolchain = overridden_toolchain(version, Path::new(&override_root))?;
        verify_project_version(&toolchain, project_path).await?;
        return Ok(toolchain);
    }
    let platform = toolchain_platform()?;
    let directory = layout.toolchain_directory(platform, version)?;
    let lock_path = directory.join("toolchain.lock");
    let lock_file = create_private_file(&lock_path, false)?;
    acquire_lock(&lock_file).await?;

    if let Ok(toolchain) = cached_toolchain(&directory, platform, version) {
        verify_project_version(&toolchain, project_path).await?;
        return Ok(toolchain);
    }

    clean_staging_directories(&directory)?;
    let source_url = source_url(platform, version)?;
    let archive_name = source_url
        .rsplit('/')
        .next()
        .ok_or_else(|| "the MxBuild download URL is invalid".to_string())?;
    let archive_path = layout.downloads().join(archive_name);
    ensure_archive(&source_url, &archive_path).await?;
    let archive_sha256 = sha256_file_async(archive_path.clone()).await?;
    let staging = directory.join(format!("root.staging-{}", random_suffix()?));
    let archive_for_extract = archive_path.clone();
    let staging_for_extract = staging.clone();
    tokio::task::spawn_blocking(move || {
        archive::extract_toolchain(&archive_for_extract, &staging_for_extract)
    })
    .await
    .map_err(|_| "the MxBuild extraction worker failed".to_string())??;
    let (mxbuild, mx) = find_tools(&staging)?;
    let root = directory.join("root");
    if root.exists() {
        ensure_direct_directory(&root)?;
        fs::remove_dir_all(&root).map_err(|error| {
            format!("could not replace an incomplete MxBuild toolchain: {error}")
        })?;
    }
    fs::rename(&staging, &root)
        .map_err(|error| format!("could not publish the MxBuild toolchain: {error}"))?;
    let marker = ToolchainMarker {
        version: version.to_string(),
        platform: platform.to_string(),
        source_url: source_url.clone(),
        archive_sha256: archive_sha256.clone(),
        mxbuild_relative_path: mxbuild
            .strip_prefix(&staging)
            .map_err(|_| "the MxBuild executable escaped the toolchain".to_string())?
            .to_string_lossy()
            .to_string(),
        mx_relative_path: mx
            .strip_prefix(&staging)
            .map_err(|_| "the mx executable escaped the toolchain".to_string())?
            .to_string_lossy()
            .to_string(),
        capability_basis: TOOLCHAIN_SOURCE_BASIS.to_string(),
    };
    write_json(&directory.join(TOOLCHAIN_MARKER), &marker)?;
    let toolchain = cached_toolchain(&directory, platform, version)?;
    verify_project_version(&toolchain, project_path).await?;
    Ok(toolchain)
}

pub(super) async fn resolve_java(
    toolchain: &Toolchain,
    project_path: &Path,
) -> Result<JavaRuntime, String> {
    let required_major = project_java_major(toolchain, project_path)
        .await
        .unwrap_or_else(|| {
            if version_major(&toolchain.version) >= 11 {
                21
            } else {
                17
            }
        });
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("MENDIMARU_JAVA_HOME") {
        candidates.push(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(value));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from(format!(
            "/usr/lib/jvm/java-{required_major}-openjdk"
        )));
        candidates.push(PathBuf::from(format!(
            "/usr/lib/jvm/java-{required_major}-openjdk-amd64"
        )));
    }
    if let Some(java) = executable_on_path(java_executable_name()) {
        if let Some(home) = java.parent().and_then(Path::parent) {
            candidates.push(home.to_path_buf());
        }
    }
    candidates.sort();
    candidates.dedup();
    for candidate in candidates {
        let Ok(home) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !home.is_dir() {
            continue;
        }
        let executable = home.join("bin").join(java_executable_name());
        if !is_direct_file(&executable) {
            continue;
        }
        if java_major(&executable).await == Some(required_major) {
            return Ok(JavaRuntime {
                home,
                executable,
                major: required_major,
            });
        }
    }
    Err(format!(
        "Java {required_major} is required by the exact Mendix project version"
    ))
}

async fn ensure_archive(url: &str, path: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .user_agent("mendimaru-portable-runtime/0.1")
        .build()
        .map_err(|error| format!("could not initialize the MxBuild downloader: {error}"))?;
    let head = client
        .head(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("could not inspect the official MxBuild archive: {error}"))?;
    let expected = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|length| *length > 0 && *length <= MAX_DOWNLOAD_BYTES)
        .ok_or_else(|| {
            "the official MxBuild archive length is unavailable or unsafe".to_string()
        })?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("the MxBuild download target is not a direct file".to_string());
        }
        if metadata.len() == expected {
            return Ok(());
        }
        if metadata.len() > expected {
            return Err("the cached MxBuild archive has an invalid length".to_string());
        }
    }
    let existing = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut request = client.get(url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("could not download the official MxBuild archive: {error}"))?;
    let append = existing > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut output = options
        .open(path)
        .await
        .map_err(|error| format!("could not open the MxBuild download target: {error}"))?;
    set_private_file_permissions(path)?;
    let mut received = if append { existing } else { 0 };
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| format!("the MxBuild archive download was interrupted: {error}"))?;
        received = received
            .checked_add(chunk.len() as u64)
            .filter(|length| *length <= expected && *length <= MAX_DOWNLOAD_BYTES)
            .ok_or_else(|| "the MxBuild archive exceeded its declared length".to_string())?;
        output
            .write_all(&chunk)
            .await
            .map_err(|error| format!("could not persist the MxBuild archive: {error}"))?;
    }
    output
        .sync_all()
        .await
        .map_err(|error| format!("could not persist the MxBuild archive: {error}"))?;
    if received != expected {
        return Err("the MxBuild archive download is incomplete".to_string());
    }
    Ok(())
}

fn cached_toolchain(directory: &Path, platform: &str, version: &str) -> Result<Toolchain, String> {
    let marker: ToolchainMarker = read_json(&directory.join(TOOLCHAIN_MARKER))?;
    if marker.version != version
        || marker.platform != platform
        || marker.capability_basis != TOOLCHAIN_SOURCE_BASIS
        || marker.archive_sha256.len() != 64
        || !marker
            .archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("the cached MxBuild marker does not match the request".to_string());
    }
    let root = directory.join("root");
    ensure_direct_directory(&root)?;
    let mxbuild = checked_tool_path(&root, &marker.mxbuild_relative_path)?;
    let mx = checked_tool_path(&root, &marker.mx_relative_path)?;
    Ok(Toolchain {
        version: marker.version,
        mxbuild,
        mx,
        source_url: marker.source_url,
        archive_sha256: marker.archive_sha256,
    })
}

fn overridden_toolchain(version: &str, root: &Path) -> Result<Toolchain, String> {
    if !root.is_absolute() {
        return Err("MENDIMARU_MXBUILD_HOME must be absolute".to_string());
    }
    let root = fs::canonicalize(root)
        .map_err(|error| format!("could not resolve MENDIMARU_MXBUILD_HOME: {error}"))?;
    ensure_direct_directory(&root)?;
    let marker_version = fs::read_to_string(root.join(OVERRIDE_VERSION_MARKER))
        .ok()
        .map(|value| value.trim().to_string());
    let basename_matches = root.file_name().and_then(|name| name.to_str()) == Some(version);
    if marker_version.as_deref() != Some(version) && !basename_matches {
        return Err(format!(
            "MENDIMARU_MXBUILD_HOME must be named {version} or contain an exact {OVERRIDE_VERSION_MARKER} marker"
        ));
    }
    let (mxbuild, mx) = find_tools(&root)?;
    Ok(Toolchain {
        version: version.to_string(),
        mxbuild,
        mx,
        source_url: "local-exact-version-override".to_string(),
        archive_sha256: sha256_tree(&root)?,
    })
}

async fn verify_project_version(toolchain: &Toolchain, project_path: &Path) -> Result<(), String> {
    if !is_direct_file(project_path) {
        return Err("the Mendix project is not a direct regular file".to_string());
    }
    let output = tokio::process::Command::new(&toolchain.mx)
        .arg("show-version")
        .arg(project_path)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("could not run mx show-version: {error}"))?;
    if !output.status.success() || output.stdout.len() + output.stderr.len() > 1024 * 1024 {
        return Err("mx show-version rejected the project or returned unsafe output".to_string());
    }
    let observed = String::from_utf8_lossy(&output.stdout);
    let version_pattern = Regex::new(r"\d+\.\d+\.\d+(?:\.\d+)?(?:-(?:beta|rc)(?:\.?\d+)?)?")
        .map_err(|error| error.to_string())?;
    let matches = version_pattern
        .find_iter(&observed)
        .map(|value| normalize_reported_version(value.as_str(), &toolchain.version))
        .collect::<Vec<_>>();
    if !matches.iter().any(|value| value == &toolchain.version) {
        return Err(
            "the mx tool and project do not report the requested exact version".to_string(),
        );
    }
    Ok(())
}

async fn project_java_major(toolchain: &Toolchain, project_path: &Path) -> Option<u32> {
    let output = tokio::process::Command::new(&toolchain.mx)
        .arg("show-java-version")
        .arg(project_path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout);
    Regex::new(r"\b(?:Java\s*)?(17|21)\b")
        .ok()?
        .captures(&value)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

async fn java_major(executable: &Path) -> Option<u32> {
    let output = tokio::process::Command::new(executable)
        .arg("-version")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() || output.stderr.len() + output.stdout.len() > 1024 * 1024 {
        return None;
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Regex::new(r#"version\s+\"(\d+)"#)
        .ok()?
        .captures(&combined)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn find_tools(root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let mxbuild_name = if cfg!(windows) {
        "mxbuild.exe"
    } else {
        "mxbuild"
    };
    let mx_name = if cfg!(windows) { "mx.exe" } else { "mx" };
    let mut mxbuild = None;
    let mut mx = None;
    for entry in WalkDir::new(root)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name == mxbuild_name {
            mxbuild = Some(entry.path().to_path_buf());
        } else if name == mx_name {
            mx = Some(entry.path().to_path_buf());
        }
        if mxbuild.is_some() && mx.is_some() {
            break;
        }
    }
    match (mxbuild, mx) {
        (Some(mxbuild), Some(mx)) => Ok((mxbuild, mx)),
        _ => Err("the MxBuild archive does not contain both mxbuild and mx".to_string()),
    }
}

fn checked_tool_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = root.join(relative);
    let canonical = fs::canonicalize(&path)
        .map_err(|error| format!("could not resolve a cached MxBuild executable: {error}"))?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("could not resolve the cached MxBuild root: {error}"))?;
    if !canonical.starts_with(&canonical_root) || !is_direct_file(&canonical) {
        return Err("a cached MxBuild executable escaped its toolchain".to_string());
    }
    Ok(canonical)
}

fn clean_staging_directories(directory: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("could not inspect the MxBuild toolchain directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("could not inspect MxBuild staging: {error}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("root.staging-") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect MxBuild staging: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("an MxBuild staging path is not a direct directory".to_string());
        }
        fs::remove_dir_all(entry.path())
            .map_err(|error| format!("could not clean interrupted MxBuild staging: {error}"))?;
    }
    Ok(())
}

async fn acquire_lock(file: &File) -> Result<(), String> {
    use fs2::FileExt;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => return Err(format!("could not lock the MxBuild toolchain: {error}")),
        }
    }
}

async fn sha256_file_async(path: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|_| "the MxBuild digest worker failed".to_string())?
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not open a file for hashing: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash a file: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_tree(root: &Path) -> Result<String, String> {
    let mut paths = WalkDir::new(root)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    paths.sort();
    let mut digest = Sha256::new();
    for path in paths {
        digest.update(
            path.strip_prefix(root)
                .map_err(|_| "a toolchain file escaped its root".to_string())?
                .to_string_lossy()
                .as_bytes(),
        );
        digest.update(sha256_file(&path)?.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn source_url(platform: &str, version: &str) -> Result<String, String> {
    if version_major(version) < 11
        && numeric_version(version).is_some_and(|value| value.3.is_none())
    {
        return Err("Mendix versions below 11.5 require their full build number for an exact MxBuild download".to_string());
    }
    let prefix = match platform {
        "linux-x86_64" => "mxbuild",
        "windows-x86_64" => "win-mxbuild",
        _ => return Err("the host has no official MxBuild download mapping".to_string()),
    };
    Ok(format!(
        "https://cdn.mendix.com/runtime/{prefix}-{version}.tar.gz"
    ))
}

fn toolchain_platform() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        _ => Err("the host architecture has no supported exact MxBuild package".to_string()),
    }
}

fn validate_exact_version(version: &str) -> Result<(), String> {
    crate::platform::validate_version(version)?;
    if numeric_version(version).is_none() {
        return Err("the exact Mendix version is invalid".to_string());
    }
    Ok(())
}

fn numeric_version(version: &str) -> Option<(u32, u32, u32, Option<u32>)> {
    let release = version.split('-').next()?;
    let parts = release
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    Some((parts[0], parts[1], parts[2], parts.get(3).copied()))
}

fn version_major(version: &str) -> u32 {
    numeric_version(version).map(|value| value.0).unwrap_or(0)
}

fn normalize_reported_version(observed: &str, requested: &str) -> String {
    if observed == format!("{requested}.0") {
        requested.to_string()
    } else {
        observed.to_string()
    }
}

fn java_executable_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(if cfg!(windows) { ';' } else { ':' })
        .map(Path::new)
        .map(|directory| directory.join(name))
        .find(|path| is_direct_file(path))
        .and_then(|path| fs::canonicalize(path).ok())
}

fn is_direct_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn ensure_direct_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect the toolchain directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the toolchain directory is not direct".to_string());
    }
    Ok(())
}

fn random_suffix() -> Result<String, String> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not generate an MxBuild staging nonce: {error}"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not protect the MxBuild archive: {error}"))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_support_policy_is_explicit_and_does_not_round_versions() {
        assert!(!portable_version_supported("10.24.18.999"));
        assert!(portable_version_supported("10.24.19.123"));
        assert!(portable_version_supported("11.6.5"));
        assert!(!portable_version_supported("11.7.0"));
        assert!(!portable_version_supported("11.8.9"));
        assert!(portable_version_supported("11.9.0"));
        assert!(portable_version_supported("11.12.2"));
        assert!(!portable_version_supported("12.0.0"));
        assert!(!portable_version_supported("11.12"));
    }

    #[test]
    fn official_urls_are_host_specific_and_exact() {
        assert_eq!(
            source_url("linux-x86_64", "11.12.2").expect("Linux URL"),
            "https://cdn.mendix.com/runtime/mxbuild-11.12.2.tar.gz"
        );
        assert_eq!(
            source_url("windows-x86_64", "11.12.2").expect("Windows URL"),
            "https://cdn.mendix.com/runtime/win-mxbuild-11.12.2.tar.gz"
        );
        assert!(source_url("linux-x86_64", "10.24.19").is_err());
    }

    #[test]
    #[ignore = "downloads, extracts, and executes the real exact-version MxBuild toolchain"]
    fn live_e2e_resolves_the_exact_official_toolchain() {
        let project = std::env::var_os("MENDIMARU_E2E_PROJECT")
            .map(PathBuf::from)
            .expect("MENDIMARU_E2E_PROJECT must name a disposable exact-version fixture");
        let layout = RuntimeLayout::discover().expect("runtime layout");
        let toolchain = tauri::async_runtime::block_on(ensure(&layout, "11.12.2", &project))
            .expect("exact official toolchain");
        assert_eq!(toolchain.version, "11.12.2");
        assert!(toolchain.mxbuild.is_file());
        assert!(toolchain.mx.is_file());
    }
}
