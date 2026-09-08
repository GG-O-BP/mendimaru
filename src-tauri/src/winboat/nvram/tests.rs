use super::*;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, Plan) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(TARGET), vec![0; VARS_BYTES]).unwrap();
    std::fs::write(
        root.path().join("data.img"),
        b"Windows data disk must never change",
    )
    .unwrap();
    std::fs::write(
        root.path().join("compose.yml"),
        b"Compose must never change",
    )
    .unwrap();
    let directory = open_directory(root.path()).unwrap();
    let (file, original) = read_vars(&directory, TARGET).unwrap();
    let metadata = file.metadata().unwrap();
    let directory_metadata = directory.dir_metadata().unwrap();
    let plan = Plan {
        preview: Preview {
            id: "fixture".into(),
            target_path: TARGET.into(),
            backup_path: "backup.bak".into(),
            original_path: "original.saved".into(),
            bytes: VARS_BYTES,
            sha256: format!("{:x}", Sha256::digest(&original)),
            evidence: "erased-ovmf-variable-store",
        },
        created: Instant::now(),
        compose_file: root
            .path()
            .join("compose.yml")
            .to_string_lossy()
            .into_owned(),
        mount: NvramMountPlan {
            directory: root.path().into(),
            service: "windows".into(),
            image: "ghcr.io/dockur/windows:6.03".into(),
            revision: "fixture".into(),
        },
        container_id: "a".repeat(64),
        image_id: format!("sha256:{}", "b".repeat(64)),
        directory,
        directory_identity: (directory_metadata.dev(), directory_metadata.ino()),
        original,
        file_identity: (metadata.dev(), metadata.ino()),
        backup: "backup.bak".into(),
        retired: "original.saved".into(),
        replaced: false,
    };
    (root, plan)
}

fn protected_unchanged(root: &Path) {
    assert_eq!(
        std::fs::read(root.join("data.img")).unwrap(),
        b"Windows data disk must never change"
    );
    assert_eq!(
        std::fs::read(root.join("compose.yml")).unwrap(),
        b"Compose must never change"
    );
}

fn config(root: &Path) -> AppConfig {
    AppConfig {
        language_preference: "en-US".into(),
        winboat_setup_pending: false,
        winboat_executable: "winboat".into(),
        compose_file: root.join("compose.yml").to_string_lossy().into_owned(),
        container_runtime: crate::models::ContainerRuntime::Docker,
        container_name: "WinBoat".into(),
        api_url: "http://127.0.0.1:47280".into(),
        rdp_host: "127.0.0.1".into(),
        rdp_port: 47300,
        shared_directory: root.to_string_lossy().into_owned(),
        windows_shared_directory: r"\\host.lan\Data".into(),
        freerdp_binary: "xfreerdp3".into(),
        mendix_install_root: r"C:\Program Files\Mendix".into(),
        mendix_data_root: r"C:\ProgramData\Mendix".into(),
        windows_studio_paths: vec![],
        startup_timeout_seconds: 5,
    }
}

#[test]
fn nvram_maintenance_excludes_operations_and_refuses_a_symlinked_lock() {
    let (root, _plan) = fixture();
    let config = config(root.path());
    let reader = super::super::maintenance::shared(&config).unwrap();
    assert!(super::super::maintenance::acquire(&config, true).is_err());
    drop(reader);
    let writer = super::super::maintenance::acquire(&config, true).unwrap();
    assert!(super::super::maintenance::shared(&config).is_err());
    drop(writer);
    let lock = root.path().join(".mendimaru-maintenance.lock");
    std::fs::remove_file(&lock).unwrap();
    symlink(root.path().join("data.img"), lock).unwrap();
    assert!(super::super::maintenance::shared(&config).is_err());
    protected_unchanged(root.path());
}

#[tokio::test]
async fn nvram_unconfirmed_and_ambiguous_failures_never_prepare_files() {
    let (root, _plan) = fixture();
    let config = config(root.path());
    assert!(recover(&config, "not-a-preview", false).await.is_err());
    assert!(restore(&config, "not-a-preview", false).await.is_err());
    assert!(!available(&config).await);
    assert!(!root.path().join("backup.bak").exists());
    protected_unchanged(root.path());
}

#[test]
fn nvram_compose_requires_one_direct_bind_and_rejects_named_or_shared_storage() {
    let (root, _plan) = fixture();
    let config = config(root.path());
    let compose = |storage: &str, extra: &str| {
        format!("services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - {storage}:/storage\n{extra}")
    };
    std::fs::write(
        &config.compose_file,
        compose(root.path().to_str().unwrap(), ""),
    )
    .unwrap();
    assert!(nvram_mount_plan(&config).is_ok());
    std::fs::write(&config.compose_file, compose("named-data", "")).unwrap();
    assert!(nvram_mount_plan(&config).is_err());
    std::fs::write(
        &config.compose_file,
        compose(
            root.path().to_str().unwrap(),
            &format!(
                "  sidecar:\n    image: busybox\n    volumes:\n      - {}:/data\n",
                root.path().display()
            ),
        ),
    )
    .unwrap();
    assert!(nvram_mount_plan(&config).is_err());
}

#[test]
fn nvram_backup_precedes_retirement_and_failure_restores_original_bytes() {
    let (root, mut plan) = fixture();
    plan.backup_and_retire().unwrap();
    assert_eq!(
        std::fs::read(root.path().join(&plan.backup)).unwrap(),
        plan.original
    );
    assert_eq!(
        std::fs::read(root.path().join(&plan.retired)).unwrap(),
        plan.original
    );
    assert!(!root.path().join(TARGET).exists());
    // Simulate the regenerated file left by a failed disposable guest boot.
    std::fs::write(root.path().join(TARGET), vec![0x42; VARS_BYTES]).unwrap();
    plan.restore().unwrap();
    assert_eq!(
        std::fs::read(root.path().join(TARGET)).unwrap(),
        plan.original
    );
    assert!(root.path().join("backup.bak.failed").is_file());
    assert!(root.path().join(&plan.backup).is_file());
    assert!(!plan.replaced);
    protected_unchanged(root.path());
}

