use crate::config::{compose_shared_directory, path_exists_or_binary};
use crate::models::{AppConfig, EnvironmentStatus, LaunchResult, StudioVersion, WinApp};
use crate::projects::{linux_path_to_windows_share, scan_projects};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GUEST_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const INSTALL_TIMEOUT_SECONDS: u64 = 45 * 60;
const UNINSTALL_TIMEOUT_SECONDS: u64 = 15 * 60;
const REMOTE_APP_START_GRACE_SECONDS: u64 = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsOperationReport {
    state: String,
    #[serde(default)]
    message: String,
    exit_code: Option<i32>,
    executable_path: Option<String>,
    error: Option<String>,
}

pub async fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    let winboat_available = path_exists_or_binary(&config.winboat_executable);
    let compose_available = Path::new(&config.compose_file).is_file();
    let runtime_available = path_exists_or_binary(&config.container_runtime);
    let freerdp_available = path_exists_or_binary(&config.freerdp_binary);
    let shared_directory_available = Path::new(&config.shared_directory).is_dir();
    let compose_shared = compose_shared_directory(&config.compose_file);
    let shared_mount_matches = compose_shared
        .as_deref()
        .is_some_and(|current| paths_refer_to_same_location(current, &config.shared_directory));
    let container_status = inspect_container_status(config);
    let guest_online = guest_is_online(config).await;

    let mut notices = Vec::new();
    if !winboat_available {
        notices.push("WinBoat 실행 파일을 찾지 못했습니다.".to_string());
    }
    if !compose_available {
        notices.push("WinBoat Compose 파일을 찾지 못했습니다.".to_string());
    }
    if !runtime_available {
        notices.push(format!(
            "{} 컨테이너 런타임을 찾지 못했습니다.",
            config.container_runtime
        ));
    }
    if !freerdp_available {
        notices.push("FreeRDP 3 실행 파일을 찾지 못했습니다.".to_string());
    }
    if !shared_directory_available {
        notices.push("설정된 공유 디렉터리가 존재하지 않습니다.".to_string());
    } else if !shared_mount_matches {
        notices.push("앱 설정과 Compose의 /shared 마운트가 다릅니다.".to_string());
    }
    if runtime_available && container_status != "running" {
        notices.push("WinBoat Windows가 실행 중이 아닙니다.".to_string());
    } else if container_status == "running" && !guest_online {
        notices
            .push("Windows는 실행 중이지만 Guest Server가 아직 준비되지 않았습니다.".to_string());
    }

    EnvironmentStatus {
        winboat_available,
        compose_available,
        runtime_available,
        freerdp_available,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        guest_online,
        notices,
    }
}

pub async fn installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    if !guest_is_online(config).await {
        return Err(
            "WinBoat Guest Server가 오프라인입니다. Windows를 시작한 뒤 다시 시도하세요."
                .to_string(),
        );
    }
    let client = http_client(Duration::from_secs(GUEST_REQUEST_TIMEOUT_SECONDS))?;
    let response = client
        .get(format!("{}/apps", config.api_url))
        .send()
        .await
        .map_err(|error| format!("Windows 앱 목록을 가져오지 못했습니다: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Windows 앱 목록 응답이 올바르지 않습니다: {error}"))?;
    let apps = response
        .json::<Vec<WinApp>>()
        .await
        .map_err(|error| format!("Windows 앱 목록을 해석하지 못했습니다: {error}"))?;
    Ok(parse_studio_versions(apps, &config.mendix_install_root))
}

pub async fn start_container(config: &AppConfig) -> Result<String, String> {
    let status = inspect_container_status(config);
    if status == "running" {
        return Ok(status);
    }

    if status != "not-found" {
        let output = Command::new(&config.container_runtime)
            .arg("start")
            .arg(&config.container_name)
            .output()
            .map_err(|error| format!("WinBoat 컨테이너를 시작하지 못했습니다: {error}"))?;
        ensure_success(output, "WinBoat 컨테이너 시작")?;
    } else {
        compose_up(config, false).await?;
    }
    Ok(inspect_container_status(config))
}

pub async fn recreate_container(config: &AppConfig) -> Result<(), String> {
    compose_up(config, true).await
}

