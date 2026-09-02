use serde::Deserialize;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

const MAX_LOCK_BYTES: u64 = 512;
const MAX_PROJECT_TREE_DEPTH: usize = 8;
const MAX_LOCKS_PER_CLEANUP: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "PascalCase")]
struct ProjectLock {
    session_id: String,
    process_id: u32,
}

pub(super) fn remove_dead_process_lock(
    project_directory: &Path,
    dead_process_id: u32,
) -> Result<usize, String> {
    let mut removed = 0;
    for entry in WalkDir::new(project_directory)
        .max_depth(MAX_PROJECT_TREE_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if removed >= MAX_LOCKS_PER_CLEANUP {
            break;
        }
        let path = entry.path();
        if !path_is_direct_lock(path) {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(lock) = serde_json::from_slice::<ProjectLock>(&bytes) else {
            continue;
        };
        if lock.process_id != dead_process_id || !valid_lock_session_id(&lock.session_id) {
            continue;
        }
        // Revalidate immediately before deletion. A replacement or symlink is
        // deliberately left untouched rather than guessed at.
        if !path_is_direct_lock(path) {
            continue;
        }
        if fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn path_is_direct_lock(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.len() <= MAX_LOCK_BYTES
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.len() > ".mpr.lock".len() && value.ends_with(".mpr.lock"))
}

fn valid_lock_session_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let expected_hex = !matches!(index, 8 | 13 | 18 | 23);
        let separator = *byte == b'-';
        let hexadecimal = byte.is_ascii_hexdigit();
        if (expected_hex && !hexadecimal) || (!expected_hex && !separator) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::remove_dead_process_lock;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::process;

    #[test]
    fn removes_only_the_dead_processes_lock() {
        let temporary = tempfile::tempdir().expect("temporary project");
        write_lock(
            temporary.path(),
            "dead.mpr.lock",
            r#"{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b7","ProcessId":4242}"#,
        );
        write_lock(
            temporary.path(),
            "live.mpr.lock",
            r#"{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b8","ProcessId":9000}"#,
        );
        write_lock(
            temporary.path(),
            "malformed.mpr.lock",
            r#"{"SessionId":"not-a-uuid","ProcessId":4242}"#,
        );

        let removed =
            remove_dead_process_lock(temporary.path(), 4242).expect("dead lock cleanup succeeds");
        assert_eq!(removed, 1);
        assert!(!temporary.path().join("dead.mpr.lock").exists());
        assert!(temporary.path().join("live.mpr.lock").exists());
        assert!(temporary.path().join("malformed.mpr.lock").exists());
    }

    #[test]
    fn preserves_links_and_unexpected_files() {
        let temporary = tempfile::tempdir().expect("temporary project");
        let target = temporary.path().join("target.mpr.lock");
        fs::write(
            &target,
            r#"{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b7","ProcessId":4242}"#,
        )
        .expect("target lock");
        let link = temporary.path().join("linked.mpr.lock");
        symlink(&target, &link).expect("linked lock");
        fs::write(temporary.path().join("large.mpr.lock"), "x".repeat(513)).expect("large lock");

        let removed =
            remove_dead_process_lock(temporary.path(), 4242).expect("safe lock cleanup succeeds");
        assert_eq!(removed, 1);
        assert!(link.symlink_metadata().is_ok());
        assert!(temporary.path().join("large.mpr.lock").exists());
    }

    #[test]
    fn does_not_treat_a_live_host_process_as_dead() {
        let temporary = tempfile::tempdir().expect("temporary project");
        let current = process::id();
        write_lock(
            temporary.path(),
            "current.mpr.lock",
            &format!(
                r#"{{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b7","ProcessId":{current}}}"#
            ),
        );
        let different = current.wrapping_add(1);
        let removed = remove_dead_process_lock(temporary.path(), different)
            .expect("lock cleanup completes conservatively");
        assert_eq!(removed, 0);
        assert!(temporary.path().join("current.mpr.lock").exists());
    }

    fn write_lock(directory: &Path, name: &str, content: &str) {
        fs::write(directory.join(name), content).expect("lock fixture");
    }
}
