use crate::models::{AppConfig, MendixProject, ProjectLocation};
use chrono::{DateTime, Local};
use regex::Regex;
use serde_json::Value;
use sha2::Digest;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use walkdir::{DirEntry, WalkDir};

const MAX_WORKSPACE_DEPTH: usize = 8;
const MAX_PROJECT_PATH_BYTES: usize = 4_096;
const MAX_EXTERNAL_SELECTIONS: usize = 8;
const EXTERNAL_PROJECT_TOKEN_PREFIX: &str = "external-project_";

static EXTERNAL_SELECTIONS: OnceLock<Mutex<Vec<(String, PathBuf)>>> = OnceLock::new();

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
    let workspace = Path::new(&config.shared_directory);
    if !workspace.is_dir() {
        return Err(crate::tr!(
            "error-workspace-not-found",
            path = &config.shared_directory
        ));
    }

    let mut discovered = Vec::new();
    for entry in WalkDir::new(workspace)
        .max_depth(MAX_WORKSPACE_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_visit)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || !has_mpr_extension(entry.path()) {
            continue;
        }

        let mpr_path = entry.path();
        let windows_path = if crate::platform::is_windows_native() {
            mpr_path.to_string_lossy().to_string()
        } else {
            linux_path_to_windows_share(workspace, mpr_path, &config.windows_shared_directory)?
        };
        let project =
            project_from_path(mpr_path, windows_path, ProjectLocation::ConfiguredWorkspace);
        let modified = fs::metadata(mpr_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        discovered.push((modified, project));
    }

    discovered.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(discovered.into_iter().map(|(_, project)| project).collect())
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
    let directory = mpr_path.parent().unwrap_or(mpr_path);
    let modified = fs::metadata(mpr_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
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
        version: extract_project_version(&directory.join("project-settings.user.json")),
        preferred_version: None,
        launch_pending: false,
        last_modified,
    }
}

fn external_project_from_selection(selection: &ProjectSelection, token: String) -> MendixProject {
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
        version: extract_project_version(&selection.directory().join("project-settings.user.json")),
        preferred_version: None,
        launch_pending: false,
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

#[cfg(test)]
mod tests {
    use super::{
        extract_project_version, inspect_selected_project, linux_path_to_windows_share,
        scan_projects, validate_project_selection,
    };
    use crate::models::{AppConfig, ContainerRuntime, ProjectLocation};
    use std::fs;

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
