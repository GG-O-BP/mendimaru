//! Opt-in real VM gate. Page content is a host HTTP fixture; VM inspection and
//! Docker lifecycle changes are real. This does not claim a real Studio F5 run.
use super::*;
use crate::contracts::{BackendId, BrowserTestOutcome, BrowserTestPolicy};

#[test]
#[ignore = "requires exclusive disposable WinBoat and verified snapshot opt-ins"]
fn live_environment_generation_gate() {
    assert_eq!(
        std::env::var("MENDIMARU_E2E_ALLOW_MUTATION").as_deref(),
        Ok("1")
    );
    let snapshot = std::env::var("MENDIMARU_E2E_DISPOSABLE_SNAPSHOT")
        .expect("verified disposable snapshot acknowledgement");
    assert!(!snapshot.trim().is_empty());
    assert!(std::env::var_os("MENDIMARU_E2E_VERSION").is_none());
    crate::i18n::initialize("en-US").unwrap();
    let paths = crate::app_paths::AppPaths::discover_for_cli().unwrap();
    let config = crate::application::load_config(&paths).unwrap();
    assert!(
        config.container_name.starts_with("Mendimaru154"),
        "use a dedicated issue-154 disposable VM"
    );
    let output =
        PathBuf::from(std::env::var_os("MENDIMARU_E2E_OBSERVATION_REPORT").expect("report path"));
    let compose = std::fs::read(&config.compose_file).unwrap();
    struct Restore<'a>(&'a str, &'a [u8]);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            std::fs::write(self.0, self.1).unwrap();
        }
    }
    let _restore = Restore(&config.compose_file, &compose);
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("build-generation");
    std::fs::write(&marker, "prepared-build-one").unwrap();
    let suite = root.path().join("live.browser.json");
    std::fs::write(&suite, r#"{"schemaVersion":"1.0.0","name":"VM generation gate","beforeEach":[{"action":"goto","path":"/"}],"tests":[{"name":"fixture page","steps":[{"action":"expectText","locator":{"by":"text","value":"ok","exact":true},"value":"ok"}]}]}"#).unwrap();
    tauri::async_runtime::block_on(async {
        let lease = crate::winboat::vm_use::acquire(
            &config,
            crate::winboat::vm_use::Mode::Exclusive,
            CapabilityId::BrowserTest,
        )
        .await
        .unwrap();
        lease.run(async {
            crate::winboat::startup::ensure_guest_online(&config).await.expect("restored disposable guest must boot and become healthy");
            let mut reports = Vec::new();
            for scenario in ["read-only", "compose", "recreate", "ports", "build"] {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let url = format!("http://{}/", listener.local_addr().unwrap());
                let cfg = config.clone();
                let marker_copy = marker.clone();
                let original = compose.clone();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0;2048]; let read = stream.read(&mut request).await.unwrap();
                    assert!(read > 0);
                    if scenario == "compose" {
                        let mut changed = original.clone(); changed.extend_from_slice(b"\n# disposable generation gate\n");
                        std::fs::write(&cfg.compose_file, changed).unwrap();
                    } else if scenario == "build" {
                        let next = marker_copy.with_extension("next");
                        std::fs::write(&next, "prepared-build-one").unwrap();
                        std::fs::rename(next, &marker_copy).unwrap();
                    } else if scenario == "ports" {
                        let mut value: serde_yaml::Value = serde_yaml::from_slice(&original).unwrap();
                        let service = value["services"].as_mapping_mut().unwrap().values_mut().next().unwrap();
                        service["ports"].as_sequence_mut().unwrap().push(serde_yaml::Value::String("127.0.0.1::18080".into()));
                        std::fs::write(&cfg.compose_file, serde_yaml::to_string(&value).unwrap()).unwrap();
                    }
                    if matches!(scenario, "recreate" | "ports") {
                        // Deliberately emulate an external controller: bypass
                        // mendimaru's lifecycle calls while our test owns the VM.
                        let mut command = tokio::process::Command::new(cfg.container_runtime.as_str());
                        command.args(["compose", "-f", &cfg.compose_file, "up", "-d", "--force-recreate"]);
                        let result = process::output(command, CommandPolicy::new(Duration::from_secs(150), 65536), None, "disposable VM recreation").await.unwrap();
                        assert!(result.status.success(), "disposable recreation failed");
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
                    // Browser may request favicon on the same origin.
                    while let Ok(Ok((mut stream, _))) = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await {
                        let _ = stream.read(&mut request).await;
                        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await;
                    }
                });
                let result = crate::application::browser_test_url(BackendId::LinuxWinboat, Some(&config), marker.to_str(), &url, suite.to_str().unwrap(), BrowserTestPolicy {
                    navigation_timeout_milliseconds: 10000, action_timeout_milliseconds: 3000, assertion_timeout_milliseconds: 3000,
                    fail_on_console_error: false, fail_on_network_failure: false, record_video: false, record_har: false, max_artifact_bytes: 32*1024*1024, retention_runs: 20,
                }).await;
                server.await.unwrap();
                let after_controller = Source { config: config.clone(), management: crate::winboat::vm_use::observation(), runtime_id: None, studio_id: None, build: None, build_requested: false }.snapshot().await;
                std::fs::write(&config.compose_file, &compose).unwrap();
                let result = result.unwrap();
                let report = result.environment.as_ref().unwrap();
                assert!(report.valid());
                assert!(report.baseline.container.is_some() && report.baseline.compose.is_some() && report.baseline.published_ports.is_some());
                assert_eq!(report.missing, vec![Component::Runtime, Component::Studio]);
                if scenario == "read-only" {
                    assert_eq!(result.outcome, BrowserTestOutcome::Passed);
                    assert!(report.events.is_empty());
                    assert_eq!(report.baseline.container, report.latest.container);
                    assert_eq!(report.baseline.compose, report.latest.compose);
                    assert_eq!(report.baseline.published_ports, report.latest.published_ports);
                } else {
                    assert_eq!(result.outcome, BrowserTestOutcome::Failed);
                    let expected = match scenario { "compose" => Component::Compose, "build" => Component::Build, _ => Component::Container };
                    assert!(report.events.iter().any(|e| e.component == expected), "{scenario}: {report:?}");
                }
                if scenario == "ports" {
                    assert!(after_controller.published_ports.is_some());
                    assert_ne!(report.baseline.published_ports, after_controller.published_ports);
                }
                if matches!(scenario, "ports" | "recreate") {
                    assert!(after_controller.container.is_some());
                    assert_ne!(report.baseline.container, after_controller.container);
                }
                reports.push(serde_json::json!({"scenario":scenario,"summary":result,"afterController":after_controller}));
                std::fs::write(&output, serde_json::to_vec_pretty(&serde_json::json!({"restoredGuestBootVerified":true,"runs":reports})).unwrap()).unwrap();
            }
        }).await;
    });
}
