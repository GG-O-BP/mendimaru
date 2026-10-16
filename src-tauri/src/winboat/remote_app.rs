use super::operation::WindowsOperationFailure;
use super::project_access::ProjectAccessLease;
use super::rdp_trust::TrustAttempt;
use super::security::{
    bootstrap_drive_name, redirected_bootstrap_launcher, secure_powershell_launcher,
    OperationSecurity,
};
use crate::models::AppConfig;
use crate::process::{self, CommandPolicy};
use crate::projects::linux_path_to_windows_share;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;

#[cfg(test)]
pub(super) const FREERDP_CERTIFICATE_POLICY: &str = super::rdp_trust::FIRST_USE_POLICY;
pub(super) const HEADLESS_CONSOLE_HOST: &str = r"C:\Windows\System32\conhost.exe";
pub(super) const POWERSHELL_EXECUTABLE: &str =
    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
/// FreeRDP 3 handles SIGTERM by aborting the connection and disconnecting the
/// RDP session cleanly. A SIGKILL leaves a half-closed RemoteApp session that
/// the next connection can inherit (#227), so SIGKILL is only the fallback.
#[cfg(unix)]
const GRACEFUL_STOP: std::time::Duration = std::time::Duration::from_secs(2);

/// The only FreeRDP output that means the server certificate was rejected.
///
/// FreeRDP prints alarming text on a successful first use as well: the Windows
/// self-signed certificate never names `127.0.0.1` ("CERTIFICATE NAME
/// MISMATCH"), OpenSSL reports "Certificate verification failure
/// 'self-signed certificate'", and FreeRDP 3.15 prints its "REMOTE HOST
/// IDENTIFICATION HAS CHANGED ... Host key verification failed" block for an
/// unknown host (the new- and changed-host messages are swapped upstream).
/// TOFU still accepts the certificate after all of them. Only the TLS
/// handshake verdict below aborts the connection (#227).
const CERTIFICATE_REJECTION_MARKERS: &[&str] = &["certificate not trusted"];

pub(super) struct RemoteAppProcess {
    child: Child,
    diagnostics: Mutex<File>,
    _bootstrap: Option<tempfile::TempDir>,
    trust: Option<TrustAttempt>,
}

impl RemoteAppProcess {
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn from_test_child(child: Child) -> Self {
        Self {
            child,
            diagnostics: Mutex::new(tempfile::tempfile().expect("fixture diagnostics")),
            _bootstrap: None,
            trust: None,
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn from_test_child_with_trust(child: Child, trust: TrustAttempt) -> Self {
        let mut process = Self::from_test_child(child);
        process.trust = Some(trust);
        process
    }

    pub(super) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Stops FreeRDP: SIGTERM first so the RDP session is disconnected
    /// cleanly, SIGKILL only if it has not exited within a short grace period.
    pub(super) fn kill(&mut self) -> std::io::Result<()> {
        if let Ok(Some(_)) = self.child.try_wait() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // The child is ours and not yet reaped, so its PID cannot have
            // been reused by another process.
            let process_id = libc::pid_t::try_from(self.child.id()).unwrap_or(0);
            // SAFETY: kill(2) with a PID owned by this un-reaped child.
            if process_id > 0 && unsafe { libc::kill(process_id, libc::SIGTERM) } == 0 {
                let deadline = std::time::Instant::now() + GRACEFUL_STOP;
                while std::time::Instant::now() < deadline {
                    match self.child.try_wait() {
                        Ok(Some(_)) => return Ok(()),
                        Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
                        Err(_) => break,
                    }
                }
            }
        }
        self.child.kill()
    }

    pub(super) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }

    pub(super) fn certificate_failed(&self) -> bool {
        let diagnostics = self.diagnostics.lock().ok().and_then(|file| {
            let total = file.metadata().ok()?.len();
            let length = total.min(MAX_DIAGNOSTIC_BYTES as u64) as usize;
            let mut bytes = vec![0_u8; length];
            // Rejection can follow a large burst of connection warnings.
            // Inspect the bounded tail instead of freezing the first 64 KiB.
            let count = read_diagnostics_from(&file, &mut bytes, total - length as u64).ok()?;
            bytes.truncate(count);
            Some(String::from_utf8_lossy(&bytes).into_owned())
        });
        certificate_rejected(&diagnostics.unwrap_or_default())
    }

