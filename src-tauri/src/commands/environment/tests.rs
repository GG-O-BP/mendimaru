#![cfg(target_os = "linux")]
use super::*;
use crate::app_paths::AppPaths;
use crate::models::{AppConfig, ContainerRuntime};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fixture() -> (
    tempfile::TempDir,
    AppPaths,
    AppConfig,
    tokio::task::JoinHandle<()>,
) {
    let root = tempfile::tempdir().unwrap();
    let compose_dir = root.path().join("winboat");
    fs::create_dir(&compose_dir).unwrap();
    fs::set_permissions(&compose_dir, fs::Permissions::from_mode(0o775)).unwrap();
    let name = crate::contracts::secure_identifier("vm").unwrap();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let compose = compose_dir.join("docker-compose.yml");
    fs::write(&compose, format!("services:\n  windows:\n    image: ghcr.io/dockur/windows:6.05\n    labels:\n      io.winboat.managed: 'true'\n    container_name: {name}\n    volumes:\n      - data:/storage\n      - {}:/shared\n", shared.display())).unwrap();
    fs::set_permissions(&compose, fs::Permissions::from_mode(0o664)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let health = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 2048];
            let _ = socket.read(&mut buf).await;
            let body = "{\"status\":\"ok\"}";
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let config = AppConfig {
        language_preference: "en-US".into(),
        winboat_setup_pending: true,
        winboat_executable: "/usr/bin/true".into(),
        compose_file: compose.to_string_lossy().into(),
        container_runtime: ContainerRuntime::Docker,
        container_name: name,
        api_url: format!("http://{address}"),
        rdp_host: "127.0.0.1".into(),
        rdp_port: 47300,
        shared_directory: shared.to_string_lossy().into(),
        windows_shared_directory: r"\\host.lan\Data".into(),
        freerdp_binary: "/usr/bin/true".into(),
        mendix_install_root: r"C:\Program Files\Mendix".into(),
        mendix_data_root: r"C:\ProgramData\Mendix".into(),
        windows_studio_paths: vec![],
        startup_timeout_seconds: 5,
    };
    let paths = AppPaths::for_tests(root.path().join("config"), root.path().join("cache"));
    crate::config::persist_config_from(&paths, &config).unwrap();
    (root, paths, config, health)
}

#[tokio::test]
async fn first_setup_and_repeat_settings_save_accept_775_without_recreating_or_chmod() {
    crate::i18n::initialize("en-US").unwrap();
    let (_root, paths, config, health) = fixture().await;
    let compose_before = fs::read(&config.compose_file).unwrap();
    let result = complete_winboat_setup_with_config(&paths, config.clone(), config.clone())
        .await
        .unwrap();
    assert!(!result.config.winboat_setup_pending);
    assert!(!result.mount_changed && !result.container_recreated);
    let loaded = crate::config::load_config_from(&paths).unwrap();
    assert!(!loaded.winboat_setup_pending);
    assert_eq!(loaded.language_preference, config.language_preference);
    assert_eq!(loaded.shared_directory, config.shared_directory);
    let revision = crate::settings::preview_settings_save(loaded.clone(), true)
        .unwrap()
        .unwrap()
        .compose_revision;
    let saved = crate::settings::save_settings_from(&paths, loaded, true, Some(revision))
        .await
        .unwrap();
    assert!(!saved.mount_changed && !saved.container_recreated);
    assert_eq!(fs::read(&config.compose_file).unwrap(), compose_before);
    assert_eq!(
        fs::metadata(std::path::Path::new(&config.compose_file).parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o775
    );
    assert_eq!(
        fs::metadata(&config.compose_file).unwrap().mode() & 0o777,
        0o664
    );
    assert!(!std::path::Path::new(&config.compose_file)
        .parent()
        .unwrap()
        .join(".mendimaru-maintenance.lock")
        .exists());
    assert!(
        complete_winboat_setup_with_config(&paths, saved.config.clone(), saved.config)
            .await
            .is_err()
    );
    health.abort();
}

#[tokio::test]
async fn setup_failure_keeps_pending_and_settings_reports_its_own_capability() {
    crate::i18n::initialize("en-US").unwrap();
    let (_root, paths, config, health) = fixture().await;
    let original = fs::read(paths.config_directory().join("config.json")).unwrap();
    let compose = fs::read(&config.compose_file).unwrap();
    // Preempt only this fixture VM's private maintenance file.
    let lock = crate::winboat::maintenance::private_lock_path(&config, false);
    drop(crate::winboat::maintenance::shared(&config).unwrap());
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o640)).unwrap();
    let error = complete_winboat_setup_with_config(&paths, config.clone(), config.clone())
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        crate::models::CommandErrorCode::CoordinationUntrusted
    );
    let details = error.details.unwrap();
    assert_eq!(
        details.capability,
        Some(crate::contracts::CapabilityId::EnvironmentConfigure)
    );
    assert!(!details.retryable);
    assert_eq!(
        fs::read(paths.config_directory().join("config.json")).unwrap(),
        original
    );
    assert_eq!(fs::read(&config.compose_file).unwrap(), compose);
    assert!(
        crate::config::load_config_from(&paths)
            .unwrap()
            .winboat_setup_pending
    );
    fs::set_permissions(lock, fs::Permissions::from_mode(0o600)).unwrap();
    health.abort();
}

#[tokio::test]
#[ignore = "explicit opt-in: complete the actual pending setup only when no Compose/VM change is required"]
async fn live_complete_pending_setup_without_vm_mutation() {
    assert_eq!(
        std::env::var("MENDIMARU_E2E_COMPLETE_SETUP").as_deref(),
        Ok("1")
    );
    crate::i18n::initialize("en-US").unwrap();
    let paths = AppPaths::discover_for_cli().unwrap();
    let preferred = crate::config::load_config_from(&paths).unwrap();
    assert!(
        preferred.winboat_setup_pending,
        "do not rewrite completed setup"
    );
    let detected = crate::config::detect_config().unwrap();
    let preview = crate::settings::preview_settings_save(detected.clone(), true)
        .unwrap()
        .unwrap();
    assert!(
        !preview.mount_changed && !preview.container_will_recreate,
        "refuse a live VM mutation"
    );
    assert_eq!(detected.compose_file, preferred.compose_file);
    assert_eq!(detected.shared_directory, preferred.shared_directory);
    let before = fs::read(&preferred.compose_file).unwrap();
    let mode = fs::metadata(
        std::path::Path::new(&preferred.compose_file)
            .parent()
            .unwrap(),
    )
    .unwrap()
    .mode();
    let result = complete_winboat_setup_from(&paths).await.unwrap();
    assert!(
        !result.config.winboat_setup_pending
            && !result.container_recreated
            && !result.mount_changed
    );
    assert!(
        !crate::config::load_config_from(&paths)
            .unwrap()
            .winboat_setup_pending
    );
    assert_eq!(fs::read(&preferred.compose_file).unwrap(), before);
    assert_eq!(
        fs::metadata(
            std::path::Path::new(&preferred.compose_file)
                .parent()
                .unwrap()
        )
        .unwrap()
        .mode(),
        mode
    );
}
