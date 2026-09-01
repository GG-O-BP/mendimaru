use crate::models::{AppConfig, MendixProject, ProjectLocation, ProjectScanResult};
use chrono::{DateTime, Local};
use regex::Regex;
use serde_json::Value;
use sha2::Digest;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime};
use walkdir::{DirEntry, WalkDir};

const MAX_WORKSPACE_DEPTH: usize = 8;
const MAX_PROJECT_PATH_BYTES: usize = 4_096;
const MAX_EXTERNAL_SELECTIONS: usize = 8;
const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
const MAX_TOTAL_SETTINGS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SCAN_ENTRIES: usize = 100_000;
const MAX_PROJECTS: usize = 10_000;
const MAX_SCAN_ERRORS: usize = 32;
const MAX_SCAN_DURATION: Duration = Duration::from_secs(5);
const EXTERNAL_PROJECT_TOKEN_PREFIX: &str = "external-project_";

static EXTERNAL_SELECTIONS: OnceLock<Mutex<Vec<(String, PathBuf)>>> = OnceLock::new();
static SETTINGS_VERSION_REGEX: OnceLock<Regex> = OnceLock::new();

#[derive(Clone, Copy)]
struct ScanLimits {
    max_entries: usize,
    max_projects: usize,
    max_settings_bytes: u64,
    max_total_settings_bytes: u64,
    max_duration: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_SCAN_ENTRIES,
            max_projects: MAX_PROJECTS,
            max_settings_bytes: MAX_SETTINGS_BYTES,
            max_total_settings_bytes: MAX_TOTAL_SETTINGS_BYTES,
            max_duration: MAX_SCAN_DURATION,
        }
    }
}

#[derive(Default)]
struct ScanReport {
    skipped_entries: usize,
    error_count: usize,
    errors: Vec<String>,
    settings_bytes_read: u64,
    truncated: bool,
}

