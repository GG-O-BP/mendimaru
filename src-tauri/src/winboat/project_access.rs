use crate::models::{AppConfig, ProjectLocation};
use crate::projects::{linux_path_to_windows_share, ProjectSelection};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const STORE_FILE_NAME: &str = "protected-project-sessions.json";
const STORE_SCHEMA_VERSION: &str = "1.0.0";
const MAX_STORE_BYTES: u64 = 128 * 1024;
const MAX_RECORDS: usize = 64;
const SHARE_PREFIX: &str = "mpr-";
const SHARE_DIGEST_CHARS: usize = 16;
const MAX_SHARE_NAME_BYTES: usize = SHARE_PREFIX.len() + SHARE_DIGEST_CHARS;
const PROJECT_READY_TIMEOUT_SECONDS: u64 = 30;

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectAccessProviderKind {
    ConfiguredWorkspace,
    FreeRdpDrive,
}

#[derive(Debug, Clone)]
pub(super) struct ProjectAccessLease {
    guest_project_path: String,
    provider_kind: ProjectAccessProviderKind,
    share_identity: Option<String>,
    project_digest: String,
    drive: Option<FreeRdpDrive>,
    reconnectable: bool,
}

#[derive(Debug, Clone)]
struct FreeRdpDrive {
    share_name: String,
    host_root: PathBuf,
}

impl ProjectAccessLease {
    pub(super) fn guest_project_path(&self) -> &str {
        &self.guest_project_path
    }

    pub(super) const fn provider_kind(&self) -> ProjectAccessProviderKind {
        self.provider_kind
    }

    pub(super) fn share_identity(&self) -> Option<&str> {
        self.share_identity.as_deref()
    }

    pub(super) fn project_digest(&self) -> &str {
        &self.project_digest
    }

    pub(super) const fn reconnectable(&self) -> bool {
        self.reconnectable
    }

    pub(super) const fn readiness_timeout_seconds(&self) -> u64 {
        PROJECT_READY_TIMEOUT_SECONDS
    }

    pub(super) fn freerdp_drive_argument(&self) -> Result<Option<String>, String> {
        self.drive.as_ref().map(FreeRdpDrive::argument).transpose()
    }
}

impl FreeRdpDrive {
    fn argument(&self) -> Result<String, String> {
        validate_share_name(&self.share_name)?;
        let root = self
            .host_root
            .to_str()
            .ok_or_else(|| crate::tr!("error-project-path-encoding"))?;
        validate_host_root_argument(root)?;
        Ok(format!("/drive:{},{}", self.share_name, root))
    }
}

trait ProjectAccessProvider {
    fn prepare(
        &self,
        config: &AppConfig,
        selection: &ProjectSelection,
    ) -> Result<ProjectAccessLease, String>;
}

struct ConfiguredWorkspaceProvider;
struct FreeRdpDriveProvider;

impl ProjectAccessProvider for ConfiguredWorkspaceProvider {
    fn prepare(
        &self,
        config: &AppConfig,
        selection: &ProjectSelection,
    ) -> Result<ProjectAccessLease, String> {
        let guest_project_path = linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            selection.mpr_path(),
            &config.windows_shared_directory,
        )?;
        Ok(ProjectAccessLease {
            guest_project_path,
            provider_kind: ProjectAccessProviderKind::ConfiguredWorkspace,
            share_identity: None,
            project_digest: selection.project_digest(),
            drive: None,
            reconnectable: true,
        })
    }
}

