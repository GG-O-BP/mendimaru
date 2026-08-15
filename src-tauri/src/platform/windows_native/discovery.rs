use super::process::hidden_command;
use crate::models::{AppConfig, StudioVersion};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};
use winreg::RegKey;

const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const VERSION_SELECTOR_KEY: &str = r"SOFTWARE\Mendix\Mendix Version Selector";
const VERSION_SELECTOR_FILE_COMMAND: &str =
    r"SOFTWARE\Classes\Mendix Version Selector.mpr\shell\open\command";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UninstallCommand {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstallationRecord {
    pub studio: StudioVersion,
    pub uninstall: Option<UninstallCommand>,
}

#[derive(Debug, Clone)]
struct Candidate {
    version: String,
    display_name: String,
    executable: PathBuf,
    install_root: PathBuf,
    source: String,
    uninstall: Option<UninstallCommand>,
    priority: u8,
}

#[derive(Debug, Clone)]
struct RegistryEntry {
    display_name: String,
    publisher: String,
    display_version: Option<String>,
    install_location: Option<String>,
    display_icon: Option<String>,
    quiet_uninstall: Option<String>,
    uninstall: Option<String>,
}

pub(super) fn discover(config: &AppConfig) -> Vec<InstallationRecord> {
    let mut candidates = registry_candidates();
    let selector = version_selector_evidence();

    for root in standard_roots(config) {
        candidates.extend(scan_install_root(&root, "Standard path", 30, config));
    }
    for root in selector.install_roots {
        candidates.extend(scan_install_root(&root, "Version Selector", 40, config));
    }
    for executable in selector.custom_executables {
        if let Some(candidate) = candidate_from_executable(
            &executable,
            "Version Selector custom version",
            60,
            None,
            config,
        ) {
            candidates.push(candidate);
        }
    }
    for path in &config.windows_studio_paths {
        candidates.extend(scan_custom_path(Path::new(path), config));
    }

    merge_candidates(candidates)
}

pub(super) fn find(config: &AppConfig, version: &str) -> Option<InstallationRecord> {
    discover(config)
        .into_iter()
        .find(|record| record.studio.version == version)
}

fn registry_candidates() -> Vec<Candidate> {
    let mut result = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let root = RegKey::predef(hive);
            let Ok(uninstall_root) = root.open_subkey_with_flags(UNINSTALL_KEY, KEY_READ | view)
            else {
                continue;
            };
            for subkey_name in uninstall_root.enum_keys().filter_map(Result::ok) {
                let Ok(subkey) =
                    uninstall_root.open_subkey_with_flags(&subkey_name, KEY_READ | view)
                else {
                    continue;
                };
                let entry = RegistryEntry {
                    display_name: registry_string(&subkey, "DisplayName").unwrap_or_default(),
                    publisher: registry_string(&subkey, "Publisher").unwrap_or_default(),
                    display_version: registry_string(&subkey, "DisplayVersion"),
                    install_location: registry_string(&subkey, "InstallLocation"),
                    display_icon: registry_string(&subkey, "DisplayIcon"),
                    quiet_uninstall: registry_string(&subkey, "QuietUninstallString"),
                    uninstall: registry_string(&subkey, "UninstallString"),
                };
                if let Some(candidate) = candidate_from_registry_entry(entry) {
                    result.push(candidate);
                }
            }
        }
    }
    result
}

fn candidate_from_registry_entry(entry: RegistryEntry) -> Option<Candidate> {
    if !is_studio_pro_entry(&entry.display_name, &entry.publisher) {
        return None;
    }
    let version = entry
        .display_version
        .as_deref()
        .and_then(normalized_version)
        .or_else(|| normalized_version(&entry.display_name))?;
    let install_location = entry
        .install_location
        .filter(|value| !value.trim().is_empty())
        .map(|value| PathBuf::from(value.trim().trim_matches('"')));
    let executable = entry
        .display_icon
        .as_deref()
        .and_then(display_icon_path)
        .filter(|path| is_studio_executable(path))
        .filter(|path| path.is_file())
        .or_else(|| {
            install_location
                .as_ref()
                .map(|root| root.join(r"modeler\studiopro.exe"))
                .filter(|path| path.is_file())
        })?;
    let install_root = install_location
        .filter(|path| path.is_dir())
        .or_else(|| studio_install_root(&executable))
        .unwrap_or_else(|| executable.clone());
    let uninstall = entry
        .quiet_uninstall
        .filter(|value| !value.trim().is_empty())
        .or(entry.uninstall)
        .and_then(|command| parse_uninstall_command(&command));
    Some(Candidate {
        version,
        display_name: entry.display_name,
        executable,
        install_root,
        source: "Windows Registry".to_string(),
        uninstall,
        priority: 100,
    })
}