impl ScanReport {
    fn push_error(&mut self, message: String) {
        self.error_count += 1;
        if self.errors.len() < MAX_SCAN_ERRORS {
            self.errors.push(message);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectSelection {
    canonical_mpr_path: PathBuf,
    canonical_directory: PathBuf,
    location: ProjectLocation,
}

impl ProjectSelection {
    pub(crate) fn mpr_path(&self) -> &Path {
        &self.canonical_mpr_path
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.canonical_directory
    }

    pub(crate) const fn location(&self) -> ProjectLocation {
        self.location
    }

    pub(crate) fn project_digest(&self) -> String {
        format!(
            "{:x}",
            sha2::Sha256::digest(self.canonical_mpr_path.as_os_str().as_encoded_bytes())
        )
    }
}

pub fn scan_projects(config: &AppConfig) -> Result<Vec<MendixProject>, String> {
    Ok(scan_projects_with_limits(config, ScanLimits::default(), None)?.projects)
}

pub(crate) fn scan_projects_result(
    config: &AppConfig,
    cancellation: &Arc<AtomicBool>,
) -> Result<ProjectScanResult, String> {
    scan_projects_with_limits(config, ScanLimits::default(), Some(cancellation))
}

fn scan_projects_with_limits(
    config: &AppConfig,
    limits: ScanLimits,
    cancellation: Option<&AtomicBool>,
) -> Result<ProjectScanResult, String> {
    let workspace = Path::new(&config.shared_directory);
    if !workspace.is_dir() {
        return Err(crate::tr!(
            "error-workspace-not-found",
            path = &config.shared_directory
        ));
    }

    let started = Instant::now();
    let mut report = ScanReport::default();
    let mut discovered = Vec::new();
    let mut visited_entries = 0_usize;

    for result in WalkDir::new(workspace)
        .max_depth(MAX_WORKSPACE_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_visit)
    {
        if cancellation.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            report.truncated = true;
            break;
        }
        if visited_entries >= limits.max_entries {
            report.push_error(format!(
                "workspace scan stopped after {} entries",
                limits.max_entries
            ));
            report.truncated = true;
            break;
        }
        visited_entries += 1;
        if visited_entries.is_multiple_of(128) && started.elapsed() >= limits.max_duration {
            report.push_error(format!(
                "workspace scan stopped after {}ms",
                limits.max_duration.as_millis()
            ));
            report.truncated = true;
            break;
        }

        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                report.skipped_entries += 1;
                report.push_error(format!("{error}"));
                continue;
            }
        };
        if !entry.file_type().is_file() || !has_mpr_extension(entry.path()) {
            continue;
        }
        if discovered.len() >= limits.max_projects {
            report.push_error(format!(
                "workspace scan stopped after {} projects",
                limits.max_projects
            ));
            report.truncated = true;
            break;
        }

        let mpr_path = entry.path();
        let modified = match entry
            .metadata()
            .map_err(std::io::Error::from)
            .and_then(|metadata| metadata.modified())
        {
            Ok(modified) => modified,
            Err(error) => {
                report.skipped_entries += 1;
                report.push_error(format!("{mpr_path:?}: {error}"));
                continue;
            }
        };
        let windows_path = if crate::platform::is_windows_native() {
            mpr_path.to_string_lossy().to_string()
        } else {
            linux_path_to_windows_share(workspace, mpr_path, &config.windows_shared_directory)?
        };
        let (version, bytes_read) = extract_project_version_bounded(
            &mpr_path
                .parent()
                .unwrap_or(mpr_path)
                .join("project-settings.user.json"),
            limits.max_settings_bytes,
            &mut report,
        );
        report.settings_bytes_read += bytes_read;
        let project = project_with_version(
            mpr_path,
            windows_path,
            ProjectLocation::ConfiguredWorkspace,
            version,
            Some(modified),
        );
        discovered.push((modified, project));
        if report.settings_bytes_read > limits.max_total_settings_bytes {
            report.push_error(format!(
                "workspace scan stopped after reading {} settings bytes",
                limits.max_total_settings_bytes
            ));
            report.truncated = true;
            break;
        }
    }

    discovered.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    let source_key = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
        .to_string_lossy()
        .to_string();
    Ok(ProjectScanResult {
        source_key,
        projects: discovered.into_iter().map(|(_, project)| project).collect(),
        visited_entries,
        skipped_entries: report.skipped_entries,
        error_count: report.error_count,
        errors: report.errors,
        settings_bytes_read: report.settings_bytes_read,
        truncated: report.truncated,
        duration_ms: started.elapsed().as_millis() as u64,
        watcher_active: false,
    })
}

pub(crate) fn inspect_selected_project(
    config: &AppConfig,
    requested_path: &Path,
) -> Result<MendixProject, String> {
    let selection = validate_project_selection(config, requested_path)?;
    match selection.location() {
        ProjectLocation::ConfiguredWorkspace => {
            let windows_path = linux_path_to_windows_share(
                Path::new(&config.shared_directory),
                selection.mpr_path(),
                &config.windows_shared_directory,
            )?;
            Ok(project_from_path(
                selection.mpr_path(),
                windows_path,
                ProjectLocation::ConfiguredWorkspace,
            ))
        }
        ProjectLocation::ExplicitHostSelection => {
            let token = remember_external_selection(&selection)?;
            Ok(external_project_from_selection(&selection, token))
        }
    }
}