    /// Called on the first valid authenticated report, including Windows
    /// failures. Trust persistence failures must not be silently ignored.
    pub(super) fn confirm_trust(&mut self) -> Result<(), WindowsOperationFailure> {
        if let Some(trust) = &mut self.trust {
            trust.confirm()?;
        }
        Ok(())
    }
}

pub(super) fn certificate_rejected(diagnostics: &str) -> bool {
    let diagnostics = diagnostics.to_ascii_lowercase();
    CERTIFICATE_REJECTION_MARKERS
        .iter()
        .any(|marker| diagnostics.contains(marker))
}

impl Drop for RemoteAppProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.kill();
                let _ = self.child.wait();
            }
        }
    }
}

fn container_credentials(config: &AppConfig) -> Result<(String, String), String> {
    let mut command = Command::new(config.container_runtime.as_str());
    command
        .arg("inspect")
        .arg("--format")
        .arg("{{range .Config.Env}}{{println .}}{{end}}")
        .arg(&config.container_name);
    let output = process::output_sync(
        command,
        CommandPolicy::STATUS,
        None,
        "container credential inspection",
    )
    .map_err(|error| crate::tr!("error-windows-credentials-inspect", error = error))?;
    if !output.status.success() {
        return Err(crate::tr!("error-windows-account-missing"));
    }
    let environment = String::from_utf8_lossy(&output.stdout);
    let username = environment
        .lines()
        .find_map(|line| line.strip_prefix("USERNAME="))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| crate::tr!("error-windows-username-missing"))?;
    let password = environment
        .lines()
        .find_map(|line| line.strip_prefix("PASSWORD="))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| crate::tr!("error-windows-password-missing"))?;
    Ok((username, password))
}

pub(super) fn spawn_powershell_file(
    config: &AppConfig,
    script_path: &Path,
    label: &str,
    security: &OperationSecurity,
    project_access: Option<&ProjectAccessLease>,
) -> Result<RemoteAppProcess, WindowsOperationFailure> {
    let windows_script_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        script_path,
        &config.windows_shared_directory,
    )?;
    let launcher = secure_powershell_launcher(&windows_script_path, security);
    let bootstrap = tempfile::Builder::new()
        .prefix("mendimaru-auth-")
        .tempdir()
        .map_err(|_| "private RemoteApp bootstrap unavailable".to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(bootstrap.path().join("bootstrap.ps1"))
        .and_then(|mut f| f.write_all(launcher.as_bytes()).and_then(|()| f.sync_all()))
        .map_err(|_| "private RemoteApp bootstrap unavailable".to_string())?;
    let encoded = encode_powershell_script(&redirected_bootstrap_launcher(launcher.as_bytes()));
    let arguments = headless_powershell_arguments(&encoded);
    if arguments.encode_utf16().count() * 2 >= 16_000 {
        return Err(crate::tr!("error-remoteapp-command-too-long").into());
    }
    spawn_remote_app(
        config,
        HEADLESS_CONSOLE_HOST,
        Some(&arguments),
        label,
        project_access,
        &bootstrap_drive_name(launcher.as_bytes()),
        bootstrap,
    )
}

pub(super) fn powershell_encoded_arguments(encoded_command: &str) -> String {
    format!(
        "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy RemoteSigned -EncodedCommand {encoded_command}"
    )
}

pub(super) fn headless_powershell_arguments(encoded_command: &str) -> String {
    format!(
        "--headless {POWERSHELL_EXECUTABLE} {}",
        powershell_encoded_arguments(encoded_command)
    )
}

pub(super) fn encode_powershell_script(script: &str) -> String {
    let utf16le: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    BASE64_STANDARD.encode(utf16le)
}