fn is_studio_pro_entry(display_name: &str, publisher: &str) -> bool {
    let name = display_name.trim().to_ascii_lowercase();
    let publisher = publisher.trim().to_ascii_lowercase();
    matches!(
        publisher.as_str(),
        "mendix"
            | "mendix technology"
            | "mendix technology b.v."
            | "mendix technology bv"
            | "siemens"
            | "siemens ag"
    ) && name.starts_with("mendix ")
        && !name.contains("version selector")
        && !name.contains("native mobile")
        && normalized_version(display_name).is_some()
}

fn registry_string(key: &RegKey, name: &str) -> Option<String> {
    key.get_value::<String, _>(name).ok()
}

fn display_icon_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    let without_index = Regex::new(r",\s*-?\d+\s*$")
        .expect("display icon suffix regex")
        .replace(trimmed, "");
    let path = without_index.trim().trim_matches('"');
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn parse_uninstall_command(value: &str) -> Option<UninstallCommand> {
    let parts = split_windows_command_line(value);
    let (executable, arguments) = parts.split_first()?;
    let executable = PathBuf::from(executable);
    if executable.as_os_str().is_empty() {
        return None;
    }
    Some(UninstallCommand {
        executable,
        arguments: arguments.to_vec(),
    })
}

fn split_windows_command_line(value: &str) -> Vec<String> {
    let characters: Vec<char> = value.chars().collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        if index == characters.len() {
            break;
        }
        let mut argument = String::new();
        let mut quoted = false;
        while index < characters.len() {
            if characters[index].is_whitespace() && !quoted {
                break;
            }
            if characters[index] == '\\' {
                let start = index;
                while index < characters.len() && characters[index] == '\\' {
                    index += 1;
                }
                let count = index - start;
                if index < characters.len() && characters[index] == '"' {
                    argument.extend(std::iter::repeat_n('\\', count / 2));
                    if count % 2 == 0 {
                        quoted = !quoted;
                    } else {
                        argument.push('"');
                    }
                    index += 1;
                } else {
                    argument.extend(std::iter::repeat_n('\\', count));
                }
                continue;
            }
            if characters[index] == '"' {
                quoted = !quoted;
                index += 1;
                continue;
            }
            argument.push(characters[index]);
            index += 1;
        }
        result.push(argument);
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
    }
    result
}

#[derive(Default)]
struct VersionSelectorEvidence {
    install_roots: Vec<PathBuf>,
    custom_executables: Vec<PathBuf>,
}

fn version_selector_evidence() -> VersionSelectorEvidence {
    let mut evidence = VersionSelectorEvidence::default();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let root = RegKey::predef(hive);
            if let Ok(key) = root.open_subkey_with_flags(VERSION_SELECTOR_KEY, KEY_READ | view) {
                if let Some(path) = registry_string(&key, "Path") {
                    add_selector_root(Path::new(path.trim().trim_matches('"')), &mut evidence);
                }
                collect_selector_paths(&key, 0, &mut evidence.custom_executables);
            }
            if let Ok(key) =
                root.open_subkey_with_flags(VERSION_SELECTOR_FILE_COMMAND, KEY_READ | view)
            {
                if let Some(command) = registry_string(&key, "") {
                    if let Some(executable) = split_windows_command_line(&command).first() {
                        add_selector_root(Path::new(executable), &mut evidence);
                    }
                }
            }
        }
    }
    evidence
}

fn add_selector_root(path: &Path, evidence: &mut VersionSelectorEvidence) {
    let selector_directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if let Some(root) = selector_directory.parent() {
        evidence.install_roots.push(root.to_path_buf());
    }
}

fn collect_selector_paths(key: &RegKey, depth: usize, paths: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    for (name, _) in key.enum_values().filter_map(Result::ok) {
        if let Ok(value) = key.get_value::<String, _>(&name) {
            let path = PathBuf::from(value.trim().trim_matches('"'));
            if is_studio_executable(&path) && path.is_file() {
                paths.push(path);
            }
        }
    }
    for name in key.enum_keys().filter_map(Result::ok) {
        if let Ok(child) = key.open_subkey_with_flags(name, KEY_READ) {
            collect_selector_paths(&child, depth + 1, paths);
        }
    }
}

fn standard_roots(config: &AppConfig) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(&config.mendix_install_root)];
    for variable in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(directory) = std::env::var_os(variable) {
            roots.push(PathBuf::from(directory).join("Mendix"));
        }
    }
    roots
}

