use super::scripts::powershell_literal;
use crate::config::resolved_rdp_port;
use crate::models::AppConfig;
use crate::projects::linux_path_to_windows_share;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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
) -> Result<Child, String> {
    let windows_script_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        script_path,
        &config.windows_shared_directory,
    )?;
    // RAIL limits RemoteApplicationCmdLine to 16,000 bytes. Encoding the full
    // script can exceed that limit, so only encode a short command that invokes
    // the script already stored in the WinBoat shared directory.
    let launcher = format!(
        "& '{}'; exit $LASTEXITCODE",
        powershell_literal(&windows_script_path)
    );
    let encoded = encode_powershell_script(&launcher);
    let arguments = powershell_encoded_arguments(&encoded);
    // Do not publish PowerShell itself as the RemoteApp. FreeRDP can map its
    // console before WindowStyle Hidden takes effect, which causes a visible
    // flash. WScript is windowless here and starts PowerShell hidden while
    // preserving the already-elevated RemoteApp session token.
    let hidden_launcher_path = write_hidden_powershell_launcher(script_path, &arguments)?;
    let windows_hidden_launcher_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &hidden_launcher_path,
        &config.windows_shared_directory,
    )?;
    spawn_remote_app(
        config,
        r"C:\Windows\System32\wscript.exe",
        Some("//B //NoLogo"),
        Some(&windows_hidden_launcher_path),
        label,
    )
}

fn write_hidden_powershell_launcher(
    script_path: &Path,
    powershell_arguments: &str,
) -> Result<PathBuf, String> {
    let launcher_path = script_path.with_extension("vbs");
    fs::write(
        &launcher_path,
        hidden_powershell_launcher(powershell_arguments),
    )
    .map_err(|error| crate::tr!("error-hidden-wrapper-save", error = error))?;
    Ok(launcher_path)
}

pub(super) fn hidden_powershell_launcher(powershell_arguments: &str) -> String {
    format!(
        "Option Explicit\r\n\
         Dim shell, exitCode\r\n\
         Set shell = CreateObject(\"WScript.Shell\")\r\n\
         exitCode = shell.Run(\"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe {powershell_arguments}\", 0, True)\r\n\
         WScript.Quit exitCode\r\n"
    )
}

pub(super) fn powershell_encoded_arguments(encoded_command: &str) -> String {
    format!(
        "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -EncodedCommand {encoded_command}"
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
    remote_file: Option<&str>,
    label: &str,
) -> Result<Child, String> {
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
    if let Some(file) = remote_file.filter(|file| !file.is_empty()) {
        remote_app.push_str(",file:");
        remote_app.push_str(file);
    }
    let arguments = [
        format!("/u:{username}"),
        format!("/p:{password}"),
        format!("/v:{}", config.rdp_host),
        format!("/port:{}", resolved_rdp_port(config)),
        "/cert:ignore".to_string(),
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
    let mut child = Command::new(&config.freerdp_binary)
        .arg("/args-from:stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| crate::tr!("error-remoteapp-run", error = error))?;
    let payload = format!("{}\n", arguments.join("\n"));
    child
        .stdin
        .take()
        .ok_or_else(|| crate::tr!("error-freerdp-input-open"))?
        .write_all(payload.as_bytes())
        .map_err(|error| crate::tr!("error-freerdp-credentials-send", error = error))?;
    Ok(child)
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
