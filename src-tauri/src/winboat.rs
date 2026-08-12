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
const STUDIO_LAUNCH_TIMEOUT_SECONDS: u64 = 5 * 60;
const INSTALL_TIMEOUT_SECONDS: u64 = 45 * 60;
const UNINSTALL_TIMEOUT_SECONDS: u64 = 15 * 60;
const REMOTE_APP_START_GRACE_SECONDS: u64 = 20;
const REMOTE_APP_START_ATTEMPTS: usize = 2;
const REMOTE_APP_RETRY_DELAY_SECONDS: u64 = 2;

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
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| format!("실행 상태 디렉터리를 만들 수 없습니다: {error}"))?;
    let operation_id = format!(
        "launch-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = launch_studio_script(
        &selected.executable_path,
        project_argument.as_deref(),
        &windows_report_path,
    );
    let script_path = write_command_script(config, &operation_id, &script)?;
    let report = run_windows_operation(
        config,
        &script_path,
        &label,
        &report_path,
        STUDIO_LAUNCH_TIMEOUT_SECONDS,
        "Studio Pro 실행",
        true,
    )
    .await?;
    if report.executable_path.as_deref().is_none_or(str::is_empty) {
        return Err("Studio Pro 창은 열렸지만 실행 경로를 확인하지 못했습니다.".to_string());
    }
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
    let script_path = write_command_script(config, &operation_id, &script)?;
    let label = format!("Install Studio Pro {version}");
    let report = run_windows_operation(
        config,
        &script_path,
        &label,
        &report_path,
        INSTALL_TIMEOUT_SECONDS,
        "Studio Pro 설치",
        false,
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
    let script_path = write_command_script(config, &operation_id, &script)?;
    let label = format!("Uninstall Studio Pro {version}");
    run_windows_operation(
        config,
        &script_path,
        &label,
        &report_path,
        UNINSTALL_TIMEOUT_SECONDS,
        "Studio Pro 제거",
        false,
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

fn launch_studio_script(
    executable_path: &str,
    project_path: Option<&str>,
    windows_report_path: &str,
) -> String {
    const TEMPLATE: &str = r#"$ErrorActionPreference = 'Stop'
$executable = '__EXECUTABLE_PATH__'
$projectPath = '__PROJECT_PATH__'
$resultPath = '__RESULT_PATH__'
$process = $null

function Write-LaunchResult {
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

function Get-StudioProcesses {
    return @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and $_.Path.Equals($executable, [System.StringComparison]::OrdinalIgnoreCase)
        } catch {
            $false
        }
    })
}

function Get-ReadyStudioProcess {
    foreach ($candidate in (@(Get-StudioProcesses) | Sort-Object StartTime -Descending)) {
        $candidate.Refresh()
        if ($candidate.MainWindowHandle -ne [IntPtr]::Zero) {
            return $candidate
        }
    }
    return $null
}

try {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Studio Pro executable not found: $executable"
    }

    Write-LaunchResult 'starting' 'Studio Pro is starting.' $null $executable $null
    if ([string]::IsNullOrWhiteSpace($projectPath)) {
        $process = Start-Process -FilePath $executable -PassThru
    } else {
        $quotedProjectPath = '"' + $projectPath + '"'
        $process = Start-Process -FilePath $executable -ArgumentList $quotedProjectPath -PassThru
    }

    $minimumReadyAt = (Get-Date).AddSeconds(2)
    $handoffDeadline = (Get-Date).AddSeconds(15)
    $deadline = (Get-Date).AddMinutes(4)
    $readyProcess = $null
    do {
        $readyProcess = Get-ReadyStudioProcess
        if ($null -ne $readyProcess -and (Get-Date) -ge $minimumReadyAt) {
            break
        }
        if ($process.HasExited -and $null -eq $readyProcess -and (Get-Date) -ge $handoffDeadline) {
            throw "Studio Pro exited before its window opened (code $($process.ExitCode))."
        }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)

    if ($null -eq $readyProcess) {
        throw 'Studio Pro window did not appear within 4 minutes.'
    }

    # Give FreeRDP time to publish the confirmed Windows handle as a local
    # RemoteApp window before the Linux-side launch button is enabled again.
    Start-Sleep -Milliseconds 1200
    Write-LaunchResult 'succeeded' 'Studio Pro window is ready.' $null $executable $null

    # Studio Pro can hand the sign-in window to another studiopro.exe process.
    # Keep the RemoteApp host alive across that handoff and only exit after no
    # process with the selected executable path has existed for 15 seconds.
    $missingSince = $null
    while ($true) {
        $studioProcesses = @(Get-StudioProcesses)
        if ($studioProcesses.Count -gt 0) {
            $missingSince = $null
        } elseif ($null -eq $missingSince) {
            $missingSince = Get-Date
        } elseif (((Get-Date) - $missingSince).TotalSeconds -ge 15) {
            break
        }
        Start-Sleep -Milliseconds 500
    }
    exit 0
} catch {
    $exitCode = if ($null -ne $process -and $process.HasExited) { [int]$process.ExitCode } else { $null }
    Write-LaunchResult 'failed' 'Studio Pro failed to start.' $exitCode $executable $_.Exception.Message
    exit 1
}
"#;

    TEMPLATE
        .replace("__EXECUTABLE_PATH__", &powershell_literal(executable_path))
        .replace(
            "__PROJECT_PATH__",
            &powershell_literal(project_path.unwrap_or_default()),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
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
$localInstaller = $null
$scriptExitCode = 0

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

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal] $identity
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'The WinBoat Windows session does not have administrator privileges.'
    }

    # Executing an installer directly from the host UNC share can block on an
    # invisible "Open File - Security Warning" dialog in RemoteApp. Stage it
    # locally and remove any downloaded-file zone marker before launching it.
    $sourceInstaller = Get-Item -LiteralPath $installer
    $stagingDirectory = Join-Path $env:ProgramData 'Mendimaru\Installers'
    New-Item -ItemType Directory -Path $stagingDirectory -Force | Out-Null
    $localInstaller = Join-Path $stagingDirectory $sourceInstaller.Name
    $localInfo = Get-Item -LiteralPath $localInstaller -ErrorAction SilentlyContinue
    if ($null -eq $localInfo -or $localInfo.Length -ne $sourceInstaller.Length) {
        Copy-Item -LiteralPath $installer -Destination $localInstaller -Force
    }
    Unblock-File -LiteralPath $localInstaller -ErrorAction SilentlyContinue

    Write-InstallResult 'running' 'Studio Pro installer is running.' $null $null $null
    $process = Start-Process -FilePath $localInstaller -ArgumentList @('/SILENT') -Wait -PassThru
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
} catch {
    $exitCode = if ($null -ne $process) { [int]$process.ExitCode } else { $null }
    Write-InstallResult 'failed' 'Studio Pro installation failed.' $exitCode $null $_.Exception.Message
    $scriptExitCode = 1
} finally {
    if ($null -ne $localInstaller) {
        Remove-Item -LiteralPath $localInstaller -Force -ErrorAction SilentlyContinue
    }
}
exit $scriptExitCode
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