pub fn open_winboat(config: &AppConfig) -> Result<(), String> {
    Command::new(&config.winboat_executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("WinBoat를 열 수 없습니다: {error}"))
}

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    project_mpr_path: Option<&str>,
) -> Result<LaunchResult, String> {
    ensure_guest_online(config).await?;
    let versions = installed_versions(config).await?;
    let selected = versions
        .into_iter()
        .find(|installed| installed.version == version)
        .ok_or_else(|| format!("Studio Pro {version} 설치를 찾을 수 없습니다."))?;

    let project_argument = if let Some(project_path) = project_mpr_path {
        validate_project_argument(config, project_path)?
    } else {
        None
    };
    let label = format!("Studio Pro {}", selected.version);
    let _remote_app = spawn_remote_app(
        config,
        &selected.executable_path,
        None,
        project_argument.as_deref(),
        &label,
    )?;
    Ok(LaunchResult {
        label,
        executable_path: selected.executable_path,
    })
}

pub async fn install_studio(
    config: &AppConfig,
    version: &str,
    windows_installer_path: &str,
) -> Result<String, String> {
    validate_version(version)?;
    ensure_guest_online(config).await?;
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| format!("설치 상태 디렉터리를 만들 수 없습니다: {error}"))?;
    let operation_id = format!(
        "install-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;

    let script = install_script(
        windows_installer_path,
        &windows_report_path,
        &config.mendix_install_root,
        version,
    );
    // Keep the exact script next to other commands so a failed installation can
    // be diagnosed without exposing the Windows password or FreeRDP arguments.
    let _script_path = write_command_script(config, &operation_id, &script)?;
    let mut remote_app =
        spawn_powershell_script(config, &script, &format!("Install Studio Pro {version}"))?;

    let report = wait_for_windows_operation(
        &report_path,
        &mut remote_app,
        INSTALL_TIMEOUT_SECONDS,
        "Studio Pro 설치",
    )
    .await?;
    report
        .executable_path
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "설치는 완료됐지만 Studio Pro 실행 경로를 확인하지 못했습니다.".to_string())
}

pub async fn launch_uninstaller(config: &AppConfig, version: &str) -> Result<(), String> {
    validate_version(version)?;
    ensure_guest_online(config).await?;
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| format!("제거 상태 디렉터리를 만들 수 없습니다: {error}"))?;
    let operation_id = format!(
        "uninstall-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = uninstall_script(
        &config.mendix_data_root,
        &config.mendix_install_root,
        version,
        &windows_report_path,
    );
    let _script_path = write_command_script(config, &operation_id, &script)?;
    let mut remote_app =
        spawn_powershell_script(config, &script, &format!("Uninstall Studio Pro {version}"))?;
    wait_for_windows_operation(
        &report_path,
        &mut remote_app,
        UNINSTALL_TIMEOUT_SECONDS,
        "Studio Pro 제거",
    )
    .await?;
    Ok(())
}

pub fn open_linux_folder(path: &str) -> Result<(), String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return Err(format!("디렉터리를 찾을 수 없습니다: {path}"));
    }
    Command::new("xdg-open")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("파일 관리자를 열 수 없습니다: {error}"))
}

pub fn validate_version(version: &str) -> Result<(), String> {
    let pattern = Regex::new(r"^\d+\.\d+\.\d+(?:\.\d+)?(?:-(?:beta|rc)\d*)?$")
        .map_err(|error| error.to_string())?;
    if pattern.is_match(version) {
        Ok(())
    } else {
        Err("버전은 11.12.2와 같은 Mendix 버전 형식이어야 합니다.".to_string())
    }
}

async fn compose_up(config: &AppConfig, force_recreate: bool) -> Result<(), String> {
    let mut command;
    if config.container_runtime == "podman" && path_exists_or_binary("podman-compose") {
        command = Command::new("podman-compose");
    } else {
        command = Command::new(&config.container_runtime);
        command.arg("compose");
    }
    command
        .arg("-f")
        .arg(&config.compose_file)
        .arg("up")
        .arg("-d");
    if force_recreate {
        command.arg("--force-recreate");
    }
    let output = command
        .output()
        .map_err(|error| format!("WinBoat Compose를 실행하지 못했습니다: {error}"))?;
    ensure_success(output, "WinBoat Compose 적용")
}