pub(crate) fn validate_project_selection(
    config: &AppConfig,
    requested_path: &Path,
) -> Result<ProjectSelection, String> {
    if !requested_path.is_absolute() {
        return Err(crate::tr!("error-project-path-absolute"));
    }
    if requested_path.as_os_str().as_encoded_bytes().len() > MAX_PROJECT_PATH_BYTES {
        return Err(crate::tr!("error-project-path-too-long"));
    }
    let requested_text = requested_path
        .to_str()
        .ok_or_else(|| crate::tr!("error-project-path-encoding"))?;
    if requested_text
        .chars()
        .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        return Err(crate::tr!("error-project-path-unsupported"));
    }
    let metadata =
        fs::symlink_metadata(requested_path).map_err(|_| crate::tr!("error-project-not-found"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || !has_mpr_extension(requested_path)
    {
        return Err(crate::tr!("error-project-not-found"));
    }
    reject_symlink_components(requested_path)?;
    let canonical_mpr_path = requested_path
        .canonicalize()
        .map_err(|_| crate::tr!("error-project-not-found"))?;
    let canonical_metadata =
        fs::metadata(&canonical_mpr_path).map_err(|_| crate::tr!("error-project-not-found"))?;
    if !canonical_metadata.is_file() || !has_mpr_extension(&canonical_mpr_path) {
        return Err(crate::tr!("error-project-not-found"));
    }
    let canonical_directory = canonical_mpr_path
        .parent()
        .filter(|directory| directory.is_dir())
        .ok_or_else(|| crate::tr!("error-project-directory-invalid"))?
        .to_path_buf();
    let canonical_workspace = Path::new(&config.shared_directory)
        .canonicalize()
        .map_err(|_| crate::tr!("error-workspace-not-found", path = &config.shared_directory))?;
    let location = if canonical_mpr_path.starts_with(&canonical_workspace) {
        ProjectLocation::ConfiguredWorkspace
    } else {
        if !cfg!(target_os = "linux") {
            return Err(crate::tr!("error-project-not-shared"));
        }
        validate_external_share_root(&canonical_directory)?;
        validate_freerdp_host_path(&canonical_directory)?;
        validate_windows_guest_file_name(&canonical_mpr_path)?;
        ProjectLocation::ExplicitHostSelection
    };
    Ok(ProjectSelection {
        canonical_mpr_path,
        canonical_directory,
        location,
    })
}

pub(crate) fn external_selection_path(reference: &str) -> Option<PathBuf> {
    if !is_external_project_token(reference) {
        return None;
    }
    let selections = EXTERNAL_SELECTIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()?;
    selections
        .iter()
        .find(|(token, _)| token == reference)
        .map(|(_, path)| path.clone())
}

fn remember_external_selection(selection: &ProjectSelection) -> Result<String, String> {
    let token = format!(
        "{EXTERNAL_PROJECT_TOKEN_PREFIX}{}",
        selection.project_digest()
    );
    if !is_external_project_token(&token) {
        return Err(crate::tr!("error-project-not-found"));
    }
    let mut selections = EXTERNAL_SELECTIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map_err(|_| crate::tr!("error-project-session-metadata"))?;
    selections.retain(|(existing, _)| *existing != token);
    selections.push((token.clone(), selection.mpr_path().to_path_buf()));
    let overflow = selections.len().saturating_sub(MAX_EXTERNAL_SELECTIONS);
    selections.drain(0..overflow);
    Ok(token)
}

fn is_external_project_token(value: &str) -> bool {
    value
        .strip_prefix(EXTERNAL_PROJECT_TOKEN_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn validate_external_share_root(directory: &Path) -> Result<(), String> {
    if directory.parent().is_none() {
        return Err(crate::tr!("error-project-share-too-broad"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        if Path::new(&home)
            .canonicalize()
            .is_ok_and(|home| home == directory)
        {
            return Err(crate::tr!("error-project-share-too-broad"));
        }
    }
    let metadata =
        fs::metadata(directory).map_err(|_| crate::tr!("error-project-directory-invalid"))?;
    if metadata.permissions().readonly() {
        return Err(crate::tr!("error-project-share-read-only"));
    }
    Ok(())
}

fn validate_freerdp_host_path(directory: &Path) -> Result<(), String> {
    let value = directory
        .to_str()
        .ok_or_else(|| crate::tr!("error-project-path-encoding"))?;
    if value.len() > MAX_PROJECT_PATH_BYTES
        || value.contains(',')
        || value.contains('\\')
        || value.contains('"')
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        return Err(crate::tr!("error-project-path-unsupported"));
    }
    Ok(())
}

fn validate_windows_guest_file_name(path: &Path) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::tr!("error-project-path-encoding"))?;
    if file_name.ends_with([' ', '.'])
        || file_name
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        return Err(crate::tr!("error-project-path-unsupported"));
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit() && suffix != "0"
            })
    {
        return Err(crate::tr!("error-project-path-unsupported"));
    }
    Ok(())
}