impl ProjectAccessProvider for FreeRdpDriveProvider {
    fn prepare(
        &self,
        _config: &AppConfig,
        selection: &ProjectSelection,
    ) -> Result<ProjectAccessLease, String> {
        let project_digest = selection.project_digest();
        let share_name = format!("{SHARE_PREFIX}{}", &project_digest[..SHARE_DIGEST_CHARS]);
        validate_share_name(&share_name)?;
        let file_name = selection
            .mpr_path()
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| crate::tr!("error-project-path-encoding"))?;
        if file_name
            .chars()
            .any(|character| r#"<>:"/\|?*"#.contains(character))
        {
            return Err(crate::tr!("error-project-path-unsupported"));
        }
        let guest_project_path = format!(r"\\tsclient\{share_name}\{file_name}");
        let drive = FreeRdpDrive {
            share_name: share_name.clone(),
            host_root: selection.directory().to_path_buf(),
        };
        let _ = drive.argument()?;
        Ok(ProjectAccessLease {
            guest_project_path,
            provider_kind: ProjectAccessProviderKind::FreeRdpDrive,
            share_identity: Some(share_name),
            project_digest,
            drive: Some(drive),
            reconnectable: false,
        })
    }
}

pub(super) fn prepare(
    config: &AppConfig,
    selection: &ProjectSelection,
) -> Result<ProjectAccessLease, String> {
    match selection.location() {
        ProjectLocation::ConfiguredWorkspace => {
            ConfiguredWorkspaceProvider.prepare(config, selection)
        }
        ProjectLocation::ExplicitHostSelection => FreeRdpDriveProvider.prepare(config, selection),
    }
}

fn validate_share_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_SHARE_NAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(crate::tr!("error-project-share-identity"));
    }
    Ok(())
}

fn validate_host_root_argument(value: &str) -> Result<(), String> {
    if value.is_empty()
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

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedSessionStore {
    schema_version: String,
    records: Vec<ProtectedSessionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedSessionRecord {
    session_id: String,
    project_digest: String,
    share_identity: String,
    version: String,
    created_at: DateTime<Utc>,
}

pub(super) fn remember_protected_session(
    config: &AppConfig,
    session_id: &str,
    version: &str,
    lease: &ProjectAccessLease,
) -> Result<(), String> {
    match lease.provider_kind() {
        ProjectAccessProviderKind::ConfiguredWorkspace if lease.reconnectable() => return Ok(()),
        ProjectAccessProviderKind::FreeRdpDrive if !lease.reconnectable() => {}
        _ => return Err(crate::tr!("error-project-session-metadata")),
    }
    let share_identity = lease
        .share_identity()
        .ok_or_else(|| crate::tr!("error-project-share-identity"))?;
    validate_session_record(session_id, version, lease.project_digest(), share_identity)?;
    let _guard = store_lock()?;
    let path = store_path(config)?;
    let mut store = load_store(&path)?;
    store
        .records
        .retain(|record| record.session_id != session_id);
    store.records.push(ProtectedSessionRecord {
        session_id: session_id.to_string(),
        project_digest: lease.project_digest().to_string(),
        share_identity: share_identity.to_string(),
        version: version.to_string(),
        created_at: Utc::now(),
    });
    store
        .records
        .sort_by_key(|record| std::cmp::Reverse(record.created_at));
    store.records.truncate(MAX_RECORDS);
    save_store(&path, &store)
}

pub(super) fn forget_protected_session(config: &AppConfig, session_id: &str) {
    let Ok(_guard) = store_lock() else {
        return;
    };
    let Ok(path) = store_path(config) else {
        return;
    };
    let Ok(mut store) = load_store(&path) else {
        return;
    };
    let previous = store.records.len();
    store
        .records
        .retain(|record| record.session_id != session_id);
    if store.records.len() != previous {
        let _ = save_store(&path, &store);
    }
}

pub(super) fn protected_session_ids(config: &AppConfig) -> Result<HashSet<String>, String> {
    let _guard = store_lock()?;
    let path = store_path(config)?;
    load_store(&path).map(|store| {
        store
            .records
            .into_iter()
            .map(|record| record.session_id)
            .collect()
    })
}

pub(super) fn retain_live_protected_sessions(
    config: &AppConfig,
    live_session_ids: &HashSet<String>,
) -> Result<(), String> {
    let _guard = store_lock()?;
    let path = store_path(config)?;
    let mut store = load_store(&path)?;
    let previous = store.records.len();
    store
        .records
        .retain(|record| live_session_ids.contains(&record.session_id));
    if store.records.len() != previous {
        save_store(&path, &store)?;
    }
    Ok(())
}

fn validate_session_record(
    session_id: &str,
    version: &str,
    project_digest: &str,
    share_identity: &str,
) -> Result<(), String> {
    let session = session_id
        .strip_prefix("studio-")
        .and_then(|value| value.split_once('-'))
        .filter(|(process_id, ticks)| {
            process_id.parse::<u32>().is_ok_and(|value| value > 0)
                && ticks.parse::<u64>().is_ok_and(|value| value > 0)
        });
    if session.is_none()
        || project_digest.len() != 64
        || !project_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(crate::tr!("error-project-session-metadata"));
    }
    crate::platform::validate_version(version)?;
    validate_share_name(share_identity)
}

fn store_path(config: &AppConfig) -> Result<PathBuf, String> {
    let directory = super::studio::secure_shared_directory(config, ".mendimaru")?;
    Ok(directory.join(STORE_FILE_NAME))
}

fn load_store(path: &Path) -> Result<ProtectedSessionStore, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProtectedSessionStore {
                schema_version: STORE_SCHEMA_VERSION.to_string(),
                records: Vec::new(),
            });
        }
        Err(error) => return Err(crate::tr!("error-project-session-store", error = error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_STORE_BYTES
    {
        return Err(crate::tr!(
            "error-project-session-store",
            error = "the metadata file is unsafe"
        ));
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_STORE_BYTES + 1).read_to_end(&mut content))
        .map_err(|error| crate::tr!("error-project-session-store", error = error))?;
    if content.len() as u64 > MAX_STORE_BYTES {
        return Err(crate::tr!(
            "error-project-session-store",
            error = "the metadata file is too large"
        ));
    }
    let store: ProtectedSessionStore = serde_json::from_slice(&content)
        .map_err(|error| crate::tr!("error-project-session-store", error = error))?;
    validate_store(&store)?;
    Ok(store)
}