async fn ensure_guest_online(config: &AppConfig) -> Result<(), String> {
    if guest_is_online(config).await {
        return Ok(());
    }
    start_container(config).await?;
    let timeout = Duration::from_secs(config.startup_timeout_seconds);
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        if guest_is_online(config).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(format!(
        "{}초 안에 WinBoat Guest Server가 준비되지 않았습니다.",
        config.startup_timeout_seconds
    ))
}

async fn guest_is_online(config: &AppConfig) -> bool {
    let Ok(client) = http_client(Duration::from_secs(2)) else {
        return false;
    };
    client
        .get(format!("{}/health", config.api_url))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("mendimaru/0.1 (WinBoat Studio Pro manager)")
        .build()
        .map_err(|error| format!("HTTP 클라이언트를 만들 수 없습니다: {error}"))
}

fn inspect_container_status(config: &AppConfig) -> String {
    let output = Command::new(&config.container_runtime)
        .arg("inspect")
        .arg("--format")
        .arg("{{.State.Status}}")
        .arg(&config.container_name)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "not-found".to_string(),
    }
}

fn ensure_success(output: Output, operation: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("{operation}에 실패했습니다: {}", output.status))
    } else {
        Err(format!("{operation}에 실패했습니다: {stderr}"))
    }
}

fn parse_studio_versions(apps: Vec<WinApp>, install_root: &str) -> Vec<StudioVersion> {
    let root = normalize_windows_path(install_root)
        .trim_end_matches('\\')
        .to_string();
    let prefix = format!("{}\\", root.to_lowercase());
    let version_pattern = Regex::new(r"^(\d+\.\d+\.\d+)(?:\.\d+)?$").expect("version regex");
    let mut versions = BTreeMap::<String, StudioVersion>::new();

    for app in apps {
        let normalized_path = normalize_windows_path(&app.path);
        let lower_path = normalized_path.to_lowercase();
        if !lower_path.starts_with(&prefix) || !lower_path.ends_with(r"\modeler\studiopro.exe") {
            continue;
        }
        let relative = &normalized_path[prefix.len()..];
        let Some(folder) = relative.split('\\').next() else {
            continue;
        };
        let Some(captures) = version_pattern.captures(folder) else {
            continue;
        };
        let version = captures
            .get(1)
            .expect("version capture")
            .as_str()
            .to_string();
        versions.entry(version.clone()).or_insert(StudioVersion {
            version: version.clone(),
            display_name: if app.name.is_empty() {
                format!("Studio Pro {version}")
            } else {
                app.name
            },
            executable_path: app.path,
            install_root: format!("{}\\{}", install_root.trim_end_matches('\\'), folder),
            source: if app.source.is_empty() {
                "WinBoat Guest Server".to_string()
            } else {
                app.source
            },
        });
    }

    let mut result: Vec<_> = versions.into_values().collect();
    result.sort_by_key(|item| std::cmp::Reverse(version_parts(&item.version)));
    result
}

fn version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect()
}

fn normalize_windows_path(path: &str) -> String {
    path.replace('/', "\\")
}

fn validate_project_argument(
    config: &AppConfig,
    requested_path: &str,
) -> Result<Option<String>, String> {
    let requested = Path::new(requested_path);
    if !requested.is_file()
        || !requested
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mpr"))
    {
        return Err("선택한 Mendix .mpr 프로젝트를 찾을 수 없습니다.".to_string());
    }
    let projects = scan_projects(config)?;
    let project = projects
        .into_iter()
        .find(|project| paths_refer_to_same_location(&project.mpr_path, requested_path))
        .ok_or_else(|| "프로젝트는 설정된 공유 워크스페이스 안에 있어야 합니다.".to_string())?;
    Ok(Some(project.windows_path))
}

fn paths_refer_to_same_location(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left_path == right_path,
    }
}

