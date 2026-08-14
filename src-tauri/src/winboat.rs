mod client;
mod container;
mod operation;
mod remote_app;
mod scripts;
mod studio;

pub use client::installed_versions;
pub use container::{
    environment_status, guest_is_online, open_winboat, recreate_container, start_container,
};
pub use studio::{
    install_studio, launch_studio, launch_uninstaller, open_linux_folder, validate_version,
    StudioInstallPhase, StudioInstallProgress,
};

#[cfg(test)]
use client::parse_studio_versions;
#[cfg(test)]
use operation::{localize_windows_reason, parse_install_report};
#[cfg(test)]
use remote_app::{
    encode_powershell_script, hidden_powershell_launcher, powershell_encoded_arguments,
};
#[cfg(test)]
use scripts::{install_script, launch_studio_script, uninstall_script};

#[cfg(test)]
mod tests {
    use super::{
        encode_powershell_script, hidden_powershell_launcher, install_script, install_studio,
        installed_versions, launch_studio, launch_studio_script, launch_uninstaller,
        localize_windows_reason, parse_install_report, parse_studio_versions,
        powershell_encoded_arguments, uninstall_script, validate_version,
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
        crate::i18n::initialize("en-US").expect("English localization initializes");
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
            "\u{feff}{\"state\":\"installing\",\"message\":\"working\",\"percentage\":42.5,\"estimated\":false,\"exitCode\":null,\"executablePath\":null,\"error\":null}",
        )
        .expect("report should parse");
        assert_eq!(
            report.state,
            super::operation::WindowsOperationState::Installing
        );
        assert_eq!(report.percentage, Some(42.5));
        assert!(!report.estimated);
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
        assert!(script.contains("MENDIMARU_PROJECT_STILL_OPEN"));
        assert!(script.contains("MENDIMARU_STUDIO_STILL_RUNNING"));
        assert!(script.contains("Removing files left by a partial uninstall"));
        assert!(script.contains("Remove-Item -LiteralPath $versionFolder -Recurse -Force"));
        assert!(script.contains("MENDIMARU_UNINSTALL_STILL_EXISTS"));
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
        assert!(script.contains("Copy-InstallerWithProgress $installer $localInstaller"));
        assert!(script.contains("Unblock-File -LiteralPath $localInstaller"));
        assert!(script.contains("Mendimaru.NativeProgressReader"));
        assert!(script.contains("PBM_GETPOS"));
        assert!(script.contains("TNewProgressBar"));
        assert!(script.contains("IsWindowVisible(window)"));
        assert!(script.contains("'/SILENT'"));
        assert!(script.contains("'/SUPPRESSMSGBOXES'"));
        assert!(script.contains("'/NORESTART'"));
        assert!(script.contains(") -PassThru"));
        assert!(!script.contains(") -Wait -PassThru"));
        assert!(script.contains("Write-InstallResult 'verifying'"));
        assert!(script.contains("Write-InstallResult 'succeeded'"));
        assert!(script.contains("Remove-Item -LiteralPath $localInstaller"));
        assert!(!script.contains("-Verb RunAs"));
    }

    #[test]
    fn converts_script_error_codes_to_current_language() {
        crate::i18n::initialize("en-US").expect("English localization initializes");
        let localized =
            localize_windows_reason(r"MENDIMARU_INSTALLER_NOT_FOUND:C:\Missing\StudioPro.exe");
        assert!(localized.contains(r"C:\Missing\StudioPro.exe"));
        assert!(!localized.contains("MENDIMARU_INSTALLER_NOT_FOUND"));

        let localized = localize_windows_reason("MENDIMARU_ADMIN_REQUIRED");
        assert!(!localized.contains("MENDIMARU_ADMIN_REQUIRED"));
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
            |progress| {
                eprintln!(
                    "install progress: state={:?} percentage={:?} estimated={}",
                    progress.phase.download_state(),
                    progress.percentage,
                    progress.estimated
                );
            },
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
        tauri::async_runtime::block_on(launch_studio(&config, &version, None))
            .expect("Studio Pro launch must succeed");
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