fn validate_store(store: &ProtectedSessionStore) -> Result<(), String> {
    if store.schema_version != STORE_SCHEMA_VERSION || store.records.len() > MAX_RECORDS {
        return Err(crate::tr!(
            "error-project-session-store",
            error = "the metadata shape is unsupported"
        ));
    }
    let mut identifiers = HashSet::new();
    for record in &store.records {
        validate_session_record(
            &record.session_id,
            &record.version,
            &record.project_digest,
            &record.share_identity,
        )?;
        if !identifiers.insert(&record.session_id) {
            return Err(crate::tr!(
                "error-project-session-store",
                error = "duplicate session metadata"
            ));
        }
    }
    Ok(())
}

fn save_store(path: &Path, store: &ProtectedSessionStore) -> Result<(), String> {
    validate_store(store)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(crate::tr!(
                "error-project-session-store",
                error = "the metadata file is unsafe"
            ));
        }
    }
    let content = serde_json::to_vec_pretty(store)
        .map_err(|error| crate::tr!("error-project-session-store", error = error))?;
    if content.len() as u64 > MAX_STORE_BYTES {
        return Err(crate::tr!(
            "error-project-session-store",
            error = "the metadata file is too large"
        ));
    }
    let temporary = temporary_path(path)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(&content)?;
            file.sync_all()
        })
        .and_then(|()| replace_file(&temporary, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(crate::tr!("error-project-session-store", error = error));
    }
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or_else(|| {
        crate::tr!(
            "error-project-session-store",
            error = "the metadata file has no parent"
        )
    })?;
    for _ in 0..8 {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)
            .map_err(|error| crate::tr!("error-project-session-store", error = error))?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let candidate = parent.join(format!(".{STORE_FILE_NAME}.{suffix}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(crate::tr!(
        "error-project-session-store",
        error = "could not allocate a metadata transaction"
    ))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn store_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| crate::tr!("error-project-session-store", error = "lock poisoned"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{
        prepare, protected_session_ids, remember_protected_session, retain_live_protected_sessions,
        validate_host_root_argument, ProjectAccessProviderKind,
    };
    use crate::models::{AppConfig, ContainerRuntime};
    use crate::projects::validate_project_selection;
    use std::collections::HashSet;
    use std::fs;

    fn config(workspace: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: workspace.to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    #[test]
    fn providers_share_one_guest_path_and_lifetime_contract() {
        let workspace = tempfile::tempdir().expect("workspace");
        let inside_directory = workspace.path().join("Inside project");
        fs::create_dir(&inside_directory).expect("inside directory");
        let inside = inside_directory.join("Orders.mpr");
        fs::write(&inside, b"mpr").expect("inside project");

        let outside = tempfile::tempdir().expect("outside");
        let outside_directory = outside.path().join("한국어 project 'quoted'");
        fs::create_dir(&outside_directory).expect("outside directory");
        let outside_mpr = outside_directory.join("注文 project.mpr");
        fs::write(&outside_mpr, b"mpr").expect("outside project");
        let config = config(workspace.path());

        let configured = prepare(
            &config,
            &validate_project_selection(&config, &inside).expect("inside selection"),
        )
        .expect("configured provider");
        assert_eq!(
            configured.provider_kind(),
            ProjectAccessProviderKind::ConfiguredWorkspace
        );
        assert!(configured.reconnectable());
        assert!(configured
            .freerdp_drive_argument()
            .expect("argument")
            .is_none());

        let explicit = prepare(
            &config,
            &validate_project_selection(&config, &outside_mpr).expect("outside selection"),
        )
        .expect("explicit provider");
        assert_eq!(
            explicit.provider_kind(),
            ProjectAccessProviderKind::FreeRdpDrive
        );
        assert!(!explicit.reconnectable());
        assert!(explicit
            .guest_project_path()
            .starts_with(r"\\tsclient\mpr-"));
        let share_identity = explicit.share_identity().expect("share identity");
        assert!(share_identity.is_ascii());
        assert!(share_identity.len() <= super::MAX_SHARE_NAME_BYTES);
        let argument = explicit
            .freerdp_drive_argument()
            .expect("argument")
            .expect("drive");
        assert!(argument.starts_with("/drive:mpr-"));
        assert!(argument.contains("한국어 project 'quoted'"));
        assert!(!argument.contains('\n'));
    }

    #[test]
    fn drive_argument_rejects_every_stdin_or_freerdp_delimiter() {
        for value in [
            "/tmp/project,other",
            "/tmp/project\\alias",
            "/tmp/project\"quoted",
            "/tmp/project\noption",
            "/tmp/project\roption",
            "",
        ] {
            assert!(
                validate_host_root_argument(value).is_err(),
                "unsafe root was accepted: {value:?}"
            );
        }
        validate_host_root_argument("/tmp/한국어 project 'quoted'").expect("safe UTF-8 root");
    }

    #[test]
    fn protected_session_store_contains_no_host_path_and_prunes_ended_sessions() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let project_directory = outside.path().join("Secret Customer");
        fs::create_dir(&project_directory).expect("project directory");
        let mpr = project_directory.join("Orders.mpr");
        fs::write(&mpr, b"mpr").expect("project");
        let config = config(workspace.path());
        let lease = prepare(
            &config,
            &validate_project_selection(&config, &mpr).expect("selection"),
        )
        .expect("lease");
        let session_id = "studio-4242-638908128000000000";

        remember_protected_session(&config, session_id, "11.12.2", &lease)
            .expect("remember protected session");
        assert!(protected_session_ids(&config)
            .expect("protected sessions")
            .contains(session_id));
        let serialized = fs::read_to_string(
            workspace
                .path()
                .join(".mendimaru/protected-project-sessions.json"),
        )
        .expect("metadata");
        assert!(!serialized.contains("Secret Customer"));
        assert!(!serialized.contains(&mpr.to_string_lossy().to_string()));

        retain_live_protected_sessions(&config, &HashSet::new()).expect("prune sessions");
        assert!(protected_session_ids(&config)
            .expect("protected sessions")
            .is_empty());
    }
}
