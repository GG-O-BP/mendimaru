use std::path::PathBuf;

const ROOT_ENVIRONMENT_VARIABLE: &str = "MENDIMARU_E2E_ROOT";
const ROOT_MARKER_FILE: &str = ".mendimaru-e2e-root";
const ROOT_MARKER_CONTENT: &str = "mendimaru isolated native e2e\n";

pub(crate) fn require_isolated_root() -> Result<PathBuf, String> {
    let configured = std::env::var_os(ROOT_ENVIRONMENT_VARIABLE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{ROOT_ENVIRONMENT_VARIABLE} is required for an e2e build"))?;
    if !configured.is_absolute() || !configured.is_dir() {
        return Err(format!(
            "{ROOT_ENVIRONMENT_VARIABLE} must name an existing absolute directory"
        ));
    }
    let canonical = configured
        .canonicalize()
        .map_err(|error| format!("failed to inspect {ROOT_ENVIRONMENT_VARIABLE}: {error}"))?;
    let canonical = display_path(canonical);
    let marker = std::fs::read_to_string(canonical.join(ROOT_MARKER_FILE))
        .map_err(|_| format!("{ROOT_ENVIRONMENT_VARIABLE} is missing its safety marker"))?;
    if marker != ROOT_MARKER_CONTENT {
        return Err(format!(
            "{ROOT_ENVIRONMENT_VARIABLE} has an invalid safety marker"
        ));
    }
    Ok(canonical)
}

fn display_path(path: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path
}

pub(crate) fn directory(name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("invalid e2e directory name".to_string());
    }
    Ok(require_isolated_root()?.join(name))
}

#[cfg(target_os = "windows")]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BoundedProcessCleanupResult {
    failure_kind: &'static str,
    process_ids: Vec<u32>,
}

#[cfg(target_os = "windows")]
#[tauri::command]
pub(crate) async fn e2e_bounded_process_cleanup() -> Result<BoundedProcessCleanupResult, String> {
    use crate::process::{self, CancellationToken, CommandFailureKind, CommandPolicy};
    use std::time::Duration;
    use tokio::process::Command;

    const SCRIPT: &str = r#"
const { spawn } = require("node:child_process");
const { writeFileSync } = require("node:fs");
const child = spawn(process.env.MENDIMARU_E2E_CHILD, ["-t", "127.0.0.1"], {
  stdio: "ignore",
  windowsHide: true,
});
writeFileSync(
  process.env.MENDIMARU_E2E_PID_FILE,
  `${process.pid}\n${child.pid}\n`,
  { encoding: "ascii" },
);
setInterval(() => {}, 100);
"#;

    let root = require_isolated_root()?;
    let pid_file = root.join("bounded-process.pids");
    match tokio::fs::remove_file(&pid_file).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("failed to reset the e2e PID fixture: {error}")),
    }
    let node = std::env::var_os("MENDIMARU_E2E_NODE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .ok_or_else(|| "MENDIMARU_E2E_NODE must name the running Node executable".to_string())?;
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| "SystemRoot is unavailable for the Windows e2e fixture".to_string())?;
    let child = system_root.join(r"System32\ping.exe");
    if !child.is_file() {
        return Err("the protected Windows e2e process fixtures are unavailable".to_string());
    }

    let mut command = Command::new(node);
    command
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .args(["-e", SCRIPT])
        .env("MENDIMARU_E2E_CHILD", child)
        .env("MENDIMARU_E2E_PID_FILE", &pid_file);
    let cancellation = CancellationToken::default();
    let trigger = cancellation.clone();
    let observed_pid_file = pid_file.clone();
    let observe_and_cancel = async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            match tokio::fs::read_to_string(&observed_pid_file).await {
                Ok(content) => {
                    let process_ids = content
                        .lines()
                        .map(str::parse::<u32>)
                        .collect::<Result<Vec<_>, _>>();
                    if process_ids.is_ok_and(|process_ids| {
                        process_ids.len() == 2 && !process_ids.contains(&0)
                    }) {
                        trigger.cancel();
                        return Ok::<(), String>(());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("failed to inspect the e2e PID fixture: {error}"))
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("the Windows e2e child did not publish its process IDs".to_string());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let (result, observed) = tokio::join!(
        process::output(
            command,
            CommandPolicy::new(Duration::from_secs(9), 1024),
            Some(&cancellation),
            "Windows e2e process cancellation",
        ),
        observe_and_cancel,
    );
    observed?;
    let failure = match result {
        Ok(_) => return Err("the Windows e2e process unexpectedly completed".to_string()),
        Err(failure) => failure,
    };
    if failure.kind() != CommandFailureKind::Cancelled {
        return Err(format!(
            "the Windows e2e process ended as {:?} instead of cancellation",
            failure.kind()
        ));
    }
    let process_ids = tokio::fs::read_to_string(&pid_file)
        .await
        .map_err(|error| format!("failed to read the e2e PID fixture: {error}"))?
        .lines()
        .map(|line| {
            line.parse::<u32>()
                .map_err(|_| "the Windows e2e PID fixture is invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if process_ids.len() != 2 || process_ids.contains(&0) {
        return Err("the Windows e2e fixture did not create one root and one child".to_string());
    }
    let _ = tokio::fs::remove_file(pid_file).await;
    Ok(BoundedProcessCleanupResult {
        failure_kind: "cancelled",
        process_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::directory;

    #[test]
    fn rejects_directory_traversal() {
        assert!(directory("../outside").is_err());
    }
}