function Get-RunningStudioPro {
    param([string]$ExecutablePath)

    return @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and $_.Path.Equals($ExecutablePath, [System.StringComparison]::OrdinalIgnoreCase)
        } catch {
            $false
        }
    })
}

function Close-RunningStudioPro {
    param([string]$ExecutablePath)

    $running = @(Get-RunningStudioPro $ExecutablePath)
    if ($running.Count -eq 0) { return }

    foreach ($studioProcess in $running) {
        if ($studioProcess.MainWindowHandle -ne [IntPtr]::Zero) {
            $null = $studioProcess.CloseMainWindow()
        }
    }

    $deadline = (Get-Date).AddSeconds(20)
    do {
        Start-Sleep -Milliseconds 500
        $running = @(Get-RunningStudioPro $ExecutablePath)
    } while ($running.Count -gt 0 -and (Get-Date) -lt $deadline)

    if ($running.Count -gt 0) {
        # The idle Sign In/Select App shells sometimes ignore WM_CLOSE. They
        # cannot contain an unsaved project, so they are safe to terminate.
        # Never force-close an actual project window.
        $safeWindowTitles = @('Mendix Studio Pro - Sign In', 'Mendix Studio Pro - Select App')
        $unsafeProcesses = @($running | Where-Object {
            $_.Refresh()
            -not [string]::IsNullOrWhiteSpace($_.MainWindowTitle) -and
                $safeWindowTitles -notcontains $_.MainWindowTitle
        })
        if ($unsafeProcesses.Count -gt 0) {
            throw 'Studio Pro is still running with a project open. Close it and try uninstalling again.'
        }
        foreach ($studioProcess in $running) {
            Stop-Process -Id $studioProcess.Id -Force
        }
        Start-Sleep -Seconds 2
        $running = @(Get-RunningStudioPro $ExecutablePath)
        if ($running.Count -gt 0) {
            throw 'Studio Pro is still running. Close it and try uninstalling again.'
        }
    }
}

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal] $identity
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'The WinBoat Windows session does not have administrator privileges.'
    }

    $studioPro = Find-StudioPro
    if ($null -ne $studioPro) {
        Close-RunningStudioPro $studioPro
    }

    $folder = Get-ChildItem -LiteralPath $dataRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $folder) {
        # Recover from an interrupted uninstall that removed its metadata but
        # could not delete a running Studio Pro executable. Find-StudioPro only
        # returns a matching child of the configured Mendix install root.
        Write-UninstallResult 'running' 'Removing files left by a partial uninstall.' $null $null
        if ($null -ne $studioPro) {
            $versionFolder = Split-Path -Parent (Split-Path -Parent $studioPro)
            Remove-Item -LiteralPath $versionFolder -Recurse -Force
        }
        $studioPro = Find-StudioPro
        if ($null -ne $studioPro) {
            throw "StudioPro.exe still exists after partial uninstall cleanup: $studioPro"
        }
        Write-UninstallResult 'succeeded' 'Studio Pro uninstall completed.' 0 $null
        exit 0
    }

    $uninstaller = Join-Path $folder.FullName 'uninst\unins000.exe'
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        throw "Uninstaller not found: $uninstaller"
    }

    Write-UninstallResult 'running' 'Studio Pro uninstaller is running.' $null $null
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/SILENT') -Wait -PassThru
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

