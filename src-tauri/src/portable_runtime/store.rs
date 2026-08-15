use crate::app_paths::AppPaths;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const STORE_DIRECTORY: &str = "portable-runtime";
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct RuntimeLayout {
    root: PathBuf,
}

impl RuntimeLayout {
    pub(super) fn discover() -> Result<Self, String> {
        let paths = AppPaths::discover_for_cli()?;
        paths.ensure_cache_directory()?;
        let root = direct_child(paths.cache_directory(), STORE_DIRECTORY)?;
        ensure_private_directory(&root)?;
        for name in [
            "artifacts",
            "downloads",
            "projects",
            "sessions",
            "toolchains",
        ] {
            ensure_private_directory(&direct_child(&root, name)?)?;
        }
        Ok(Self { root })
    }

    #[cfg(test)]
    pub(super) fn for_tests(root: PathBuf) -> Result<Self, String> {
        ensure_private_directory(&root)?;
        for name in [
            "artifacts",
            "downloads",
            "projects",
            "sessions",
            "toolchains",
        ] {
            ensure_private_directory(&direct_child(&root, name)?)?;
        }
        Ok(Self { root })
    }

    pub(super) fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub(super) fn toolchain_directory(
        &self,
        platform: &str,
        version: &str,
    ) -> Result<PathBuf, String> {
        let platform_root = checked_directory(&self.root.join("toolchains"), platform)?;
        checked_directory(&platform_root, version)
    }

    pub(super) fn project_directory(&self, project_key: &str) -> Result<PathBuf, String> {
        validate_digest(project_key)?;
        checked_directory(&self.root.join("projects"), project_key)
    }

    pub(super) fn project_builds_directory(&self, project_key: &str) -> Result<PathBuf, String> {
        let project = self.project_directory(project_key)?;
        checked_directory(&project, "builds")
    }

    pub(super) fn build_directory(
        &self,
        project_key: &str,
        build_key: &str,
    ) -> Result<PathBuf, String> {
        validate_digest(build_key)?;
        let builds = self.project_builds_directory(project_key)?;
        checked_directory(&builds, build_key)
    }

    pub(super) fn artifact_record(&self, artifact_id: &str) -> Result<PathBuf, String> {
        validate_identifier(artifact_id, "artifact")?;
        Ok(self
            .root
            .join("artifacts")
            .join(format!("{artifact_id}.json")))
    }

    pub(super) fn session_directory(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_identifier(session_id, "runtime")?;
        checked_directory(&self.root.join("sessions"), session_id)
    }

    pub(super) fn session_record(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_identifier(session_id, "runtime")?;
        Ok(self
            .root
            .join("sessions")
            .join(session_id)
            .join("session.json"))
    }
}

pub(super) fn checked_directory(parent: &Path, component: &str) -> Result<PathBuf, String> {
    validate_component(component)?;
    let path = direct_child(parent, component)?;
    ensure_private_directory(&path)?;
    Ok(path)
}

pub(super) fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            ensure_direct(parent, "parent directory")?;
        }
    }
    fs::create_dir_all(path)
        .map_err(|error| format!("could not create the runtime cache directory: {error}"))?;
    ensure_direct(path, "runtime cache directory")?;
    set_directory_permissions(path)?;
    Ok(())
}

pub(super) fn create_private_file(path: &Path, truncate: bool) -> Result<File, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the private file has no parent directory".to_string())?;
    ensure_direct(parent, "private file parent")?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("the private file target is not a direct regular file".to_string());
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(truncate);
    let file = options
        .open(path)
        .map_err(|error| format!("could not open the private runtime file: {error}"))?;
    set_file_permissions(path)?;
    Ok(file)
}

pub(super) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the runtime record has no parent directory".to_string())?;
    ensure_direct(parent, "runtime record parent")?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("the runtime record target is not a direct regular file".to_string());
        }
    }
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record"),
        random_suffix()?
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("could not create the runtime record: {error}"))?;
    set_file_permissions(&temporary)?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("could not serialize the runtime record: {error}"))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist the runtime record: {error}"))?;
    drop(file);
    atomic_replace(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(())
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    read_json_bounded(path, MAX_RECORD_BYTES)
}

pub(super) fn read_json_bounded<T: DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect the runtime record: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the runtime record is not a direct regular file".to_string());
    }
    if metadata.len() > max_bytes {
        return Err("the runtime record exceeds the size limit".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(max_bytes + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("could not read the runtime record: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err("the runtime record exceeds the size limit".to_string());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse the runtime record: {error}"))
}

pub(super) fn validate_identifier(value: &str, prefix: &str) -> Result<(), String> {
    let Some(suffix) = value.strip_prefix(&format!("{prefix}_")) else {
        return Err("the runtime identifier is invalid".to_string());
    };
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the runtime identifier is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the runtime cache digest is invalid".to_string());
    }
    Ok(())
}

fn direct_child(parent: &Path, component: &str) -> Result<PathBuf, String> {
    validate_component(component)?;
    Ok(parent.join(component))
}

fn validate_component(component: &str) -> Result<(), String> {
    let path = Path::new(component);
    if component.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("the runtime cache path component is invalid".to_string());
    }
    Ok(())
}

fn ensure_direct(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect the {label}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("the {label} must be a direct directory"));
    }
    Ok(())
}

fn random_suffix() -> Result<String, String> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not generate a runtime record nonce: {error}"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not protect the runtime cache directory: {error}"))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not protect the runtime cache file: {error}"))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("could not replace the runtime record atomically: {error}"))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
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
        Err(format!(
            "could not replace the runtime record atomically: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_layout_rejects_untrusted_identifiers_and_round_trips_private_json() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let layout = RuntimeLayout::for_tests(temporary.path().join("runtime")).expect("layout");
        assert!(layout.session_directory("../../escape").is_err());
        assert!(layout.artifact_record("artifact_short").is_err());

        let session_id = format!("runtime_{}", "ab".repeat(16));
        layout
            .session_directory(&session_id)
            .expect("validated session directory");
        let path = layout.session_record(&session_id).expect("record path");
        write_json(&path, &serde_json::json!({ "state": "ready" })).expect("write record");
        let value: serde_json::Value = read_json(&path).expect("read record");
        assert_eq!(value["state"], "ready");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