fn scan_install_root(
    root: &Path,
    source: &str,
    priority: u8,
    config: &AppConfig,
) -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let executable = entry.path().join(r"modeler\studiopro.exe");
            candidate_from_executable(&executable, source, priority, None, config)
        })
        .collect()
}

fn scan_custom_path(path: &Path, config: &AppConfig) -> Vec<Candidate> {
    if path.is_file() {
        return candidate_from_executable(path, "Custom path", 70, None, config)
            .into_iter()
            .collect();
    }
    if !path.is_dir() {
        return Vec::new();
    }
    let direct = [
        path.join("studiopro.exe"),
        path.join("StudioPro.exe"),
        path.join(r"modeler\studiopro.exe"),
        path.join(r"modeler\StudioPro.exe"),
    ];
    let mut candidates: Vec<_> = direct
        .iter()
        .filter_map(|executable| {
            candidate_from_executable(executable, "Custom path", 70, None, config)
        })
        .collect();
    candidates.extend(scan_install_root(path, "Custom path", 70, config));
    candidates
}

fn candidate_from_executable(
    executable: &Path,
    source: &str,
    priority: u8,
    version_hint: Option<&str>,
    config: &AppConfig,
) -> Option<Candidate> {
    if !is_studio_executable(executable) || !executable.is_file() {
        return None;
    }
    let install_root = studio_install_root(executable)?;
    let version = version_hint
        .and_then(normalized_version)
        .or_else(|| version_from_path(&install_root))
        .or_else(|| product_version(executable))?;
    let uninstall = fallback_uninstaller(config, &install_root);
    Some(Candidate {
        display_name: format!("Mendix {version}"),
        version,
        executable: executable.to_path_buf(),
        install_root,
        source: source.to_string(),
        uninstall,
        priority,
    })
}

fn studio_install_root(executable: &Path) -> Option<PathBuf> {
    let modeler = executable.parent()?;
    if modeler
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("modeler"))
    {
        modeler.parent().map(Path::to_path_buf)
    } else {
        Some(modeler.to_path_buf())
    }
}

fn is_studio_executable(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("studiopro.exe"))
}

fn version_from_path(path: &Path) -> Option<String> {
    path.ancestors().take(4).find_map(|component| {
        component
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(normalized_version)
    })
}

fn product_version(executable: &Path) -> Option<String> {
    let output = hidden_command("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write((Get-Item -LiteralPath $env:MENDIMARU_VERSION_PATH).VersionInfo.ProductVersion)",
        ])
        .env("MENDIMARU_VERSION_PATH", executable)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
        .as_deref()
        .and_then(normalized_version)
}

fn normalized_version(value: &str) -> Option<String> {
    Regex::new(r"(?i)(\d+\.\d+\.\d+)(?:\.\d+)?(-(?:beta|rc)(?:\.?\d+)?)?")
        .expect("Studio Pro version regex")
        .captures(value)
        .map(|captures| {
            let base = captures.get(1).expect("version base").as_str();
            let prerelease = captures
                .get(2)
                .map(|value| value.as_str().to_ascii_lowercase())
                .unwrap_or_default();
            format!("{base}{prerelease}")
        })
}

fn fallback_uninstaller(config: &AppConfig, install_root: &Path) -> Option<UninstallCommand> {
    let folder = install_root.file_name()?;
    let executable = Path::new(&config.mendix_data_root)
        .join(folder)
        .join(r"uninst\unins000.exe");
    executable.is_file().then(|| UninstallCommand {
        executable,
        arguments: vec!["/SILENT".to_string()],
    })
}