struct WindowsOperationWaitError {
    message: String,
    retryable: bool,
}

async fn run_windows_operation(
    config: &AppConfig,
    script_path: &Path,
    label: &str,
    report_path: &Path,
    timeout_seconds: u64,
    operation: &str,
    keep_remote_app_alive: bool,
) -> Result<WindowsOperationReport, String> {
    for attempt in 0..REMOTE_APP_START_ATTEMPTS {
        let mut remote_app = spawn_powershell_file(config, script_path, label)?;
        match wait_for_windows_operation(report_path, &mut remote_app, timeout_seconds, operation)
            .await
        {
            Ok(report) => {
                if !keep_remote_app_alive {
                    stop_remote_app(&mut remote_app);
                }
                return Ok(report);
            }
            Err(error) => {
                stop_remote_app(&mut remote_app);
                if error.retryable && attempt + 1 < REMOTE_APP_START_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(REMOTE_APP_RETRY_DELAY_SECONDS)).await;
                    continue;
                }
                return Err(error.message);
            }
        }
    }
    unreachable!("the RemoteApp attempt loop always returns")
}

fn stop_remote_app(remote_app: &mut Child) {
    match remote_app.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = remote_app.kill();
            let _ = remote_app.wait();
        }
    }
}

async fn wait_for_windows_operation(
    report_path: &Path,
    remote_app: &mut Child,
    timeout_seconds: u64,
    operation: &str,
) -> Result<WindowsOperationReport, WindowsOperationWaitError> {
    let started = tokio::time::Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let mut remote_app_exited_at = None;
    let mut last_report_state = None;

    loop {
        if let Ok(content) = tokio::fs::read_to_string(report_path).await {
            if let Ok(report) = parse_install_report(&content) {
                last_report_state = Some(report.state.clone());
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
                        return Err(WindowsOperationWaitError {
                            message: format!(
                                "Windows에서 {operation}에 실패했습니다{exit_code}: {reason}"
                            ),
                            retryable: false,
                        });
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
                    return Err(WindowsOperationWaitError {
                        message: format!(
                            "{operation}용 WinBoat RemoteApp 상태를 확인하지 못했습니다: {error}"
                        ),
                        retryable: false,
                    });
                }
            }
        }

        if let Some((exited_at, status)) = remote_app_exited_at {
            if exited_at.elapsed() >= Duration::from_secs(REMOTE_APP_START_GRACE_SECONDS) {
                return match last_report_state.as_deref() {
                    Some(state) => Err(WindowsOperationWaitError {
                        message: format!(
                            "Windows가 {operation} 완료를 보고하기 전에 RemoteApp 연결이 종료되었습니다 (마지막 상태: {state}, FreeRDP 상태: {status})."
                        ),
                        retryable: false,
                    }),
                    None => Err(WindowsOperationWaitError {
                        message: format!(
                            "{operation} 명령이 Windows에서 시작되지 않았습니다 (FreeRDP 상태: {status})."
                        ),
                        retryable: true,
                    }),
                };
            }
        }

        if started.elapsed() >= timeout {
            return Err(WindowsOperationWaitError {
                message: format!(
                    "{operation}가 {}분 안에 완료되지 않았습니다. WinBoat Windows에서 상태를 확인해 주세요.",
                    timeout_seconds / 60
                ),
                retryable: false,
            });
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn parse_install_report(content: &str) -> Result<WindowsOperationReport, serde_json::Error> {
    serde_json::from_str(content.trim_start_matches('\u{feff}').trim())
}

fn spawn_powershell_file(
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
    .map_err(|error| format!("숨김 Windows 명령 래퍼를 저장할 수 없습니다: {error}"))?;
    Ok(launcher_path)
}

fn hidden_powershell_launcher(powershell_arguments: &str) -> String {
    format!(
        "Option Explicit\r\n\
         Dim shell, exitCode\r\n\
         Set shell = CreateObject(\"WScript.Shell\")\r\n\
         exitCode = shell.Run(\"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe {powershell_arguments}\", 0, True)\r\n\
         WScript.Quit exitCode\r\n"
    )
}

fn powershell_encoded_arguments(encoded_command: &str) -> String {
    format!(
        "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -EncodedCommand {encoded_command}"
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
        encode_powershell_script, hidden_powershell_launcher, install_script, install_studio,
        installed_versions, launch_studio, launch_studio_script, launch_uninstaller,
        parse_install_report, parse_studio_versions, powershell_encoded_arguments,
        uninstall_script, validate_version,
    };
    use crate::{
        config,
        models::{AppConfig, WinApp},
        projects::linux_path_to_windows_share,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use std::path::Path;

    fn live_e2e_context() -> (AppConfig, String) {
        assert_eq!(
            std::env::var("MENDIMARU_E2E_ALLOW_MUTATION").as_deref(),
            Ok("1"),
            "set MENDIMARU_E2E_ALLOW_MUTATION=1 to mutate the live WinBoat VM"
        );
        let version = std::env::var("MENDIMARU_E2E_VERSION")
            .expect("set MENDIMARU_E2E_VERSION to the exact test version");
        validate_version(&version).expect("the E2E version must be valid");
        let config = config::detect_config().expect("the live WinBoat configuration must exist");
        (config, version)
    }

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

        assert!(script.contains("-Wait -PassThru"));
        assert!(!script.contains("-Verb RunAs"));
        assert!(script.contains("WindowsBuiltInRole]::Administrator"));
        assert!(script.contains("CloseMainWindow()"));
        assert!(script.contains("Close-RunningStudioPro $studioPro"));
        assert!(script.contains("Mendix Studio Pro - Sign In"));
        assert!(script.contains("Mendix Studio Pro - Select App"));
        assert!(script.contains("Stop-Process -Id $studioProcess.Id -Force"));
        assert!(script.contains("with a project open"));
        assert!(script.contains("Close it and try uninstalling again"));
        assert!(script.contains("Removing files left by a partial uninstall"));
        assert!(script.contains("Remove-Item -LiteralPath $versionFolder -Recurse -Force"));
        assert!(script.contains("StudioPro.exe still exists after uninstall"));
        assert!(script.contains("'C:\\ProgramData\\Mendix'"));
        assert!(script.contains("'11.13.0'"));
        assert!(!script.contains("__VERSION__"));
    }

    #[test]
    fn installer_uses_the_existing_elevated_winboat_session() {
        let script = install_script(
            r"\\host.lan\Data\.mendimaru\installers\Mendix-11.13.0-Setup.exe",
            r"\\host.lan\Data\.mendimaru\operations\install.json",
            r"C:\Program Files\Mendix",
            "11.13.0",
        );

        assert!(script.contains("WindowsBuiltInRole]::Administrator"));
        assert!(script.contains("Join-Path $env:ProgramData 'Mendimaru\\Installers'"));
        assert!(script.contains("Copy-Item -LiteralPath $installer"));
        assert!(script.contains("Unblock-File -LiteralPath $localInstaller"));
        assert!(script.contains(
            "Start-Process -FilePath $localInstaller -ArgumentList @('/SILENT') -Wait -PassThru"
        ));
        assert!(script.contains("Remove-Item -LiteralPath $localInstaller"));
        assert!(!script.contains("-Verb RunAs"));
    }

    #[test]
    fn studio_launcher_waits_for_a_real_window_and_keeps_remote_app_alive() {
        let script = launch_studio_script(
            r"C:\Program Files\Mendix\11.13.0\modeler\studiopro.exe",
            Some(r"\\host.lan\Data\Orders\Orders.mpr"),
            r"\\host.lan\Data\.mendimaru\operations\launch.json",
        );

        assert!(script.contains("MainWindowHandle -ne [IntPtr]::Zero"));
        assert!(script.contains("Start-Process -FilePath $executable"));
        assert!(script.contains("Start-Sleep -Milliseconds 1200"));
        assert!(script.contains("Get-StudioProcesses"));
        assert!(script.contains("$studioProcesses.Count -gt 0"));
        assert!(script.contains("TotalSeconds -ge 15"));
        assert!(script.contains(r"'\\host.lan\Data\Orders\Orders.mpr'"));
        assert!(!script.contains("__EXECUTABLE_PATH__"));
    }

    #[test]
    fn powershell_file_launcher_stays_below_freerdp_rail_argument_limit() {
        let launcher = r"& '\\host.lan\Data\.mendimaru\commands\launch-11.12.2.ps1'";
        let encoded = encode_powershell_script(launcher);
        let arguments = powershell_encoded_arguments(&encoded);

        // TS_RAIL_ORDER_EXEC allows at most 16,000 bytes for Arguments.
        assert!(arguments.encode_utf16().count() * 2 < 16_000);
        assert!(arguments.contains("-EncodedCommand"));
        assert!(!arguments.contains("MainWindowHandle"));
    }

    #[test]
    fn windows_script_host_runs_powershell_hidden_and_waits_for_it() {
        let arguments = powershell_encoded_arguments("QQA=");
        let launcher = hidden_powershell_launcher(&arguments);

        assert!(launcher.contains("WScript.Shell"));
        assert!(launcher.contains("shell.Run("));
        assert!(launcher.contains(", 0, True)"));
        assert!(launcher.contains("WScript.Quit exitCode"));
        assert!(arguments.contains("-WindowStyle Hidden"));
        assert!(arguments.contains("-EncodedCommand QQA="));
        assert!(!arguments.contains("WindowStyle Normal"));
    }

    #[test]
    #[ignore = "installs Studio Pro in the live WinBoat VM"]
    fn live_e2e_install_studio() {
        let (config, version) = live_e2e_context();
        let installer_path = Path::new(&config.shared_directory)
            .join(".mendimaru/installers")
            .join(format!("Mendix-{version}-Setup.exe"));
        assert!(
            installer_path.is_file(),
            "cached installer does not exist: {}",
            installer_path.display()
        );
        let windows_installer_path = linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            &installer_path,
            &config.windows_shared_directory,
        )
        .expect("the cached installer path must map to the Windows share");

        let executable_path = tauri::async_runtime::block_on(install_studio(
            &config,
            &version,
            &windows_installer_path,
        ))
        .expect("Studio Pro installation must succeed");
        assert!(
            executable_path
                .to_ascii_lowercase()
                .ends_with("studiopro.exe"),
            "unexpected executable path: {executable_path}"
        );

        let installed = tauri::async_runtime::block_on(installed_versions(&config))
            .expect("installed versions must be readable after installation");
        assert!(
            installed.iter().any(|item| item.version == version),
            "the installed version list does not contain {version}"
        );
    }

    #[test]
    #[ignore = "launches Studio Pro in the live WinBoat VM"]
    fn live_e2e_launch_studio() {
        let (config, version) = live_e2e_context();
        let result = tauri::async_runtime::block_on(launch_studio(&config, &version, None))
            .expect("Studio Pro launch must succeed");

        assert_eq!(result.label, format!("Studio Pro {version}"));
        assert!(
            result
                .executable_path
                .to_ascii_lowercase()
                .ends_with("studiopro.exe"),
            "unexpected executable path: {}",
            result.executable_path
        );
    }

    #[test]
    #[ignore = "uninstalls Studio Pro from the live WinBoat VM"]
    fn live_e2e_uninstall_studio() {
        let (config, version) = live_e2e_context();
        tauri::async_runtime::block_on(launch_uninstaller(&config, &version))
            .expect("Studio Pro uninstall must succeed");

        let installed = tauri::async_runtime::block_on(installed_versions(&config))
            .expect("installed versions must be readable after uninstall");
        assert!(
            installed.iter().all(|item| item.version != version),
            "the installed version list still contains {version}"
        );
    }
}
