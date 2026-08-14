use crate::models::{AppConfig, MendixProject};
use chrono::{DateTime, Local};
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Component, Path};
use std::time::SystemTime;
use walkdir::{DirEntry, WalkDir};

const MAX_WORKSPACE_DEPTH: usize = 8;

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
        let directory = mpr_path.parent().unwrap_or(workspace);
        let metadata = fs::metadata(mpr_path).ok();
        let modified = metadata.as_ref().and_then(|value| value.modified().ok());
        let last_modified = modified.map(|time| {
            let local: DateTime<Local> = time.into();
            local.to_rfc3339()
        });
        let project_name = project_name(directory, mpr_path);
        let windows_path =
            linux_path_to_windows_share(workspace, mpr_path, &config.windows_shared_directory)?;
        let version = extract_project_version(&directory.join("project-settings.user.json"));

        discovered.push((
            modified.unwrap_or(SystemTime::UNIX_EPOCH),
            MendixProject {
                name: project_name,
                directory: directory.to_string_lossy().to_string(),
                mpr_path: mpr_path.to_string_lossy().to_string(),
                windows_path,
                version,
                last_modified,
            },
        ));
    }

    discovered.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(discovered.into_iter().map(|(_, project)| project).collect())
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
    settings
        .get("settingsParts")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("type")?.as_str())
        .find_map(|type_name| {
            version_regex
                .captures(type_name)
                .and_then(|captures| captures.get(1))
                .map(|version| version.as_str().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::{linux_path_to_windows_share, scan_projects};
    use crate::models::{AppConfig, ContainerRuntime};
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
        assert_eq!(projects[0].version.as_deref(), Some("11.12.2"));
        assert!(projects[0].windows_path.ends_with(r"Orders\Orders.mpr"));
    }
}
