mod client;
mod container;
mod operation;
mod remote_app;
mod scripts;
mod security;
mod studio;

pub use client::installed_versions;
pub use container::{
    environment_status, guest_is_online, open_winboat, recreate_container, start_container,
};
pub(crate) use operation::WindowsOperationFailure;
pub use studio::{install_studio, launch_studio, launch_uninstaller, open_linux_folder};

#[cfg(test)]
use client::parse_studio_versions;
#[cfg(test)]
use operation::{localize_windows_reason, parse_install_report};
#[cfg(test)]
use remote_app::{
    encode_powershell_script, powershell_encoded_arguments, FREERDP_CERTIFICATE_POLICY,
};
#[cfg(test)]
use scripts::{install_script, launch_studio_script, uninstall_script};
#[cfg(test)]
use scripts::{reparse_probe_script, security_probe_script};
#[cfg(test)]
use security::{secure_powershell_launcher, OperationSecurity};

#[cfg(test)]
mod tests {
    use super::operation::{run_windows_operation, WindowsOperationRequest};
    use super::{
        encode_powershell_script, install_script, install_studio, installed_versions,
        launch_studio, launch_studio_script, launch_uninstaller, localize_windows_reason,
        parse_install_report, parse_studio_versions, powershell_encoded_arguments,
        reparse_probe_script, secure_powershell_launcher, security_probe_script, uninstall_script,
        OperationSecurity, FREERDP_CERTIFICATE_POLICY,
    };
    use crate::{
        config,
        models::{AppConfig, WinApp},
        platform::validate_version,
        projects::linux_path_to_windows_share,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use sha2::{Digest, Sha256};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

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

    fn file_sha256(path: &Path) -> String {
        let mut file = File::open(path).expect("open E2E installer");
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = file.read(&mut buffer).expect("hash E2E installer");
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        format!("{:x}", digest.finalize())
    }

    fn random_test_id(prefix: &str) -> String {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).expect("generate E2E fixture ID");
        format!(
            "{prefix}-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }

    fn run_live_security_probe(
        config: &AppConfig,
        target: &str,
        root: &str,
        expected_sha256: &str,
        timeout_seconds: u64,
    ) -> Result<super::operation::WindowsOperationReport, String> {
        run_live_script_probe(config, timeout_seconds, |windows_report_path| {
            security_probe_script(target, root, expected_sha256, windows_report_path)
        })
    }

    fn run_live_script_probe<F>(
        config: &AppConfig,
        timeout_seconds: u64,
        build_script: F,
    ) -> Result<super::operation::WindowsOperationReport, String>
    where
        F: FnOnce(&str) -> String,
    {
        run_live_script_probe_inner(config, timeout_seconds, false, build_script)
    }

    fn run_live_script_probe_inner<F>(
        config: &AppConfig,
        timeout_seconds: u64,
        tamper_script: bool,
        build_script: F,
    ) -> Result<super::operation::WindowsOperationReport, String>
    where
        F: FnOnce(&str) -> String,
    {
        let id = random_test_id("security-probe");
        let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
        let command_directory = Path::new(&config.shared_directory).join(".mendimaru/commands");
        std::fs::create_dir_all(&operation_directory).expect("create probe operation directory");
        std::fs::create_dir_all(&command_directory).expect("create probe command directory");
        let report_path = operation_directory.join(format!("{id}.json"));
        let windows_report_path = linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            &report_path,
            &config.windows_shared_directory,
        )
        .expect("map probe report path");
        let script = build_script(&windows_report_path);
        let script_sha256 = format!("{:x}", Sha256::digest(script.as_bytes()));
        let script_path = command_directory.join(format!("{id}.ps1"));
        let mut script_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&script_path)
            .expect("create security probe script");
        script_file
            .write_all(script.as_bytes())
            .and_then(|_| script_file.sync_all())
            .expect("persist security probe script");
        if tamper_script {
            script_file
                .seek(SeekFrom::Start(0))
                .and_then(|_| script_file.write_all(b"#"))
                .and_then(|_| script_file.sync_all())
                .expect("tamper probe script without changing its length");
        }
        drop(script_file);

        let result = tauri::async_runtime::block_on(run_windows_operation(
            config,
            WindowsOperationRequest {
                script_path: &script_path,
                script_sha256: &script_sha256,
                label: "Mendimaru Security Probe",
                report_path: &report_path,
                timeout_seconds,
                operation: "running a WinBoat security probe",
                keep_remote_app_alive: false,
            },
            |_| {},
        ));
        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_file(&report_path);
        let mut report_temporary = report_path.as_os_str().to_os_string();
        report_temporary.push(".tmp");
        let _ = std::fs::remove_file(PathBuf::from(report_temporary));
        result.map_err(|error| error.message)
    }

    struct DirectoryCleanup(PathBuf);

    impl Drop for DirectoryCleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct FileRestore {
        path: PathBuf,
        content: Vec<u8>,
    }