fn spawn_remote_app(
    config: &AppConfig,
    executable_path: &str,
    app_arguments: Option<&str>,
    label: &str,
    project_access: Option<&ProjectAccessLease>,
    bootstrap_drive: &str,
    bootstrap: tempfile::TempDir,
) -> Result<RemoteAppProcess, WindowsOperationFailure> {
    let (username, password) = container_credentials(config)?;
    let safe_label: String = label
        .chars()
        .filter(|character| !matches!(character, ',' | '\'' | '"'))
        .collect();
    // HiDef maps each RemoteApp window to a GFX surface. FreeRDP 3.32.2
    // recursively takes the RAIL window lock while repainting those surfaces
    // (xf_rail_window_common -> xf_AppWindowRepaintFromSurface), then aborts.
    // Pair ordinary RAIL with the bitmap pipeline: GFX without HiDef leaves
    // the X11 window pixmaps unpainted.
    let mut remote_app = format!("/app:program:{executable_path},name:{safe_label},hidef:off");
    if let Some(arguments) = app_arguments.filter(|arguments| !arguments.is_empty()) {
        remote_app.push_str(",cmd:");
        remote_app.push_str(arguments);
    }
    let trust_store = app_freerdp_config_directory()?;
    let trust = TrustAttempt::prepare(config, &trust_store)?;
    let certificate_policy = trust.certificate_argument();
    let mut arguments = vec![
        format!("/u:{username}"),
        format!("/p:{password}"),
        format!("/v:{}", config.rdp_host),
        format!("/port:{}", config.rdp_port),
        certificate_policy,
        "/log-level:WARN".to_string(),
        "+clipboard".to_string(),
        "/sound:sys:pulse".to_string(),
        "/microphone:sys:pulse".to_string(),
        "/floatbar".to_string(),
        "/compression".to_string(),
        "/network:lan".to_string(),
        "/tune:FreeRDP_SupportGraphicsPipeline:FALSE".to_string(),
        "/sec:tls".to_string(),
        "-wallpaper".to_string(),
        "/scale-desktop:100".to_string(),
        format!("/wm-class:mendimaru-{}", css_slug(&safe_label)),
    ];
    if let Some(argument) = project_access
        .map(ProjectAccessLease::freerdp_drive_argument)
        .transpose()?
        .flatten()
    {
        arguments.push(argument);
    }
    let bootstrap_path = bootstrap
        .path()
        .to_str()
        .filter(|p| !p.chars().any(|c| c.is_control() || c == ',' || c == '"'))
        .ok_or_else(|| "private RemoteApp bootstrap unavailable".to_string())?;
    arguments.push(format!("/drive:{bootstrap_drive},{bootstrap_path}"));
    arguments.push(remote_app);

    // FreeRDP 3 can parse one argument per line from stdin. This keeps the
    // Windows password out of the process list and out of application logs.
    // An anonymous private file keeps FreeRDP's output descriptors valid after
    // a one-shot CLI parent exits. A pipe would lose its reader at process exit
    // and can terminate FreeRDP with SIGPIPE, which in turn closes the Windows
    // RemoteApp session. The file has no persistent pathname and is never
    // exposed through CLI output or operation history.
    let diagnostics =
        tempfile::tempfile().map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    let diagnostic_stdout = diagnostics
        .try_clone()
        .map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    let diagnostic_stderr = diagnostics
        .try_clone()
        .map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    let child = Command::new(&config.freerdp_binary)
        .arg("/args-from:stdin")
        .env("XDG_CONFIG_HOME", trust.config_directory())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(diagnostic_stdout))
        .stderr(Stdio::from(diagnostic_stderr))
        .spawn()
        .map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    // Install the owner before sending input: EPIPE or a missing input pipe
    // must still reap the child and discard its private TOFU state.
    let mut process = RemoteAppProcess {
        child,
        diagnostics: Mutex::new(diagnostics),
        _bootstrap: Some(bootstrap),
        trust: Some(trust),
    };
    let payload = format!("{}\n", arguments.join("\n"));
    process
        .child
        .stdin
        .take()
        .ok_or_else(|| crate::tr!("error-freerdp-input-open"))?
        .write_all(payload.as_bytes())
        .map_err(|error| crate::tr!("error-freerdp-credentials-send", error = error))?;
    Ok(process)
}