fn merge_candidates(candidates: Vec<Candidate>) -> Vec<InstallationRecord> {
    let mut merged = BTreeMap::<String, Candidate>::new();
    for candidate in candidates {
        // Command APIs identify an installation by its public Studio Pro version.
        // Keep exactly one deterministic record per version so launch/uninstall can
        // never resolve to a different executable than the one shown in the UI.
        let key = candidate.version.to_ascii_lowercase();
        match merged.get_mut(&key) {
            Some(existing) => {
                let existing_path = normalized_path_key(&existing.executable);
                let candidate_path = normalized_path_key(&candidate.executable);
                let same_executable = existing_path == candidate_path;
                if same_executable && existing.uninstall.is_none() && candidate.uninstall.is_some()
                {
                    existing.uninstall = candidate.uninstall.clone();
                }
                let candidate_is_preferred = candidate.priority > existing.priority
                    || (candidate.priority == existing.priority
                        && candidate.uninstall.is_some()
                        && existing.uninstall.is_none())
                    || (candidate.priority == existing.priority
                        && candidate.uninstall.is_some() == existing.uninstall.is_some()
                        && candidate_path < existing_path);
                if candidate_is_preferred {
                    // Uninstall metadata is installation-specific. Only inherit it
                    // when both discoveries point to the same StudioPro.exe.
                    let uninstall = candidate.uninstall.clone().or_else(|| {
                        same_executable
                            .then(|| existing.uninstall.clone())
                            .flatten()
                    });
                    *existing = Candidate {
                        uninstall,
                        ..candidate
                    };
                }
            }
            None => {
                merged.insert(key, candidate);
            }
        }
    }
    let mut records: Vec<_> = merged
        .into_values()
        .map(|candidate| InstallationRecord {
            studio: StudioVersion {
                version: candidate.version,
                display_name: candidate.display_name,
                executable_path: candidate.executable.to_string_lossy().to_string(),
                install_root: candidate.install_root.to_string_lossy().to_string(),
                source: candidate.source,
                removable: candidate.uninstall.is_some(),
            },
            uninstall: candidate.uninstall,
        })
        .collect();
    records.sort_by(|left, right| {
        version_sort_key(&right.studio.version)
            .cmp(&version_sort_key(&left.studio.version))
            .then_with(|| {
                left.studio
                    .executable_path
                    .cmp(&right.studio.executable_path)
            })
    });
    records
}

fn normalized_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn version_sort_key(version: &str) -> (Vec<u32>, u8, u32) {
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(core, suffix)| (core, Some(suffix)));
    let numeric = core
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect();
    let (stage, sequence) = match prerelease {
        None => (2, u32::MAX),
        Some(suffix) if suffix.to_ascii_lowercase().starts_with("rc") => {
            (1, prerelease_sequence(suffix))
        }
        Some(suffix) => (0, prerelease_sequence(suffix)),
    };
    (numeric, stage, sequence)
}

