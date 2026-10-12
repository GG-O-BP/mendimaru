use super::*;
use std::os::unix::fs::symlink;
fn fixture() -> (tempfile::TempDir, AppConfig, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    fs_set_mode(root.path(), 0o700);
    let config: AppConfig = serde_json::from_value(serde_json::json!({
        "languagePreference":"en-US", "winboatSetupPending":true,
        "winboatExecutable":"/usr/bin/true", "composeFile":root.path().join("absent.yml"),
        "containerRuntime":"docker", "containerName":crate::contracts::secure_identifier("vm").unwrap(),
        "apiUrl":"http://127.0.0.1:9", "rdpHost":"127.0.0.1", "rdpPort":9,
        "sharedDirectory":root.path(), "windowsSharedDirectory":"\\\\host.lan\\Data", "freerdpBinary":"/usr/bin/true",
        "mendixInstallRoot":"C:\\Program Files\\Mendix", "mendixDataRoot":"C:\\ProgramData\\Mendix", "windowsStudioPaths":[], "startupTimeoutSeconds":1
    })).unwrap();
    let private = root.path().join("coordination");
    std::fs::create_dir(&private).unwrap();
    fs_set_mode(&private, 0o755);
    std::fs::write(
        private.join(format!("{}.maintenance.lock", "a".repeat(64))),
        b"retain inode and bytes",
    )
    .unwrap();
    fs_set_mode(
        &private.join(format!("{}.maintenance.lock", "a".repeat(64))),
        0o640,
    );
    (root, config, private)
}
fn fs_set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
#[test]
fn confirmed_repair_changes_only_reviewed_owned_modes_not_inodes_or_data() {
    let (root, config, private) = fixture();
    let plan = plan_at(&config, &private).unwrap();
    let file = &plan.entries[0].file;
    let inode = file.metadata().unwrap().ino();
    plan.apply(&config).unwrap();
    let file = private.join(format!("{}.maintenance.lock", "a".repeat(64)));
    assert_eq!(std::fs::read(&file).unwrap(), b"retain inode and bytes");
    assert_eq!(std::fs::metadata(&file).unwrap().ino(), inode);
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(std::fs::metadata(&private).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().mode() & 0o7777,
        0o700
    );
}
#[test]
fn changed_preview_and_busy_owner_are_refused_before_any_chmod() {
    let (_root, config, private) = fixture();
    let plan = plan_at(&config, &private).unwrap();
    let file = private.join(format!("{}.maintenance.lock", "a".repeat(64)));
    fs_set_mode(&file, 0o660);
    assert!(plan.apply(&config).is_err());
    assert_eq!(std::fs::metadata(&private).unwrap().mode() & 0o7777, 0o755);
    let plan = plan_at(&config, &private).unwrap();
    let owner = File::open(&file).unwrap();
    fs2::FileExt::try_lock_exclusive(&owner).unwrap();
    assert_eq!(
        plan.apply(&config).unwrap_err().code,
        crate::models::CommandErrorCode::CoordinationBusy
    );
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o7777, 0o660);
}
#[test]
fn links_and_unknown_files_are_not_repaired() {
    let (root, config, private) = fixture();
    let target = root.path().join("target");
    std::fs::write(&target, b"protected").unwrap();
    let name = private.join(format!("{}.runtime-stop.lock", "b".repeat(64)));
    symlink(&target, name).unwrap();
    assert!(matches!(
        plan_at(&config, &private),
        Err(LockError::Untrusted)
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"protected");
    assert_eq!(std::fs::metadata(&private).unwrap().mode() & 0o7777, 0o755);
}

#[test]
fn expired_or_retargeted_preview_and_unconfirmed_request_cannot_change_modes() {
    let (_root, config, private) = fixture();
    let mut plan = plan_at(&config, &private).unwrap();
    plan.created = Instant::now() - Duration::from_secs(301);
    assert!(plan.apply(&config).is_err());
    let plan = plan_at(&config, &private).unwrap();
    let mut other = config.clone();
    other.container_name = crate::contracts::secure_identifier("vm").unwrap();
    assert!(plan.apply(&other).is_err());
    assert_eq!(
        repair(&config, "not-confirmed", false).unwrap_err().code,
        crate::models::CommandErrorCode::InvalidRequest
    );
    assert_eq!(std::fs::metadata(&private).unwrap().mode() & 0o7777, 0o755);
}
