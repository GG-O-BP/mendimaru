use super::security::{secure_powershell_launcher, OperationSecurity};
use crate::config::resolved_rdp_port;
use crate::models::AppConfig;
use crate::projects::linux_path_to_windows_share;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;

pub(super) const FREERDP_CERTIFICATE_POLICY: &str = "/cert:tofu";
pub(super) const HEADLESS_CONSOLE_HOST: &str = r"C:\Windows\System32\conhost.exe";
pub(super) const POWERSHELL_EXECUTABLE: &str =
    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

pub(super) struct RemoteAppProcess {
    child: Child,
    diagnostics: Mutex<File>,
}

impl RemoteAppProcess {
    pub(super) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(super) fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    pub(super) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }

    pub(super) fn certificate_failed(&self) -> bool {
        let diagnostics = self.diagnostics.lock().ok().and_then(|file| {
            let length = file.metadata().ok()?.len().min(MAX_DIAGNOSTIC_BYTES as u64) as usize;
            let mut bytes = vec![0_u8; length];
            let count = read_diagnostics_at(&file, &mut bytes).ok()?;
            bytes.truncate(count);
            Some(String::from_utf8_lossy(&bytes).to_ascii_lowercase())
        });
        let diagnostics = diagnostics.unwrap_or_default();
        diagnostics.contains("certificate")
            && [
                "mismatch",
                "verification failure",
                "verify failed",
                "not match",
                "changed",
                "denied",
            ]
            .iter()
            .any(|pattern| diagnostics.contains(pattern))
    }
}

fn container_credentials(config: &AppConfig) -> Result<(String, String), String> {
    let output = Command::new(config.container_runtime.as_str())
        .arg("inspect")
        .arg("--format")
        .arg("{{range .Config.Env}}{{println .}}{{end}}")
        .arg(&config.container_name)
        .output()
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
) -> Result<RemoteAppProcess, String> {
    let windows_script_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        script_path,
        &config.windows_shared_directory,
    )?;
    let launcher = secure_powershell_launcher(&windows_script_path, security);
    let encoded = encode_powershell_script(&launcher);
    let arguments = headless_powershell_arguments(&encoded);
    if arguments.encode_utf16().count() * 2 >= 16_000 {
        return Err(crate::tr!("error-remoteapp-command-too-long"));
    }
    spawn_remote_app(config, HEADLESS_CONSOLE_HOST, Some(&arguments), label)
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
) -> Result<RemoteAppProcess, String> {
    let (username, password) = container_credentials(config)?;
    let safe_label: String = label
        .chars()
        .filter(|character| !matches!(character, ',' | '\'' | '"'))
        .collect();
    let mut remote_app = format!("/app:program:{executable_path},name:{safe_label}");
    if let Some(arguments) = app_arguments.filter(|arguments| !arguments.is_empty()) {
        remote_app.push_str(",cmd:");
        remote_app.push_str(arguments);
    }
    let trust_store = app_freerdp_config_directory()?;
    let certificate_policy = certificate_policy(config, &trust_store)?;
    let arguments = [
        format!("/u:{username}"),
        format!("/p:{password}"),
        format!("/v:{}", config.rdp_host),
        format!("/port:{}", resolved_rdp_port(config)),
        certificate_policy,
        "/log-level:WARN".to_string(),
        "+clipboard".to_string(),
        "/sound:sys:pulse".to_string(),
        "/microphone:sys:pulse".to_string(),
        "/floatbar".to_string(),
        "/compression".to_string(),
        "/sec:tls".to_string(),
        "-wallpaper".to_string(),
        "/scale-desktop:100".to_string(),
        format!("/wm-class:mendimaru-{}", css_slug(&safe_label)),
        remote_app,
    ];

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
    let mut child = Command::new(&config.freerdp_binary)
        .arg("/args-from:stdin")
        .env("XDG_CONFIG_HOME", &trust_store)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(diagnostic_stdout))
        .stderr(Stdio::from(diagnostic_stderr))
        .spawn()
        .map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    let payload = format!("{}\n", arguments.join("\n"));
    child
        .stdin
        .take()
        .ok_or_else(|| crate::tr!("error-freerdp-input-open"))?
        .write_all(payload.as_bytes())
        .map_err(|error| crate::tr!("error-freerdp-credentials-send", error = error))?;
    Ok(RemoteAppProcess {
        child,
        diagnostics: Mutex::new(diagnostics),
    })
}

#[cfg(unix)]
fn read_diagnostics_at(file: &File, bytes: &mut [u8]) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(bytes, 0)
}

#[cfg(windows)]
fn read_diagnostics_at(file: &File, bytes: &mut [u8]) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(bytes, 0)
}

#[cfg(not(any(unix, windows)))]
fn read_diagnostics_at(file: &File, bytes: &mut [u8]) -> std::io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    file.read(bytes)
}

fn certificate_policy(config: &AppConfig, trust_store: &Path) -> Result<String, String> {
    let pin = trust_store.join("freerdp/server").join(format!(
        "{}_{}.pem",
        config.rdp_host,
        resolved_rdp_port(config)
    ));
    match std::fs::read(&pin) {
        Ok(pem) => fingerprint_policy_from_pem(&pem),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(FREERDP_CERTIFICATE_POLICY.to_string())
        }
        Err(error) => Err(crate::tr!("error-freerdp-pin-read", error = error)),
    }
}

fn fingerprint_policy_from_pem(pem: &[u8]) -> Result<String, String> {
    let pem = std::str::from_utf8(pem)
        .map_err(|error| crate::tr!("error-freerdp-pin-invalid", error = error))?;
    let body = pem
        .lines()
        .skip_while(|line| line.trim() != "-----BEGIN CERTIFICATE-----")
        .skip(1)
        .take_while(|line| line.trim() != "-----END CERTIFICATE-----")
        .map(str::trim)
        .collect::<String>();
    if body.is_empty() {
        return Err(crate::tr!(
            "error-freerdp-pin-invalid",
            error = "the certificate body is missing"
        ));
    }
    let certificate = BASE64_STANDARD
        .decode(body)
        .map_err(|error| crate::tr!("error-freerdp-pin-invalid", error = error))?;
    let fingerprint = format!("{:x}", Sha256::digest(certificate));
    Ok(format!("/cert:fingerprint:sha256:{fingerprint}"))
}

fn app_freerdp_config_directory() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| crate::tr!("error-freerdp-trust-directory-home"))?;
    let directory = base.join("mendimaru/freerdp-config");
    std::fs::create_dir_all(&directory)
        .map_err(|error| crate::tr!("error-freerdp-trust-directory-create", error = error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| crate::tr!("error-freerdp-trust-directory-create", error = error))?;
    }
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
    use super::{fingerprint_policy_from_pem, read_diagnostics_at};
    use std::io::Write;

    #[test]
    fn converts_the_app_pin_to_an_exact_sha256_fingerprint_policy() {
        let policy = fingerprint_policy_from_pem(
            b"-----BEGIN CERTIFICATE-----\nbWVuZGltYXJ1\n-----END CERTIFICATE-----\n",
        )
        .expect("fixture pin parses");
        assert_eq!(
            policy,
            "/cert:fingerprint:sha256:8edde51f9bc00d3fff19237df43bc1c6d058839aa1469b3fbd0c5479c929825d"
        );
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
}