#[test]
fn nvram_backup_or_retirement_failure_does_not_remove_the_source() {
    for collision in ["backup.bak", "original.saved"] {
        let (root, mut plan) = fixture();
        std::fs::write(root.path().join(collision), b"existing evidence").unwrap();
        assert!(plan.backup_and_retire().is_err());
        assert_eq!(
            std::fs::read(root.path().join(TARGET)).unwrap(),
            plan.original
        );
        assert_eq!(
            std::fs::read(root.path().join(collision)).unwrap(),
            b"existing evidence"
        );
        protected_unchanged(root.path());
    }
}

#[test]
fn nvram_rejects_symlinks_hardlinks_ambiguous_candidates_and_wrong_sizes() {
    let (root, mut plan) = fixture();
    std::fs::remove_file(root.path().join(TARGET)).unwrap();
    symlink("data.img", root.path().join(TARGET)).unwrap();
    assert!(plan.backup_and_retire().is_err());
    std::fs::remove_file(root.path().join(TARGET)).unwrap();
    std::fs::write(root.path().join(TARGET), &plan.original).unwrap();
    std::fs::hard_link(root.path().join(TARGET), root.path().join("linked")).unwrap();
    assert!(read_vars(&plan.directory, TARGET).is_err());
    std::fs::remove_file(root.path().join("linked")).unwrap();
    std::fs::write(root.path().join("other.vars"), &plan.original).unwrap();
    assert!(only_target(&plan.directory).is_err());
    std::fs::remove_file(root.path().join("other.vars")).unwrap();
    std::fs::write(root.path().join(TARGET), b"unexpected size").unwrap();
    assert!(read_vars(&plan.directory, TARGET).is_err());
    let parent = tempfile::tempdir().unwrap();
    symlink(root.path(), parent.path().join("alias")).unwrap();
    assert!(open_directory(&parent.path().join("alias")).is_err());
    protected_unchanged(root.path());
}

#[test]
fn nvram_changed_preview_and_tampered_backup_are_preserved_without_replacement() {
    let (root, mut plan) = fixture();
    std::fs::write(root.path().join(TARGET), vec![1; VARS_BYTES]).unwrap();
    assert!(plan.backup_and_retire().is_err());
    assert!(!root.path().join(&plan.backup).exists());
    let (root, mut plan) = fixture();
    plan.backup_and_retire().unwrap();
    std::fs::write(root.path().join(&plan.backup), vec![1; VARS_BYTES]).unwrap();
    assert!(plan.restore().is_err());
    assert_eq!(
        std::fs::read(root.path().join(&plan.retired)).unwrap(),
        plan.original
    );
    protected_unchanged(root.path());
}

#[test]
fn nvram_only_erased_stores_are_evidence_and_only_stopped_bind_containers_are_eligible() {
    assert!(erased_store(&vec![0; VARS_BYTES]));
    assert!(erased_store(&vec![0xff; VARS_BYTES]));
    assert!(!erased_store(&vec![1; VARS_BYTES]));
    assert!(!erased_store(&vec![0; VARS_BYTES - 1]));
    let (_root, plan) = fixture();
    let value = serde_json::json!({ "Id": plan.container_id, "Image": plan.image_id,
        "State": { "Status": "exited", "Running": false, "Restarting": false, "Paused": false },
        "Config": { "Image": plan.mount.image, "Labels": { "com.docker.compose.service": "windows" }, "Env": ["BOOT_MODE=windows"] },
        "Mounts": [{ "Type": "bind", "Source": plan.mount.directory, "Destination": "/storage", "RW": true }] });
    assert!(validate_container(&value, &plan.mount, true).is_ok());
    for pointer in ["/State/Running", "/State/Restarting", "/State/Paused"] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = true.into();
        assert!(validate_container(&changed, &plan.mount, true).is_err());
    }
    for env in [
        serde_json::json!(["BIOS=Y"]),
        serde_json::json!(["CLEAR=Y"]),
        serde_json::json!(["BOOT_MODE=windows", "BOOT_MODE=windows"]),
        serde_json::json!(["STORAGE=/different"]),
        serde_json::json!([null]),
    ] {
        let mut changed = value.clone();
        changed["Config"]["Env"] = env;
        assert!(validate_container(&changed, &plan.mount, true).is_err());
    }
    let mut readonly = value.clone();
    readonly["Mounts"][0]["RW"] = false.into();
    assert!(validate_container(&readonly, &plan.mount, true).is_err());
    let mut nested = value.clone();
    nested["Mounts"].as_array_mut().unwrap().push(serde_json::json!({"Type":"bind", "Source":"/unrelated", "Destination":"/storage/nested", "RW":true}));
    assert!(validate_container(&nested, &plan.mount, true).is_err());
    for status in [
        "running",
        "restarting",
        "removing",
        "paused",
        "dead",
        "unknown",
    ] {
        let mut changed = value.clone();
        changed["State"]["Status"] = status.into();
        assert!(
            validate_container(&changed, &plan.mount, true).is_err(),
            "{status}"
        );
    }
    let mut named = value.clone();
    named["Mounts"][0]["Type"] = "volume".into();
    assert!(validate_container(&named, &plan.mount, true).is_err());
    let mut custom = value;
    custom["Config"]["Env"] = serde_json::json!(["BOOT_MODE=windows_secure"]);
    assert!(validate_container(&custom, &plan.mount, true).is_err());
}