#[cfg(test)]
fn read_diagnostics_at(file: &File, bytes: &mut [u8]) -> std::io::Result<usize> {
    read_diagnostics_from(file, bytes, 0)
}

#[cfg(unix)]
fn read_diagnostics_from(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(bytes, offset)
}

#[cfg(windows)]
fn read_diagnostics_from(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(bytes, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_diagnostics_from(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(bytes)
}

fn app_freerdp_config_directory() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| crate::tr!("error-freerdp-trust-directory-home"))?;
    let directory = base.join("mendimaru/freerdp-config");
    std::fs::create_dir_all(&directory)
        .map_err(|error| crate::tr!("error-freerdp-trust-directory-create", error = error))?;
    // TrustAttempt verifies this is a direct, user-owned directory before it
    // changes permissions; never chmod through an untrusted symlink here.
    Ok(directory)
}

fn css_slug(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{certificate_rejected, read_diagnostics_at};
    use std::io::Write;

    /// FreeRDP 3.15.0 output of a successful TOFU first use against the
    /// WinBoat guest, as captured on the #227 host (timestamps removed).
    const FIRST_USE_ACCEPTED: &str = "\
[WARN][com.freerdp.crypto] - [verify_cb]: Certificate verification failure 'self-signed certificate (18)' at stack position 0
[ERROR][com.freerdp.crypto] - [tls_print_certificate_name_mismatch_error]: @           WARNING: CERTIFICATE NAME MISMATCH!           @
[ERROR][com.freerdp.crypto] - [tls_print_certificate_name_mismatch_error]: does not match the name given in the certificate:
[ERROR][com.freerdp.crypto] - [tls_print_certificate_name_mismatch_error]: A valid certificate for the wrong name should NOT be trusted!
[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: The host key for 127.0.0.1:47303 has changed
[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: Host key for 127.0.0.1 has changed and you have requested strict checking.
[ERROR][com.freerdp.crypto] - [tls_print_new_certificate_warn]: Host key verification failed.
[WARN][com.freerdp.core.license] - [license_read_binary_blob_data]: license binary blob::type BB_ERROR_BLOB, length=0, skipping.
";

    /// FreeRDP 3.15.0 output when `deny` rejects a certificate that differs
    /// from the pinned one.
    const PINNED_REJECTED: &str = "\
[WARN][com.freerdp.crypto] - [verify_cb]: Certificate verification failure 'self-signed certificate (18)' at stack position 0
[ERROR][com.freerdp.crypto] - [tls_print_certificate_error]: New host key for 127.0.0.1:47304
[ERROR][com.freerdp.crypto] - [tls_print_certificate_error]: @    WARNING: NEW HOST IDENTIFICATION!     @
[ERROR][com.freerdp.crypto] - [freerdp_tls_handshake]: certificate not trusted, aborting.
[ERROR][com.freerdp.core] - [transport_default_connect_tls]: ERRCONNECT_TLS_CONNECT_FAILED [0x00020008]
";

    #[test]
    fn a_successful_first_use_is_not_mistaken_for_a_certificate_rejection() {
        assert!(!certificate_rejected(FIRST_USE_ACCEPTED));
        assert!(!certificate_rejected(""));
        assert!(certificate_rejected(PINNED_REJECTED));
        assert!(certificate_rejected("CERTIFICATE NOT TRUSTED, ABORTING."));
    }

    #[test]
    fn diagnostic_reads_do_not_move_the_child_writer_offset() {
        let diagnostics = tempfile::tempfile().expect("anonymous diagnostics");
        let mut writer = diagnostics.try_clone().expect("diagnostic writer");
        writer.write_all(b"certificate ").expect("first diagnostic");
        let mut first = vec![0_u8; 64];
        let count = read_diagnostics_at(&diagnostics, &mut first).expect("read diagnostics");
        assert_eq!(&first[..count], b"certificate ");

        writer.write_all(b"mismatch").expect("second diagnostic");
        let mut complete = vec![0_u8; 64];
        let count = read_diagnostics_at(&diagnostics, &mut complete).expect("reread diagnostics");
        assert_eq!(&complete[..count], b"certificate mismatch");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejection_after_a_large_warning_burst_is_still_classified() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let process = super::RemoteAppProcess::from_test_child(child);
        let mut writer = process.diagnostics.lock().unwrap().try_clone().unwrap();
        writer
            .write_all(&vec![b'w'; super::MAX_DIAGNOSTIC_BYTES + 1])
            .unwrap();
        writer.write_all(PINNED_REJECTED.as_bytes()).unwrap();
        assert!(process.certificate_failed());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn aborting_the_operation_future_discards_private_tofu_and_reaps_its_child() {
        let store = tempfile::tempdir().unwrap();
        let trust = crate::winboat::rdp_trust::TrustAttempt::fixture(store.path());
        let scratch = trust.config_directory().to_path_buf();
        std::fs::write(
            scratch.join("freerdp/server/127.0.0.1_47300.pem"),
            b"-----BEGIN CERTIFICATE-----\nbWVuZGltYXJ1\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let child = std::process::Command::new("sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .unwrap();
        let process_id = child.id();
        let process = super::RemoteAppProcess::from_test_child_with_trust(child, trust);
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _owner = process;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!scratch.exists());
        assert!(!store
            .path()
            .join("freerdp/server/127.0.0.1_47300.pem")
            .exists());
        assert!(!std::path::Path::new(&format!("/proc/{process_id}")).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_a_remote_app_reaps_its_child_process() {
        let child = std::process::Command::new("sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .expect("spawn RemoteApp stand-in");
        let process_id = child.id();
        let remote_app = super::RemoteAppProcess {
            child,
            diagnostics: std::sync::Mutex::new(
                tempfile::tempfile().expect("anonymous diagnostics"),
            ),
            _bootstrap: None,
            trust: None,
        };

        drop(remote_app);

        assert!(
            !std::path::Path::new(&format!("/proc/{process_id}")).exists(),
            "the dropped RemoteApp child was not reaped"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stopping_a_remote_app_asks_freerdp_to_disconnect_before_killing_it() {
        let directory = tempfile::tempdir().expect("marker directory");
        let marker = directory.path().join("terminated");
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "trap 'echo graceful > {}; exit 0' TERM; sleep 60 & wait",
                marker.display()
            ))
            .spawn()
            .expect("spawn RemoteApp stand-in");
        let mut remote_app = super::RemoteAppProcess::from_test_child(child);
        // Let the shell install its trap before the signal arrives.
        std::thread::sleep(std::time::Duration::from_millis(200));

        remote_app.kill().expect("stop RemoteApp");
        let status = remote_app.wait().expect("reap RemoteApp");

        assert!(status.success(), "SIGTERM must allow a clean exit");
        assert_eq!(
            std::fs::read_to_string(&marker).expect("graceful marker"),
            "graceful\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_remote_app_that_ignores_sigterm_is_still_killed_within_the_grace_period() {
        let child = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; exec sleep 60"])
            .spawn()
            .expect("spawn RemoteApp stand-in");
        let process_id = child.id();
        let mut remote_app = super::RemoteAppProcess::from_test_child(child);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();

        remote_app.kill().expect("stop RemoteApp");
        let _ = remote_app.wait();

        let elapsed = started.elapsed();
        assert!(elapsed >= super::GRACEFUL_STOP, "{elapsed:?}");
        assert!(elapsed < super::GRACEFUL_STOP * 4, "{elapsed:?}");
        assert!(!std::path::Path::new(&format!("/proc/{process_id}")).exists());
    }
}