#[cfg(unix)]
fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| crate::tr!("error-project-not-found"))?;
        if metadata.file_type().is_symlink() {
            return Err(crate::tr!("error-project-symlink"));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_symlink_components(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn project_from_path(
    mpr_path: &Path,
    windows_path: String,
    location: ProjectLocation,
) -> MendixProject {
    let mut report = ScanReport::default();
    let (version, _) = extract_project_version_bounded(
        &mpr_path
            .parent()
            .unwrap_or(mpr_path)
            .join("project-settings.user.json"),
        MAX_SETTINGS_BYTES,
        &mut report,
    );
    project_with_version(mpr_path, windows_path, location, version, None)
}

fn project_with_version(
    mpr_path: &Path,
    windows_path: String,
    location: ProjectLocation,
    version: Option<String>,
    modified: Option<SystemTime>,
) -> MendixProject {
    let directory = mpr_path.parent().unwrap_or(mpr_path);
    let modified = modified.or_else(|| {
        fs::metadata(mpr_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
    });
    let last_modified = modified.map(|time| {
        let local: DateTime<Local> = time.into();
        local.to_rfc3339()
    });
    MendixProject {
        name: project_name(directory, mpr_path),
        directory: directory.to_string_lossy().to_string(),
        mpr_path: mpr_path.to_string_lossy().to_string(),
        windows_path,
        location,
        version,
        preferred_version: None,
        launch_pending: false,
        favorite: false,
        last_launched_at: None,
        last_modified,
    }
}

fn external_project_from_selection(selection: &ProjectSelection, token: String) -> MendixProject {
    let mut report = ScanReport::default();
    let (version, _) = extract_project_version_bounded(
        &selection.directory().join("project-settings.user.json"),
        MAX_SETTINGS_BYTES,
        &mut report,
    );
    let modified = fs::metadata(selection.mpr_path())
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let last_modified = modified.map(|time| {
        let local: DateTime<Local> = time.into();
        local.to_rfc3339()
    });
    MendixProject {
        name: project_name(selection.directory(), selection.mpr_path()),
        directory: String::new(),
        mpr_path: token,
        windows_path: String::new(),
        location: ProjectLocation::ExplicitHostSelection,
        version,
        preferred_version: None,
        launch_pending: false,
        favorite: false,
        last_launched_at: None,
        last_modified,
    }
}

pub fn linux_path_to_windows_share(
    workspace: &Path,
    path: &Path,
    windows_root: &str,
) -> Result<String, String> {
    let relative = path
        .strip_prefix(workspace)
        .map_err(|_| crate::tr!("error-project-outside-workspace", path = path.display()))?;
    let relative_windows = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\\");
    let root = windows_root.trim_end_matches(['\\', '/']);
    if relative_windows.is_empty() {
        Ok(root.to_string())
    } else {
        Ok(format!("{root}\\{relative_windows}"))
    }
}

fn should_visit(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git"
            | ".svn"
            | ".hg"
            | ".mendimaru"
            | ".mendix-cache"
            | "node_modules"
            | "deployment"
            | "theme-cache"
            | "dist"
            | "target"
    )
}

fn has_mpr_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mpr"))
}

fn project_name(directory: &Path, mpr_path: &Path) -> String {
    let file_stem = mpr_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Mendix project");
    if file_stem.eq_ignore_ascii_case("app") {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file_stem)
            .to_string()
    } else {
        file_stem.to_string()
    }
}

