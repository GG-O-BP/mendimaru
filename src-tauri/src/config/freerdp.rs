use crate::process::{self, CommandPolicy};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

pub(super) fn discover() -> Option<String> {
    let directories = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut candidates = Vec::new();
    for name in ["xfreerdp3", "xfreerdp"] {
        for directory in &directories {
            let path = directory.join(name);
            if path.is_absolute() && path.is_file() && !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    for path in [
        "/usr/bin/xfreerdp3",
        "/usr/bin/xfreerdp",
        "/usr/local/bin/xfreerdp",
    ] {
        let path = PathBuf::from(path);
        if path.is_file() && !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    select(&candidates).map(|path| path.to_string_lossy().into_owned())
}

fn select(candidates: &[PathBuf]) -> Option<PathBuf> {
    // An old xfreerdp3 must not hide a supported xfreerdp later in PATH.
    // Keep the first candidate for an actionable diagnostic if none qualifies.
    let started = Instant::now();
    for candidate in candidates.iter().take(16) {
        let remaining = Duration::from_secs(6).saturating_sub(started.elapsed());
        let Some(policy) = CommandPolicy::within_budget(remaining, Duration::from_secs(2), 4096)
        else {
            break;
        };
        let mut command = Command::new(candidate);
        command.arg("/version");
        let Ok(output) = process::output_sync(command, policy, None, "FreeRDP discovery") else {
            continue;
        };
        let raw = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        if output.status.success()
            && String::from_utf8_lossy(raw)
                .split_whitespace()
                .any(crate::winboat::freerdp_version_supported)
        {
            return Some(candidate.clone());
        }
    }
    candidates.first().cloned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(root: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn supported_client_is_not_hidden_by_an_old_binary_name() {
        let root = tempfile::tempdir().unwrap();
        let old = executable(
            root.path(),
            "xfreerdp3",
            "echo 'This is FreeRDP version 3.15.0'",
        );
        let new = executable(
            root.path(),
            "xfreerdp",
            "echo 'This is FreeRDP version 3.32.2'",
        );
        assert_eq!(select(&[old.clone(), new.clone()]), Some(new.clone()));
        assert_eq!(select(&[new.clone(), old.clone()]), Some(new));
        assert_eq!(select(std::slice::from_ref(&old)), Some(old));
    }

    #[test]
    fn failed_or_malformed_version_probes_do_not_hide_a_supported_client() {
        let root = tempfile::tempdir().unwrap();
        let failed = executable(
            root.path(),
            "failed",
            "echo 'FreeRDP version 3.32.2'; exit 1",
        );
        let malformed = executable(root.path(), "malformed", "echo 'FreeRDP version 4.0.0'");
        let good = executable(
            root.path(),
            "good",
            "echo 'This is FreeRDP version 3.24.0' >&2",
        );
        assert_eq!(select(&[failed, malformed, good.clone()]), Some(good));
        assert_eq!(select(&[]), None);
    }
}