    impl Drop for FileRestore {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.path, &self.content);
        }
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
        assert!(validate_version("11.6.0-beta.1").is_ok());
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
            "\u{feff}{\"state\":\"installing\",\"message\":\"working\",\"percentage\":42.5,\"estimated\":false,\"timestamp\":\"2026-08-14T00:00:00Z\",\"exitCode\":null,\"executablePath\":null,\"error\":null}".as_bytes(),
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
        assert!(script.contains("MENDIMARU_UNINSTALL_METADATA_MISSING"));
        assert!(!script.contains("Remove-Item -LiteralPath $versionFolder -Recurse -Force"));
        assert!(script.contains("Assert-MendimaruTrustedExecutable -Path $uninstaller"));
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
            &"ab".repeat(32),
            r"\\host.lan\Data",
        );

        assert!(script.contains("WindowsBuiltInRole]::Administrator"));
        assert!(script.contains("Join-Path $env:ProgramData 'Mendimaru\\Installers'"));
        assert!(script.contains("Copy-InstallerWithProgress $installer $localInstaller"));
        assert!(!script.contains("Unblock-File"));
        assert!(script.contains("ExpectedSha256 $expectedSha256"));
        assert!(script.contains("[System.IO.FileMode]::CreateNew"));
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
            r"C:\Program Files\Mendix",
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
        let security = OperationSecurity::fixture();
        let launcher = secure_powershell_launcher(
            r"\\host.lan\Data\.mendimaru\commands\launch-11.12.2.ps1",
            &security,
        );
        let encoded = encode_powershell_script(&launcher);
        let arguments = powershell_encoded_arguments(&encoded);

        // TS_RAIL_ORDER_EXEC allows at most 16,000 bytes for Arguments.
        assert!(arguments.encode_utf16().count() * 2 < 16_000);
        assert!(arguments.contains("-EncodedCommand"));
        assert!(!arguments.contains("MainWindowHandle"));
        assert!(arguments.contains("ExecutionPolicy RemoteSigned"));
    }

    #[test]
    fn windows_script_host_runs_private_copy_hidden_without_bypass() {
        let arguments = powershell_encoded_arguments("QQA=");
        assert!(arguments.contains("-WindowStyle Hidden"));
        assert!(arguments.contains("-EncodedCommand QQA="));
        assert!(arguments.contains("-ExecutionPolicy RemoteSigned"));
        assert!(!arguments.contains("Bypass"));
        assert!(!arguments.contains("WindowStyle Normal"));
        assert_eq!(FREERDP_CERTIFICATE_POLICY, "/cert:tofu");
        assert_ne!(FREERDP_CERTIFICATE_POLICY, "/cert:ignore");
    }

    #[test]
    #[ignore = "runs destructive security rejection fixtures in the live WinBoat VM"]
    fn live_e2e_rejects_untrusted_and_same_length_executables() {
        let (config, version) = live_e2e_context();
        let fixture_directory = Path::new(&config.shared_directory)
            .join(".mendimaru/security-fixtures")
            .join(random_test_id("trust"));
        std::fs::create_dir_all(&fixture_directory).expect("create security fixture directory");
        let _cleanup = DirectoryCleanup(fixture_directory.clone());

        let unsigned = fixture_directory.join("unsigned.exe");
        let mut unsigned_payload = vec![0_u8; 4096];
        unsigned_payload[..2].copy_from_slice(b"MZ");
        std::fs::write(&unsigned, &unsigned_payload).expect("write unsigned fixture");
        let windows_unsigned = linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            &unsigned,
            &config.windows_shared_directory,
        )
        .expect("map unsigned fixture");
        let unsigned_error = run_live_security_probe(
            &config,
            &windows_unsigned,
            &config.windows_shared_directory,
            &file_sha256(&unsigned),
            120,
        )
        .expect_err("an unsigned executable must be rejected");
        assert!(unsigned_error.to_ascii_lowercase().contains("authenticode"));

        let wrong_publisher_error = run_live_security_probe(
            &config,
            r"C:\Windows\System32\notepad.exe",
            r"C:\Windows",
            "",
            120,
        )
        .expect_err("a Microsoft-published executable must not enter the allowlist");
        assert!(wrong_publisher_error
            .to_ascii_lowercase()
            .contains("publisher"));

        let installer = Path::new(&config.shared_directory)
            .join(".mendimaru/installers")
            .join(format!("Mendix-{version}-Setup.exe"));
        assert!(installer.is_file(), "missing live installer fixture");
        let trusted_digest = file_sha256(&installer);
        let tampered = fixture_directory.join("same-length-tampered.exe");
        std::fs::copy(&installer, &tampered).expect("copy signed installer fixture");
        let mut tampered_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tampered)
            .expect("open copied installer fixture");
        tampered_file
            .seek(SeekFrom::Start(4096))
            .expect("seek into copied installer");
        let mut changed = [0_u8; 1];
        tampered_file
            .read_exact(&mut changed)
            .expect("read copied installer byte");
        changed[0] ^= 0x01;
        tampered_file
            .seek(SeekFrom::Start(4096))
            .and_then(|_| tampered_file.write_all(&changed))
            .and_then(|_| tampered_file.sync_all())
            .expect("tamper copied installer without changing its length");
        drop(tampered_file);
        assert_eq!(
            std::fs::metadata(&installer)
                .expect("source metadata")
                .len(),
            std::fs::metadata(&tampered)
                .expect("tampered metadata")
                .len()
        );
        let windows_tampered = linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            &tampered,
            &config.windows_shared_directory,
        )
        .expect("map tampered fixture");
        let mismatch_error = run_live_security_probe(
            &config,
            &windows_tampered,
            &config.windows_shared_directory,
            &trusted_digest,
            120,
        )
        .expect_err("a same-length replacement must fail its host digest");
        assert!(mismatch_error.contains("SHA-256"));

        let invalid_signature_error = run_live_security_probe(
            &config,
            &windows_tampered,
            &config.windows_shared_directory,
            &file_sha256(&tampered),
            120,
        )
        .expect_err("a hash-matching executable with a broken signature must be rejected");
        assert!(invalid_signature_error
            .to_ascii_lowercase()
            .contains("authenticode"));
    }

    #[test]
    #[ignore = "temporarily replaces the app-scoped FreeRDP pin in the live environment"]
    fn live_e2e_rejects_an_rdp_certificate_pin_mismatch() {
        let (config, _version) = live_e2e_context();
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .expect("locate user configuration directory");
        let pin = base
            .join("mendimaru/freerdp-config/freerdp/server")
            .join(format!(
                "{}_{}.pem",
                config.rdp_host,
                crate::config::resolved_rdp_port(&config)
            ));
        let original = std::fs::read(&pin).expect("a prior live launch must create the TOFU pin");
        let restore = FileRestore {
            path: pin.clone(),
            content: original.clone(),
        };
        let text = std::str::from_utf8(&original).expect("certificate pin is PEM text");
        let body = text
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<String>();
        let mut certificate = BASE64_STANDARD
            .decode(body)
            .expect("certificate pin body is base64");
        let final_byte = certificate.last_mut().expect("certificate is not empty");
        *final_byte ^= 0x01;
        let encoded = BASE64_STANDARD.encode(certificate);
        let wrapped = encoded
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).expect("base64 is UTF-8"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            &pin,
            format!("-----BEGIN CERTIFICATE-----\n{wrapped}\n-----END CERTIFICATE-----\n"),
        )
        .expect("replace certificate pin with a different fingerprint");

        let result = run_live_security_probe(
            &config,
            r"C:\Windows\System32\notepad.exe",
            r"C:\Windows",
            "",
            10,
        );
        drop(restore);
        let error = result
            .expect_err("a mismatched pinned certificate must block the privileged operation");
        assert!(
            error.contains("certificate does not match"),
            "certificate mismatch was not classified: {error}"
        );
    }

    #[test]
    #[ignore = "creates and removes a junction fixture in the live WinBoat VM"]
    fn live_e2e_rejects_a_reparse_point_escape() {
        let (config, _version) = live_e2e_context();
        run_live_script_probe(&config, 60, reparse_probe_script)
            .expect("a junction in the trusted path must be detected and rejected");
    }

    #[test]
    #[ignore = "changes a shared command after its expected hash is fixed"]
    fn live_e2e_rejects_a_tampered_shared_command() {
        let (config, _version) = live_e2e_context();
        let error = run_live_script_probe_inner(&config, 10, true, reparse_probe_script)
            .expect_err("a modified shared command must never produce a successful report");
        assert!(error.contains("did not finish") || error.contains("did not start"));
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
        let expected_sha256 = file_sha256(&installer_path);

        let executable_path = tauri::async_runtime::block_on(install_studio(
            &config,
            &version,
            &format!("install-{version}-live-e2e"),
            &windows_installer_path,
            &expected_sha256,
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
        tauri::async_runtime::block_on(launch_studio(
            &config,
            &version,
            &format!("launch-{version}-live-e2e"),
            None,
        ))
        .expect("Studio Pro launch must succeed");
    }

    #[test]
    #[ignore = "uninstalls Studio Pro from the live WinBoat VM"]
    fn live_e2e_uninstall_studio() {
        let (config, version) = live_e2e_context();
        tauri::async_runtime::block_on(launch_uninstaller(
            &config,
            &version,
            &format!("uninstall-{version}-live-e2e"),
        ))
        .expect("Studio Pro uninstall must succeed");

        let installed = tauri::async_runtime::block_on(installed_versions(&config))
            .expect("installed versions must be readable after uninstall");
        assert!(
            installed.iter().all(|item| item.version != version),
            "the installed version list still contains {version}"
        );
    }
}