#[cfg(test)]
fn extract_project_version(settings_path: &Path) -> Option<String> {
    let content = fs::read_to_string(settings_path).ok()?;
    let settings: Value = serde_json::from_str(&content).ok()?;
    let version_regex = Regex::new(r"Version=(\d+\.\d+\.\d+)(?:\.\d+)?").ok()?;
    let mut versions = settings
        .get("settingsParts")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("type")?.as_str())
        .filter_map(|type_name| {
            version_regex
                .captures(type_name)
                .and_then(|captures| captures.get(1))
                .map(|version| version.as_str().to_string())
        })
        .collect::<HashSet<_>>();
    if versions.len() == 1 {
        versions.drain().next()
    } else {
        None
    }
}

fn extract_project_version_bounded(
    settings_path: &Path,
    maximum_bytes: u64,
    report: &mut ScanReport,
) -> (Option<String>, u64) {
    let metadata = match fs::symlink_metadata(settings_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (None, 0);
        }
        Err(error) => {
            report.skipped_entries += 1;
            report.push_error(format!("{settings_path:?}: {error}"));
            return (None, 0);
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        report.skipped_entries += 1;
        report.push_error(format!(
            "{settings_path:?}: project settings must be a regular file"
        ));
        return (None, 0);
    }
    if metadata.len() > maximum_bytes {
        report.skipped_entries += 1;
        report.push_error(format!(
            "{settings_path:?}: project settings exceed {} bytes",
            maximum_bytes
        ));
        return (None, 0);
    }

    let mut content = Vec::with_capacity(metadata.len() as usize);
    let read_result = File::open(settings_path).and_then(|file| {
        file.take(maximum_bytes + 1)
            .read_to_end(&mut content)
            .map(|_| ())
    });
    if let Err(error) = read_result {
        report.skipped_entries += 1;
        report.push_error(format!("{settings_path:?}: {error}"));
        return (None, 0);
    }
    if content.len() as u64 > maximum_bytes {
        report.skipped_entries += 1;
        report.push_error(format!(
            "{settings_path:?}: project settings changed past the safe size limit"
        ));
        return (None, 0);
    }

    let settings = match serde_json::from_slice::<Value>(&content) {
        Ok(settings) => settings,
        Err(error) => {
            report.skipped_entries += 1;
            report.push_error(format!("{settings_path:?}: {error}"));
            return (None, content.len() as u64);
        }
    };
    (distinct_project_version(&settings), content.len() as u64)
}