fn container_credentials(config: &AppConfig) -> Result<(String, String), String> {
    let output = Command::new(&config.container_runtime)
        .arg("inspect")
        .arg("--format")
        .arg("{{range .Config.Env}}{{println .}}{{end}}")
        .arg(&config.container_name)
        .output()
        .map_err(|error| format!("Windows 계정 정보를 확인할 수 없습니다: {error}"))?;
    if !output.status.success() {
        return Err("실행 중인 WinBoat 컨테이너에서 Windows 계정을 찾지 못했습니다.".to_string());
    }
    let environment = String::from_utf8_lossy(&output.stdout);
    let username = environment
        .lines()
        .find_map(|line| line.strip_prefix("USERNAME="))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "WinBoat Windows 사용자명이 설정되지 않았습니다.".to_string())?;
    let password = environment
        .lines()
        .find_map(|line| line.strip_prefix("PASSWORD="))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "WinBoat Windows 암호가 설정되지 않았습니다.".to_string())?;
    Ok((username, password))
}

fn install_script(
    windows_installer_path: &str,
    windows_report_path: &str,
    install_root: &str,
    version: &str,
) -> String {
    const TEMPLATE: &str = r#"$ErrorActionPreference = 'Stop'
$installer = '__INSTALLER_PATH__'
$resultPath = '__RESULT_PATH__'
$installRoot = '__INSTALL_ROOT__'
$version = '__VERSION__'
$process = $null

function Write-InstallResult {
    param(
        [string]$State,
        [string]$Message,
        $ExitCode,
        $ExecutablePath,
        $ErrorMessage
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        exitCode = $ExitCode
        executablePath = $ExecutablePath
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
    }
    $temporaryPath = "$resultPath.tmp"
    $payload | ConvertTo-Json -Compress | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
    Move-Item -LiteralPath $temporaryPath -Destination $resultPath -Force
}

function Find-StudioPro {
    $folders = Get-ChildItem -LiteralPath $installRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending

    foreach ($folder in $folders) {
        $candidate = Join-Path $folder.FullName 'modeler\studiopro.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    return $null
}

try {
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "Installer not found: $installer"
    }

    Write-InstallResult 'running' 'Studio Pro installer is running.' $null $null $null
    $process = Start-Process -FilePath $installer -ArgumentList @('/SILENT') -Verb RunAs -Wait -PassThru
    $exitCode = [int]$process.ExitCode
    if (@(0, 1641, 3010) -notcontains $exitCode) {
        throw "Installer exited with code $exitCode."
    }

    $deadline = (Get-Date).AddMinutes(3)
    $studioPro = $null
    do {
        $studioPro = Find-StudioPro
        if ($null -ne $studioPro) { break }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)

    if ($null -eq $studioPro) {
        throw "StudioPro.exe was not created for version $version."
    }

    Write-InstallResult 'succeeded' 'Studio Pro installation completed.' $exitCode $studioPro $null
    exit 0
} catch {
    $exitCode = if ($null -ne $process) { [int]$process.ExitCode } else { $null }
    Write-InstallResult 'failed' 'Studio Pro installation failed.' $exitCode $null $_.Exception.Message
    exit 1
}
"#;

    TEMPLATE
        .replace(
            "__INSTALLER_PATH__",
            &powershell_literal(windows_installer_path),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
}

fn uninstall_script(
    data_root: &str,
    install_root: &str,
    version: &str,
    windows_report_path: &str,
) -> String {
    const TEMPLATE: &str = r#"$ErrorActionPreference = 'Stop'
$dataRoot = '__DATA_ROOT__'
$installRoot = '__INSTALL_ROOT__'
$version = '__VERSION__'
$resultPath = '__RESULT_PATH__'
$process = $null

function Write-UninstallResult {
    param(
        [string]$State,
        [string]$Message,
        $ExitCode,
        $ErrorMessage
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        exitCode = $ExitCode
        executablePath = $null
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
    }
    $temporaryPath = "$resultPath.tmp"
    $payload | ConvertTo-Json -Compress | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
    Move-Item -LiteralPath $temporaryPath -Destination $resultPath -Force
}

function Find-StudioPro {
    $folders = Get-ChildItem -LiteralPath $installRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending

    foreach ($folder in $folders) {
        $candidate = Join-Path $folder.FullName 'modeler\studiopro.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    return $null
}

try {
    $folder = Get-ChildItem -LiteralPath $dataRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $folder) {
        throw "Mendix data folder not found for $version."
    }

    $uninstaller = Join-Path $folder.FullName 'uninst\unins000.exe'
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        throw "Uninstaller not found: $uninstaller"
    }

    Write-UninstallResult 'running' 'Studio Pro uninstaller is running.' $null $null
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/SILENT') -Verb RunAs -Wait -PassThru
    $exitCode = [int]$process.ExitCode
    if (@(0, 1641, 3010) -notcontains $exitCode) {
        throw "Uninstaller exited with code $exitCode."
    }

    $deadline = (Get-Date).AddMinutes(3)
    $studioPro = Find-StudioPro
    while ($null -ne $studioPro -and (Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $studioPro = Find-StudioPro
    }
    if ($null -ne $studioPro) {
        throw "StudioPro.exe still exists after uninstall: $studioPro"
    }

    Write-UninstallResult 'succeeded' 'Studio Pro uninstall completed.' $exitCode $null
    exit 0
} catch {
    $exitCode = if ($null -ne $process) { [int]$process.ExitCode } else { $null }
    Write-UninstallResult 'failed' 'Studio Pro uninstall failed.' $exitCode $_.Exception.Message
    exit 1
}
"#;

    TEMPLATE
        .replace("__DATA_ROOT__", &powershell_literal(data_root))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
}

async fn wait_for_windows_operation(
    report_path: &Path,
    remote_app: &mut Child,
    timeout_seconds: u64,
    operation: &str,
) -> Result<WindowsOperationReport, String> {
    let started = tokio::time::Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let mut remote_app_exited_at = None;

    loop {
        if let Ok(content) = tokio::fs::read_to_string(report_path).await {
            if let Ok(report) = parse_install_report(&content) {
                match report.state.as_str() {
                    "succeeded" => return Ok(report),
                    "failed" => {
                        let reason = report
                            .error
                            .filter(|message| !message.is_empty())
                            .unwrap_or_else(|| report.message.clone());
                        let exit_code = report
                            .exit_code
                            .map(|code| format!(" (종료 코드 {code})"))
                            .unwrap_or_default();
                        return Err(format!(
                            "Windows에서 {operation}에 실패했습니다{exit_code}: {reason}"
                        ));
                    }
                    _ => {}
                }
            }
        }

        if remote_app_exited_at.is_none() {
            match remote_app.try_wait() {
                Ok(Some(status)) => {
                    remote_app_exited_at = Some((tokio::time::Instant::now(), status))
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(format!(
                        "{operation}용 WinBoat RemoteApp 상태를 확인하지 못했습니다: {error}"
                    ));
                }
            }
        }

        if let Some((exited_at, status)) = remote_app_exited_at {
            if !report_path.is_file()
                && exited_at.elapsed() >= Duration::from_secs(REMOTE_APP_START_GRACE_SECONDS)
            {
                return Err(format!(
                    "{operation} 명령이 Windows에서 시작되지 않았습니다 (FreeRDP 상태: {status})."
                ));
            }
        }

        if started.elapsed() >= timeout {
            return Err(format!(
                "{operation}가 {}분 안에 완료되지 않았습니다. WinBoat Windows에서 상태를 확인해 주세요.",
                timeout_seconds / 60
            ));
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn parse_install_report(content: &str) -> Result<WindowsOperationReport, serde_json::Error> {
    serde_json::from_str(content.trim_start_matches('\u{feff}').trim())
}

fn spawn_powershell_script(config: &AppConfig, script: &str, label: &str) -> Result<Child, String> {
    let encoded = encode_powershell_script(script);
    let arguments = format!(
        "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}"
    );
    spawn_remote_app(
        config,
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Some(&arguments),
        None,
        label,
    )
}

fn encode_powershell_script(script: &str) -> String {
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
        format!("/port:{}", config.rdp_port),
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
        .map_err(|error| format!("WinBoat RemoteApp을 실행할 수 없습니다: {error}"))?;
    let payload = format!("{}\n", arguments.join("\n"));
    child
        .stdin
        .take()
        .ok_or_else(|| "FreeRDP 보안 입력 채널을 열 수 없습니다.".to_string())?
        .write_all(payload.as_bytes())
        .map_err(|error| format!("FreeRDP에 연결 정보를 전달할 수 없습니다: {error}"))?;
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

fn write_command_script(config: &AppConfig, name: &str, content: &str) -> Result<PathBuf, String> {
    let command_directory = Path::new(&config.shared_directory).join(".mendimaru/commands");
    fs::create_dir_all(&command_directory)
        .map_err(|error| format!("Windows 명령 디렉터리를 만들 수 없습니다: {error}"))?;
    let timestamp = unix_timestamp_millis();
    let safe_name = safe_operation_name(name);
    let path = command_directory.join(format!("{safe_name}-{timestamp}.ps1"));
    fs::write(&path, content)
        .map_err(|error| format!("Windows 명령 스크립트를 저장할 수 없습니다: {error}"))?;
    Ok(path)
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn safe_operation_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        .collect()
}

fn powershell_literal(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::{
        encode_powershell_script, parse_install_report, parse_studio_versions, uninstall_script,
        validate_version,
    };
    use crate::models::WinApp;
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

    #[test]
    fn extracts_versions_from_winboat_apps_using_reference_layout() {
        let apps = vec![
            WinApp {
                name: "studiopro".into(),
                path: r"C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe".into(),
                args: String::new(),
                icon: String::new(),
                source: String::new(),
            },
            WinApp {
                name: "Mendix.VersionSelector".into(),
                path: r"C:\Program Files\Mendix\Version Selector\VersionSelector.exe".into(),
                args: String::new(),
                icon: String::new(),
                source: String::new(),
            },
            WinApp {
                name: "Studio Pro".into(),
                path: r"C:\Program Files\Mendix\10.24.3.12345\modeler\studiopro.exe".into(),
                args: String::new(),
                icon: String::new(),
                source: "registry".into(),
            },
        ];
        let versions = parse_studio_versions(apps, r"C:\Program Files\Mendix");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, "11.12.2");
        assert_eq!(versions[1].version, "10.24.3");
    }

    #[test]
    fn accepts_normal_mendix_versions_and_rejects_commands() {
        assert!(validate_version("11.12.2").is_ok());
        assert!(validate_version("10.24.0.12345").is_ok());
        assert!(validate_version("11.0.0-beta1").is_ok());
        assert!(validate_version("11.12.2; calc.exe").is_err());
    }

    #[test]
    fn encodes_powershell_as_utf16le_without_remote_app_quotes() {
        let script = "Write-Output 'hello'";
        let decoded = BASE64_STANDARD
            .decode(encode_powershell_script(script))
            .expect("valid base64");
        let utf16: Vec<u16> = decoded
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        assert_eq!(String::from_utf16(&utf16).expect("valid UTF-16"), script);
    }

    #[test]
    fn parses_windows_powershell_utf8_bom_report() {
        let report = parse_install_report(
            "\u{feff}{\"state\":\"succeeded\",\"message\":\"done\",\"exitCode\":0,\"executablePath\":\"C:\\\\Program Files\\\\Mendix\\\\11.13.0\\\\modeler\\\\studiopro.exe\",\"error\":null}",
        )
        .expect("report should parse");
        assert_eq!(report.state, "succeeded");
        assert_eq!(report.exit_code, Some(0));
        assert!(report.executable_path.is_some());
    }

    #[test]
    fn uninstaller_waits_for_exit_and_studio_executable_removal() {
        let script = uninstall_script(
            r"C:\ProgramData\Mendix",
            r"C:\Program Files\Mendix",
            "11.13.0",
            r"\\host.lan\Data\.mendimaru\operations\uninstall.json",
        );

        assert!(script.contains("-Verb RunAs -Wait -PassThru"));
        assert!(script.contains("StudioPro.exe still exists after uninstall"));
        assert!(script.contains("'C:\\ProgramData\\Mendix'"));
        assert!(script.contains("'11.13.0'"));
        assert!(!script.contains("__VERSION__"));
    }
}
