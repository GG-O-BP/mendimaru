use crate::models::AppConfig;
use std::collections::HashSet;
use std::path::Path;

pub(crate) fn normalize_and_validate(config: &mut AppConfig) -> Result<(), String> {
    config.language_preference = crate::i18n::normalize_preference(&config.language_preference)?;
    config.winboat_executable = super::expand_home(config.winboat_executable.trim());
    config.compose_file = super::expand_home(config.compose_file.trim());
    config.shared_directory = super::expand_home(config.shared_directory.trim());
    config.freerdp_binary = super::expand_home(config.freerdp_binary.trim());
    config.api_url = config.api_url.trim_end_matches('/').trim().to_string();
    config.rdp_host = config.rdp_host.trim().to_string();
    config.windows_shared_directory = config
        .windows_shared_directory
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_install_root = config
        .mendix_install_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_data_root = config
        .mendix_data_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    let mut seen_studio_paths = HashSet::new();
    let validate_native_paths = crate::platform::is_windows_native();
    config.windows_studio_paths = config
        .windows_studio_paths
        .iter()
        .filter_map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return None;
            }
            if !validate_native_paths {
                return Some(Ok(trimmed.to_string()));
            }
            let path = Path::new(trimmed);
            if !path.is_absolute() || (!path.is_dir() && !path.is_file()) {
                return Some(Err(crate::tr!(
                    "error-native-custom-path-invalid",
                    path = trimmed
                )));
            }
            let normalized = canonical_display_path(path)
                .map_err(|error| crate::tr!("error-shared-directory-inspect", error = error));
            Some(normalized.map(|path| path.to_string_lossy().to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|value| seen_studio_paths.insert(value.to_ascii_lowercase()))
        .collect();
    config.container_name = config.container_name.trim().to_string();

    let shared = Path::new(&config.shared_directory);
    if !shared.is_absolute() || !shared.is_dir() {
        return Err(crate::tr!("error-shared-directory-invalid"));
    }
    config.shared_directory = canonical_display_path(shared)
        .map_err(|error| crate::tr!("error-shared-directory-inspect", error = error))?
        .to_string_lossy()
        .to_string();

    if crate::platform::is_windows_native() {
        return Ok(());
    }

    if config.compose_file.is_empty()
        || config.container_name.is_empty()
        || config.api_url.is_empty()
        || config.rdp_host.is_empty()
        || config.windows_shared_directory.is_empty()
    {
        return Err(crate::tr!("error-winboat-connection-empty"));
    }
    if config.startup_timeout_seconds == 0 || config.startup_timeout_seconds > 900 {
        return Err(crate::tr!(
            "error-startup-timeout-range",
            minimum = crate::i18n::format_number(1),
            maximum = crate::i18n::format_number(900)
        ));
    }
    Ok(())
}

fn canonical_display_path(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let canonical = path.canonicalize()?;
    if !cfg!(target_os = "windows") {
        return Ok(canonical);
    }
    let value = canonical.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        return Ok(std::path::PathBuf::from(format!(r"\\{unc}")));
    }
    if let Some(local) = value.strip_prefix(r"\\?\") {
        return Ok(std::path::PathBuf::from(local));
    }
    Ok(canonical)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{canonical_display_path, normalize_and_validate};
    use crate::models::{AppConfig, ContainerRuntime};
    use std::fs;

    fn config(workspace: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: String::new(),
            compose_file: String::new(),
            container_runtime: ContainerRuntime::Docker,
            container_name: String::new(),
            api_url: String::new(),
            rdp_host: String::new(),
            rdp_port: 0,
            shared_directory: workspace.to_string_lossy().to_string(),
            windows_shared_directory: String::new(),
            freerdp_binary: String::new(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    #[test]
    fn canonicalizes_and_deduplicates_native_custom_paths() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let executable = temporary
            .path()
            .join("Portable/11.12.2/modeler/StudioPro.exe");
        fs::create_dir_all(executable.parent().expect("modeler directory"))
            .expect("create modeler directory");
        fs::write(&executable, b"fixture").expect("write Studio fixture");
        let raw = executable.to_string_lossy().to_string();
        let mut config = config(temporary.path());
        config.windows_studio_paths = vec![format!("  {raw}  "), raw];

        normalize_and_validate(&mut config).expect("valid native configuration");

        assert_eq!(config.windows_studio_paths.len(), 1);
        assert_eq!(
            std::path::PathBuf::from(&config.windows_studio_paths[0]),
            canonical_display_path(&executable).expect("canonical executable")
        );
    }

    #[test]
    fn rejects_missing_native_workspace_and_custom_paths() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let mut missing_custom = config(temporary.path());
        missing_custom.windows_studio_paths = vec![temporary
            .path()
            .join("missing/StudioPro.exe")
            .to_string_lossy()
            .to_string()];
        assert!(normalize_and_validate(&mut missing_custom).is_err());

        let mut missing_workspace = config(&temporary.path().join("missing-workspace"));
        assert!(normalize_and_validate(&mut missing_workspace).is_err());
    }
}