fn distinct_project_version(settings: &Value) -> Option<String> {
    let version_regex = SETTINGS_VERSION_REGEX.get_or_init(|| {
        Regex::new(r"Version=(\d+\.\d+\.\d+)(?:\.\d+)?").expect("valid version regex")
    });
    let mut versions = settings
        .get("settingsParts")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("type")?.as_str())
        .filter_map(|type_name| {
            version_regex
                .captures(type_name)
                .and_then(|captures| captures.get(1))
                .map(|version| version.as_str().to_string())
        })
        .collect::<HashSet<_>>();
    if versions.len() == 1 {
        versions.drain().next()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        extract_project_version, extract_project_version_bounded, linux_path_to_windows_share,
        scan_projects, scan_projects_with_limits, Duration, ScanLimits, ScanReport,
    };
    #[cfg(target_os = "linux")]
    use super::{inspect_selected_project, validate_project_selection};
    use crate::models::{AppConfig, ContainerRuntime, ProjectLocation};
    use std::fs;
    use std::sync::{atomic::AtomicBool, Arc};

    fn config_for(path: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47271".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47273,
            shared_directory: path.to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    #[test]
    fn maps_linux_project_path_to_windows_share() {
        let mapped = linux_path_to_windows_share(
            std::path::Path::new("/workspace"),
            std::path::Path::new("/workspace/Customer Portal/App.mpr"),
            r"\\host.lan\Data",
        )
        .expect("mapped path");
        assert_eq!(mapped, r"\\host.lan\Data\Customer Portal\App.mpr");
    }

    #[test]
    fn scans_only_projects_under_shared_workspace() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let project = temporary.path().join("Orders");
        fs::create_dir_all(&project).expect("project dir");
        fs::write(project.join("Orders.mpr"), b"mpr").expect("mpr file");
        fs::write(
            project.join("project-settings.user.json"),
            r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0, Culture=neutral"}]}"#,
        )
        .expect("settings");

        let projects = scan_projects(&config_for(temporary.path())).expect("projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Orders");
        assert_eq!(projects[0].location, ProjectLocation::ConfiguredWorkspace);
        assert_eq!(projects[0].version.as_deref(), Some("11.12.2"));
        assert!(projects[0].windows_path.ends_with(r"Orders\Orders.mpr"));
    }

    #[test]
    fn reads_settings_at_the_exact_byte_limit_and_rejects_one_more_byte() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let settings = temporary.path().join("project-settings.user.json");
        let content = format!(
            "{{\"settingsParts\":[{{\"type\":\"A, Version=11.12.2.0\"}}]{}}}",
            " ".repeat(128)
        );
        fs::write(&settings, &content).expect("settings");
        let mut report = ScanReport::default();
        let maximum = content.len() as u64;

        let (version, bytes) = extract_project_version_bounded(&settings, maximum, &mut report);
        assert_eq!(version.as_deref(), Some("11.12.2"));
        assert_eq!(bytes, maximum);
        assert_eq!(report.error_count, 0);

        let (version, bytes) = extract_project_version_bounded(&settings, maximum - 1, &mut report);
        assert_eq!(version, None);
        assert_eq!(bytes, 0);
        assert_eq!(report.error_count, 1);
        assert!(report.errors[0].contains("exceed"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_oversized_sparse_symlink_and_special_settings_files() {
        use super::MAX_SETTINGS_BYTES;
        use std::ffi::CString;
        use std::io::{Seek, SeekFrom, Write};

        let temporary = tempfile::tempdir().expect("temp dir");
        let project = temporary.path().join("Orders");
        fs::create_dir(&project).expect("project directory");
        fs::write(project.join("Orders.mpr"), b"mpr").expect("mpr");
        let settings = project.join("project-settings.user.json");
        let mut sparse = fs::File::create(&settings).expect("sparse settings");
        sparse
            .seek(SeekFrom::Start(MAX_SETTINGS_BYTES + 1))
            .map(|_| ())
            .and_then(|()| sparse.write_all(b" "))
            .expect("sparse settings");

        let result = scan_projects(&config_for(temporary.path())).expect("scan result");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].version, None);
        let report =
            scan_projects_with_limits(&config_for(temporary.path()), ScanLimits::default(), None)
                .expect("scan report");
        assert!(!report.truncated);
        assert!(report.skipped_entries >= 1);
        assert!(report.errors.iter().any(|error| error.contains("exceed")));

        let linked_project = temporary.path().join("Linked");
        fs::create_dir(&linked_project).expect("linked project directory");
        fs::write(linked_project.join("Linked.mpr"), b"mpr").expect("linked mpr");
        std::os::unix::fs::symlink(&settings, linked_project.join("project-settings.user.json"))
            .expect("linked settings");
        let report =
            scan_projects_with_limits(&config_for(temporary.path()), ScanLimits::default(), None)
                .expect("scan report");
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("regular file")));

        let fifo_project = temporary.path().join("Fifo");
        fs::create_dir(&fifo_project).expect("fifo project directory");
        fs::write(fifo_project.join("Fifo.mpr"), b"mpr").expect("fifo mpr");
        let fifo_settings = fifo_project.join("project-settings.user.json");
        let fifo_path =
            CString::new(fifo_settings.as_os_str().as_encoded_bytes()).expect("fifo settings path");
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        let report =
            scan_projects_with_limits(&config_for(temporary.path()), ScanLimits::default(), None)
                .expect("scan report");
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("regular file")));
    }

    #[test]
    fn stops_at_entry_project_and_total_settings_limits() {
        let temporary = tempfile::tempdir().expect("temp dir");
        for name in ["Alpha", "Beta", "Gamma"] {
            let project = temporary.path().join(name);
            fs::create_dir(&project).expect("project directory");
            fs::write(project.join(format!("{name}.mpr")), b"mpr").expect("mpr");
            fs::write(
                project.join("project-settings.user.json"),
                r#"{"settingsParts":[{"type":"A, Version=11.12.2.0"}]}"#,
            )
            .expect("settings");
        }
        let config = config_for(temporary.path());
        let limits = ScanLimits {
            max_entries: 2,
            ..ScanLimits::default()
        };
        let result = scan_projects_with_limits(&config, limits, None).expect("entry limited scan");
        assert!(result.truncated);
        assert_eq!(result.visited_entries, 2);
        assert!(result.projects.len() <= 1);
        assert_eq!(result.error_count, 1);

        let limits = ScanLimits {
            max_projects: 1,
            ..ScanLimits::default()
        };
        let result =
            scan_projects_with_limits(&config, limits, None).expect("project limited scan");
        assert!(result.truncated);
        assert_eq!(result.projects.len(), 1);

        let limits = ScanLimits {
            max_total_settings_bytes: 50,
            ..ScanLimits::default()
        };
        let result = scan_projects_with_limits(&config, limits, None).expect("byte limited scan");
        assert!(result.truncated);
        assert_eq!(result.projects.len(), 1);
        assert!(result.settings_bytes_read > 50);
    }

    #[test]
    fn cancels_and_stops_an_elapsed_scan() {
        let temporary = tempfile::tempdir().expect("temp dir");
        for index in 0..200 {
            fs::create_dir(temporary.path().join(format!("entry-{index}"))).expect("entry");
        }
        let limits = ScanLimits {
            max_duration: Duration::ZERO,
            ..ScanLimits::default()
        };
        let result = scan_projects_with_limits(&config_for(temporary.path()), limits, None)
            .expect("elapsed scan");
        assert!(result.truncated);
        assert!(result.visited_entries < 200);

        let cancellation = Arc::new(AtomicBool::new(true));
        let result = scan_projects_with_limits(
            &config_for(temporary.path()),
            ScanLimits::default(),
            Some(&cancellation),
        )
        .expect("cancelled scan");
        assert_eq!(result.visited_entries, 0);
        assert!(result.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn reports_unreadable_directories_as_partial_scan_errors() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temp dir");
        let blocked = temporary.path().join("blocked");
        fs::create_dir(&blocked).expect("blocked directory");
        fs::write(blocked.join("Orders.mpr"), b"mpr").expect("blocked mpr");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000))
            .expect("remove directory permissions");
        let result =
            scan_projects_with_limits(&config_for(temporary.path()), ScanLimits::default(), None);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700))
            .expect("restore directory permissions");
        let result = result.expect("partial scan");

        assert!(result.skipped_entries >= 1);
        assert!(result.error_count >= 1);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn continues_to_exclude_generated_directories_from_scans() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let project = temporary.path().join("Orders");
        fs::create_dir(&project).expect("project directory");
        fs::write(project.join("Orders.mpr"), b"mpr").expect("mpr");
        for excluded in [".git", "node_modules", "deployment", ".mendix-cache"] {
            let directory = temporary.path().join(excluded);
            fs::create_dir(&directory).expect("excluded directory");
            fs::write(directory.join("Excluded.mpr"), b"mpr").expect("excluded mpr");
        }

        let projects = scan_projects(&config_for(temporary.path())).expect("projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Orders");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn classifies_an_explicit_project_without_adding_a_guest_path_early() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let project = outside.path().join("Orders");
        fs::create_dir(&project).expect("project directory");
        let mpr = project.join("Orders.mpr");
        fs::write(&mpr, b"mpr").expect("project");

        let inspected =
            inspect_selected_project(&config_for(workspace.path()), &mpr).expect("selection");
        assert_eq!(inspected.location, ProjectLocation::ExplicitHostSelection);
        assert!(inspected.windows_path.is_empty());
        assert!(inspected.mpr_path.starts_with("external-project_"));
        assert!(inspected.mpr_path.len() == "external-project_".len() + 64);
        assert!(inspected.directory.is_empty());
        let serialized = serde_json::to_string(&inspected).expect("serialized selection");
        assert!(!serialized.contains(&project.to_string_lossy().to_string()));
        assert!(!serialized.contains(&mpr.to_string_lossy().to_string()));
        assert!(
            crate::projects::external_selection_path(&inspected.mpr_path).is_some(),
            "the current-process launch intent remains resolvable"
        );
        assert_eq!(
            validate_project_selection(&config_for(workspace.path()), &mpr)
                .expect("validated")
                .directory(),
            project.canonicalize().expect("canonical project")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_symlinked_files_and_parent_directory_aliases() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let project = outside.path().join("Orders");
        fs::create_dir(&project).expect("project directory");
        let mpr = project.join("Orders.mpr");
        fs::write(&mpr, b"mpr").expect("project");
        let linked_file = outside.path().join("Linked.mpr");
        symlink(&mpr, &linked_file).expect("file link");
        let linked_parent = outside.path().join("Alias");
        symlink(&project, &linked_parent).expect("parent link");

        let config = config_for(workspace.path());
        assert!(validate_project_selection(&config, &linked_file).is_err());
        assert!(validate_project_selection(&config, &linked_parent.join("Orders.mpr")).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_non_files_wrong_extensions_and_freerdp_ambiguous_paths() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let directory = outside.path().join("Orders");
        fs::create_dir(&directory).expect("directory");
        let wrong = directory.join("Orders.txt");
        fs::write(&wrong, b"not mpr").expect("wrong file");
        assert!(validate_project_selection(&config_for(workspace.path()), &wrong).is_err());
        assert!(validate_project_selection(&config_for(workspace.path()), &directory).is_err());

        let ambiguous = outside.path().join("comma,project");
        fs::create_dir(&ambiguous).expect("ambiguous directory");
        let mpr = ambiguous.join("Orders.mpr");
        fs::write(&mpr, b"mpr").expect("mpr");
        assert!(validate_project_selection(&config_for(workspace.path()), &mpr).is_err());

        let quoted = outside.path().join("double\"quote");
        fs::create_dir(&quoted).expect("quoted directory");
        let mpr = quoted.join("Orders.mpr");
        fs::write(&mpr, b"mpr").expect("quoted mpr");
        assert!(validate_project_selection(&config_for(workspace.path()), &mpr).is_err());

        let reserved = directory.join("CON.mpr");
        fs::write(&reserved, b"mpr").expect("reserved mpr");
        assert!(validate_project_selection(&config_for(workspace.path()), &reserved).is_err());
    }

    #[test]
    fn infers_only_one_distinct_project_version() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let settings = temporary.path().join("project-settings.user.json");
        fs::write(
            &settings,
            r#"{"settingsParts":[{"type":"A, Version=11.12.2.0"},{"type":"B, Version=11.12.2.0"}]}"#,
        )
        .expect("unambiguous settings");
        assert_eq!(
            extract_project_version(&settings).as_deref(),
            Some("11.12.2")
        );

        fs::write(
            &settings,
            r#"{"settingsParts":[{"type":"A, Version=11.12.2.0"},{"type":"B, Version=10.24.9.0"}]}"#,
        )
        .expect("ambiguous settings");
        assert_eq!(extract_project_version(&settings), None);
    }
}