fn prerelease_sequence(suffix: &str) -> u32 {
    suffix
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_from_executable, candidate_from_registry_entry, discover, display_icon_path,
        merge_candidates, normalized_version, parse_uninstall_command, scan_custom_path, Candidate,
        RegistryEntry,
    };
    use crate::models::{AppConfig, ContainerRuntime};
    use std::fs;
    use std::path::PathBuf;

    fn config(root: &std::path::Path) -> AppConfig {
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
            shared_directory: root.to_string_lossy().to_string(),
            windows_shared_directory: String::new(),
            freerdp_binary: String::new(),
            mendix_install_root: root
                .join("Program Files/Mendix")
                .to_string_lossy()
                .to_string(),
            mendix_data_root: root
                .join("ProgramData/Mendix")
                .to_string_lossy()
                .to_string(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn fake_studio(root: &std::path::Path, folder: &str) -> PathBuf {
        let executable = root.join(folder).join("modeler/studiopro.exe");
        fs::create_dir_all(executable.parent().expect("modeler parent")).expect("create modeler");
        fs::write(&executable, b"fake Studio Pro").expect("write executable");
        executable
    }

    #[test]
    fn parses_real_world_registry_values_without_invoking_a_shell() {
        let command = parse_uninstall_command(
            r#""C:\ProgramData\Mendix\11.12.2\uninst\unins000.exe" /SILENT /NORESTART"#,
        )
        .expect("uninstall command");
        assert_eq!(
            command.executable,
            PathBuf::from(r"C:\ProgramData\Mendix\11.12.2\uninst\unins000.exe")
        );
        assert_eq!(command.arguments, ["/SILENT", "/NORESTART"]);
        assert_eq!(
            display_icon_path(r#""C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe",0"#),
            Some(PathBuf::from(
                r"C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe"
            ))
        );
    }

    #[test]
    fn converts_registry_snapshots_and_rejects_unrelated_or_stale_entries() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let executable = fake_studio(temporary.path(), "11.12.2");
        let uninstaller = temporary.path().join("unins000.exe");
        fs::write(&uninstaller, b"uninstaller fixture").expect("write uninstaller fixture");
        let entry = RegistryEntry {
            display_name: "Mendix 11.12.2.81004".into(),
            publisher: "Mendix Technology B.V.".into(),
            display_version: Some("11.12.2.81004".into()),
            install_location: executable
                .parent()
                .and_then(|path| path.parent())
                .map(|path| path.to_string_lossy().to_string()),
            display_icon: Some(format!(r#""{}",0"#, executable.display())),
            quiet_uninstall: Some(format!(r#""{}" /SILENT /NORESTART"#, uninstaller.display())),
            uninstall: None,
        };

        let candidate = candidate_from_registry_entry(entry.clone()).expect("registry candidate");
        assert_eq!(candidate.version, "11.12.2");
        assert_eq!(candidate.executable, executable);
        assert_eq!(
            candidate.uninstall.expect("uninstall metadata").arguments,
            ["/SILENT", "/NORESTART"]
        );

        let stale_icon_with_valid_location = RegistryEntry {
            display_icon: Some(r#""C:\Missing\StudioPro.exe",0"#.into()),
            ..entry.clone()
        };
        assert_eq!(
            candidate_from_registry_entry(stale_icon_with_valid_location)
                .expect("InstallLocation fallback")
                .executable,
            executable
        );

        let unrelated = RegistryEntry {
            publisher: "Unrelated Publisher".into(),
            ..entry.clone()
        };
        assert!(candidate_from_registry_entry(unrelated).is_none());
        let deceptive = RegistryEntry {
            publisher: "Definitely Not Mendix Software".into(),
            ..entry.clone()
        };
        assert!(candidate_from_registry_entry(deceptive).is_none());

        let stale = RegistryEntry {
            display_icon: Some(r#""C:\Missing\StudioPro.exe",0"#.into()),
            install_location: Some(r"C:\Missing".into()),
            ..entry
        };
        assert!(candidate_from_registry_entry(stale).is_none());
    }

    #[test]
    fn scans_standard_and_portable_custom_layouts() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let standard = fake_studio(temporary.path(), "11.12.2");
        let portable = fake_studio(temporary.path(), "10.24.9.81004");

        let custom = scan_custom_path(temporary.path(), &config);
        assert_eq!(custom.len(), 2);
        assert!(custom.iter().any(|item| item.executable == standard));
        assert!(custom.iter().any(|item| item.executable == portable));
        assert_eq!(
            normalized_version("Mendix 10.24.9.81004"),
            Some("10.24.9".into())
        );
        assert_eq!(
            normalized_version("Mendix 11.6.0-beta.1"),
            Some("11.6.0-beta.1".into())
        );
    }

    #[test]
    fn registry_priority_wins_while_safe_uninstall_metadata_is_preserved() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let executable = fake_studio(temporary.path(), "11.12.2");
        let fallback = candidate_from_executable(&executable, "Standard path", 30, None, &config)
            .expect("fallback candidate");
        let registry = Candidate {
            source: "Windows Registry".into(),
            priority: 100,
            display_name: "Mendix 11.12.2".into(),
            ..fallback.clone()
        };

        let records = merge_candidates(vec![fallback, registry]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].studio.source, "Windows Registry");
    }

    #[test]
    fn exposes_one_deterministic_installation_per_public_version() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let standard_executable = fake_studio(&temporary.path().join("standard"), "11.12.2");
        let custom_executable = fake_studio(&temporary.path().join("portable"), "11.12.2");
        let standard =
            candidate_from_executable(&standard_executable, "Standard path", 30, None, &config)
                .expect("standard candidate");
        let custom =
            candidate_from_executable(&custom_executable, "Custom path", 70, None, &config)
                .expect("custom candidate");

        let records = merge_candidates(vec![standard, custom]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].studio.source, "Custom path");
        assert_eq!(
            PathBuf::from(&records[0].studio.executable_path),
            custom_executable
        );
    }

    #[test]
    #[ignore = "reads installed Studio Pro and Windows registry state without changing it"]
    fn live_discovers_installed_windows_versions_and_official_uninstall_metadata() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let mut config = config(temporary.path());
        config.mendix_install_root = std::env::var("ProgramW6432")
            .map(|root| PathBuf::from(root).join("Mendix"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\Program Files\Mendix"))
            .to_string_lossy()
            .to_string();
        config.mendix_data_root = std::env::var("ProgramData")
            .map(|root| PathBuf::from(root).join("Mendix"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\ProgramData\Mendix"))
            .to_string_lossy()
            .to_string();

        let records = discover(&config);
        assert!(
            !records.is_empty(),
            "no installed Studio Pro was discovered"
        );
        for mut record in records {
            assert!(PathBuf::from(&record.studio.executable_path).is_file());
            if record.studio.source == "Windows Registry" {
                assert!(record.uninstall.is_some());
                assert!(record.studio.removable);
                super::super::secure_uninstall_executable(&config, &mut record, || {
                    super::super::process::system_executable("msiexec.exe")
                })
                .expect("safe official uninstall metadata");
            }
        }
    }
}
