use super::linux::{self, Kind};
use super::*;
use crate::models::ContainerRuntime;
use std::fs::{self, File};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "winboat::maintenance::tests::coordination_lock_process";

fn vm_name() -> String {
    crate::contracts::secure_identifier("vm").unwrap()
}

fn compose_text(name: &str) -> String {
    format!(
        "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.05\n    labels:\n      io.winboat.managed: 'true'\n    container_name: {name}\n    volumes:\n      - data:/storage\n"
    )
}

fn config_at(compose: &Path, name: &str) -> AppConfig {
    AppConfig {
        language_preference: "en-US".into(),
        winboat_setup_pending: false,
        winboat_executable: "fixture".into(),
        compose_file: compose.to_string_lossy().into_owned(),
        container_runtime: ContainerRuntime::Docker,
        container_name: name.into(),
        api_url: "http://127.0.0.1:9".into(),
        rdp_host: "127.0.0.1".into(),
        rdp_port: 9,
        shared_directory: "/missing".into(),
        windows_shared_directory: "fixture".into(),
        freerdp_binary: "fixture".into(),
        mendix_install_root: "fixture".into(),
        mendix_data_root: "fixture".into(),
        windows_studio_paths: vec![],
        startup_timeout_seconds: 1,
    }
}

/// A WinBoat-like Compose directory with the given permission bits.
fn winboat_directory(root: &Path, tag: &str, mode: u32, name: &str) -> AppConfig {
    let directory = root.join(tag);
    fs::create_dir_all(&directory).unwrap();
    let compose = directory.join("docker-compose.yml");
    fs::write(&compose, compose_text(name)).unwrap();
    fs::set_permissions(&compose, fs::Permissions::from_mode(0o664)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
    config_at(&compose, name)
}

fn legacy_path(config: &AppConfig, kind: Kind) -> PathBuf {
    Path::new(&config.compose_file)
        .parent()
        .unwrap()
        .join(kind.legacy_name())
}

fn private_path(config: &AppConfig, kind: Kind) -> PathBuf {
    let key = crate::winboat::vm_use::identity_key(config).unwrap();
    let suffix = match kind {
        Kind::Maintenance => "maintenance",
        Kind::RuntimeStop => "runtime-stop",
    };
    linux::root().join(format!("{key}.{suffix}.lock"))
}

fn raw_lock(path: &Path, exclusive: bool) -> File {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    if exclusive {
        fs2::FileExt::try_lock_exclusive(&file).unwrap();
    } else {
        fs2::FileExt::try_lock_shared(&file).unwrap();
    }
    file
}

#[test]
fn ordinary_operations_do_not_depend_on_compose_parent_permissions() {
    let root = tempfile::tempdir().unwrap();
    for (tag, mode, legacy_joined) in [
        ("group-writable", 0o775, false),
        ("default", 0o755, true),
        ("private", 0o700, true),
    ] {
        let config = winboat_directory(root.path(), tag, mode, &vm_name());
        let lease = shared(&config).unwrap_or_else(|error| panic!("{tag}: {error:?}"));
        assert!(lease.is_some(), "{tag}: an existing Compose file is locked");
        assert_eq!(probe(&config), Ok(()), "{tag}");
        // The private lock is the source of truth for every permission layout.
        let private = fs::metadata(private_path(&config, Kind::Maintenance)).unwrap();
        assert!(private.is_file());
        assert_eq!(private.mode() & 0o777, 0o600, "{tag}");
        assert_eq!(private.nlink(), 1);
        let root_metadata = fs::metadata(linux::root()).unwrap();
        assert_eq!(root_metadata.mode() & 0o777, 0o700);
        assert_eq!(root_metadata.uid(), unsafe { libc::geteuid() });
        // Older versions can only use a directory that passes their checks.
        assert_eq!(
            legacy_path(&config, Kind::Maintenance).exists(),
            legacy_joined,
            "{tag}: legacy participation"
        );
        assert_eq!(
            acquire(&config, true).unwrap_err(),
            LockError::Busy,
            "{tag}"
        );
        drop(lease);
        // Parallel subprocess fixtures can briefly inherit a CLOEXEC fd
        // between fork and exec. Retry the truthful Busy until it has closed.
        let mut recovered = None;
        until(|| match acquire(&config, true) {
            Ok(lease) => {
                recovered = Some(lease);
                true
            }
            Err(LockError::Busy) => false,
            Err(error) => panic!("{error:?}"),
        });
        let recovery = recovered.unwrap();
        assert_eq!(shared(&config).unwrap_err(), LockError::Busy, "{tag}");
        drop(recovery);
        // The WinBoat directory and Compose file are never modified.
        let directory = Path::new(&config.compose_file).parent().unwrap();
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, mode);
        assert_eq!(
            fs::read_to_string(&config.compose_file).unwrap(),
            compose_text(&config.container_name)
        );
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[tokio::test]
async fn runtime_stops_serialize_without_compose_directory_trust() {
    let root = tempfile::tempdir().unwrap();
    let name = vm_name();
    let first = winboat_directory(root.path(), "first", 0o775, &name);
    let second = winboat_directory(root.path(), "second", 0o775, &name);
    let owner = runtime_stop(&first, Duration::ZERO).await.unwrap();
    assert!(!legacy_path(&first, Kind::RuntimeStop).exists());
    let started = Instant::now();
    assert_eq!(
        runtime_stop(&second, Duration::from_millis(120))
            .await
            .unwrap_err(),
        LockError::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    // Maintenance and Runtime stop locks are independent.
    let maintenance = shared(&second).unwrap();
    drop(maintenance);
    let waiter = tokio::spawn({
        let second = second.clone();
        async move { runtime_stop(&second, Duration::from_secs(10)).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!waiter.is_finished(), "the stop must wait for the owner");
    drop(owner);
    let next = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(next);
    assert!(
        private_path(&first, Kind::RuntimeStop).exists(),
        "the lock inode is never unlinked"
    );
    for config in [&first, &second] {
        let directory = Path::new(&config.compose_file).parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn one_vm_identity_shares_locks_across_compose_paths_and_overrides() {
    let root = tempfile::tempdir().unwrap();
    let name = vm_name();
    let winboat = winboat_directory(root.path(), "winboat", 0o775, &name);
    let copied = winboat_directory(root.path(), "worktree/copy", 0o755, &name);
    let other = winboat_directory(root.path(), "other", 0o775, &vm_name());
    assert_eq!(
        private_path(&winboat, Kind::Maintenance),
        private_path(&copied, Kind::Maintenance)
    );
    let recovery = acquire(&winboat, true).unwrap();
    assert_eq!(shared(&copied).unwrap_err(), LockError::Busy);
    // Another VM is independent.
    let unrelated = acquire(&other, true).unwrap();
    drop(unrelated);
    drop(recovery);
    let reader = shared(&copied).unwrap();
    assert_eq!(acquire(&winboat, true).unwrap_err(), LockError::Busy);
    drop(reader);
    for config in [&winboat, &copied, &other] {
        let directory = Path::new(&config.compose_file).parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[tokio::test]
async fn locks_held_by_older_versions_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let name = vm_name();
    let trusted = winboat_directory(root.path(), "trusted", 0o755, &name);
    let group = winboat_directory(root.path(), "group", 0o775, &name);

    // An older version running UEFI recovery holds only the legacy lock.
    let old_recovery = raw_lock(&legacy_path(&trusted, Kind::Maintenance), true);
    assert_eq!(shared(&trusted).unwrap_err(), LockError::Busy);
    // The failed attempt released its private lock immediately.
    let recovery = acquire(&group, true).unwrap();
    drop(recovery);
    drop(old_recovery);

    // An older ordinary operation holds the legacy lock shared.
    let old_reader = raw_lock(&legacy_path(&trusted, Kind::Maintenance), false);
    assert!(shared(&trusted).is_ok());
    assert_eq!(acquire(&trusted, true).unwrap_err(), LockError::Busy);
    drop(old_reader);
    let recovery = acquire(&trusted, true).unwrap();
    // A current recovery also excludes older versions through the legacy lock.
    let legacy = File::open(legacy_path(&trusted, Kind::Maintenance)).unwrap();
    assert!(fs2::FileExt::try_lock_shared(&legacy).is_err());
    drop(recovery);
    assert!(fs2::FileExt::try_lock_shared(&legacy).is_ok());

    // An older Runtime stop holds the legacy stop lock.
    let old_stop = raw_lock(&legacy_path(&trusted, Kind::RuntimeStop), true);
    assert_eq!(
        runtime_stop(&trusted, Duration::from_millis(100))
            .await
            .unwrap_err(),
        LockError::Timeout
    );
    // The private stop lock was released with the failed attempt.
    drop(runtime_stop(&group, Duration::ZERO).await.unwrap());
    drop(old_stop);
    drop(runtime_stop(&trusted, Duration::ZERO).await.unwrap());
    for config in [&trusted, &group] {
        let directory = Path::new(&config.compose_file).parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn untrusted_existing_legacy_entries_fail_closed_and_are_never_followed() {
    let root = tempfile::tempdir().unwrap();
    for mode in [0o700, 0o755, 0o775] {
        let public = winboat_directory(root.path(), &format!("public-{mode:o}"), mode, &vm_name());
        let legacy = legacy_path(&public, Kind::Maintenance);
        fs::write(&legacy, b"").unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(shared(&public).unwrap_err(), LockError::Untrusted);
        assert_eq!(fs::metadata(&legacy).unwrap().mode() & 0o777, 0o640);
        fs::set_permissions(
            Path::new(&public.compose_file).parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let linked = winboat_directory(root.path(), "linked", 0o755, &vm_name());
    let target = root.path().join("protected.img");
    fs::write(&target, b"protected").unwrap();
    symlink(&target, legacy_path(&linked, Kind::Maintenance)).unwrap();
    assert_eq!(shared(&linked).unwrap_err(), LockError::Untrusted);
    assert_eq!(fs::read(&target).unwrap(), b"protected");
    let real = winboat_directory(root.path(), "real", 0o755, &vm_name());
    let alias = root.path().join("alias");
    symlink(Path::new(&real.compose_file).parent().unwrap(), &alias).unwrap();
    let aliased = config_at(&alias.join("docker-compose.yml"), &real.container_name);
    assert_eq!(shared(&aliased).unwrap_err(), LockError::Untrusted);
}

#[test]
fn private_lock_trust_failures_are_distinct_from_busy_locks() {
    let root = tempfile::tempdir().unwrap();
    let key = "a".repeat(64);
    let open = |path: &Path| linux::open_private(path, &key, Kind::Maintenance);

    let fresh = root.path().join("fresh");
    let file = open(&fresh).unwrap();
    assert_eq!(fs::metadata(&fresh).unwrap().mode() & 0o777, 0o700);
    assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
    let lock = fresh.join(format!("{key}.maintenance.lock"));

    // A held lock is busy, never untrusted.
    fs2::FileExt::try_lock_exclusive(&file).unwrap();
    let other = open(&fresh).unwrap();
    assert_eq!(linux::try_lock(&other, false), Err(LockError::Busy));
    drop(file);
    assert_eq!(linux::try_lock(&other, false), Ok(()));
    drop(other);

    // Dangerous private file permissions.
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(open(&fresh).unwrap_err(), LockError::Untrusted);
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(open(&fresh).is_ok());

    // Hardlinks, symlinks, directories and FIFOs are refused, never followed.
    let hardlink = root.path().join("hardlink");
    fs::hard_link(&lock, &hardlink).unwrap();
    assert_eq!(open(&fresh).unwrap_err(), LockError::Untrusted);
    fs::remove_file(&hardlink).unwrap();
    assert!(open(&fresh).is_ok());
    let target = root.path().join("target");
    fs::write(&target, b"target").unwrap();
    for (tag, create) in [
        (
            "symlink",
            Box::new(|path: &Path| symlink(&target, path).unwrap()) as Box<dyn Fn(&Path)>,
        ),
        (
            "directory",
            Box::new(|path: &Path| fs::create_dir(path).unwrap()),
        ),
        (
            "fifo",
            Box::new(|path: &Path| {
                let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }),
        ),
    ] {
        let directory = root.path().join(tag);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        create(&directory.join(format!("{key}.maintenance.lock")));
        assert_eq!(open(&directory).unwrap_err(), LockError::Untrusted, "{tag}");
    }
    assert_eq!(fs::read(&target).unwrap(), b"target");

    // A namespace readable or writable by others, a symlinked namespace and a
    // non-directory are refused before any lock file is created.
    let public = root.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(open(&public).unwrap_err(), LockError::Untrusted);
    assert_eq!(fs::read_dir(&public).unwrap().count(), 0);
    let alias = root.path().join("alias");
    symlink(&fresh, &alias).unwrap();
    assert_eq!(open(&alias).unwrap_err(), LockError::Untrusted);
    let regular = root.path().join("regular");
    fs::write(&regular, b"").unwrap();
    assert_eq!(open(&regular).unwrap_err(), LockError::Untrusted);
    assert!(lock.exists(), "lock files are never unlinked");
}

#[test]
fn compose_identity_mismatch_is_reported_as_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut config = winboat_directory(root.path(), "winboat", 0o775, &vm_name());
    config.container_name = vm_name();
    assert_eq!(shared(&config).unwrap_err(), LockError::Identity);
    assert_eq!(probe(&config), Err(LockError::Identity));
    let directory = Path::new(&config.compose_file).parent().unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    // An absent Compose file has nothing to recover and takes no lock.
    let missing = config_at(&root.path().join("missing/compose.yml"), &vm_name());
    assert!(shared(&missing).unwrap().is_some());
}

#[test]
fn every_cause_has_a_distinct_safe_reason_message_and_retry_policy() {
    crate::i18n::initialize("en-US").expect("localization");
    let reasons = LockError::ALL.map(LockError::reason);
    let messages = LockError::ALL.map(LockError::localized);
    for (index, error) in LockError::ALL.into_iter().enumerate() {
        assert!(!reasons[index].contains('/'), "reasons stay path-free");
        assert_eq!(LockError::from_message(reasons[index]), Some(error));
        assert_eq!(LockError::from_message(&messages[index]), Some(error));
        for other in index + 1..reasons.len() {
            assert_ne!(reasons[index], reasons[other]);
            assert_ne!(messages[index], messages[other]);
        }
        let backend = error.backend_error(crate::contracts::CapabilityId::StudioInstall);
        assert_eq!(backend.code, error.code());
        assert_eq!(
            backend.capability,
            Some(crate::contracts::CapabilityId::StudioInstall)
        );
        assert_eq!(backend.retryable, error.retryable());
        for misleading in [
            crate::tr!("error-startup-command-failed"),
            crate::tr!("error-startup-precondition-failed"),
        ] {
            assert_ne!(messages[index], misleading);
        }
    }
    assert!(LockError::Busy.retryable() && LockError::Timeout.retryable());
    assert!(!LockError::Untrusted.retryable() && !LockError::Unavailable.retryable());
    assert!(!LockError::Identity.retryable());
    assert_eq!(LockError::from_message("unrelated"), None);
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "process barrier timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn(root: &Path, config: &AppConfig, tag: &str, mode: &str) -> Process {
    Process(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
            .env("MENDIMARU_COORDINATION_FIXTURE_ROOT", root)
            .env("MENDIMARU_COORDINATION_FIXTURE_TAG", tag)
            .env("MENDIMARU_COORDINATION_FIXTURE_MODE", mode)
            .env(
                "MENDIMARU_COORDINATION_FIXTURE_COMPOSE",
                &config.compose_file,
            )
            .env(
                "MENDIMARU_COORDINATION_FIXTURE_NAME",
                &config.container_name,
            )
            .env("MENDIMARU_CONFIG_DIR", root.join(tag).join("config"))
            .env("MENDIMARU_CACHE_DIR", root.join(tag).join("cache"))
            .env("TMPDIR", root.join(tag).join("tmp"))
            .env("XDG_RUNTIME_DIR", root.join(tag).join("runtime"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

/// Child half of the cross-process tests; inert in an ordinary test run.
#[test]
fn coordination_lock_process() {
    let Ok(root) = std::env::var("MENDIMARU_COORDINATION_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let tag = std::env::var("MENDIMARU_COORDINATION_FIXTURE_TAG").unwrap();
    let mode = std::env::var("MENDIMARU_COORDINATION_FIXTURE_MODE").unwrap();
    let config = config_at(
        Path::new(&std::env::var("MENDIMARU_COORDINATION_FIXTURE_COMPOSE").unwrap()),
        &std::env::var("MENDIMARU_COORDINATION_FIXTURE_NAME").unwrap(),
    );
    let event = |suffix: &str| root.join(format!("{tag}.{suffix}"));
    tauri::async_runtime::block_on(async {
        let lease = match mode.as_str() {
            "exclusive" => acquire(&config, true),
            "shared" => acquire(&config, false),
            "stop" => runtime_stop(&config, Duration::from_secs(10)).await,
            _ => unreachable!(),
        }
        .unwrap();
        fs::write(event("held"), b"").unwrap();
        until(|| event("release").exists());
        drop(lease);
    });
    fs::write(event("released"), b"").unwrap();
}

#[tokio::test]
async fn uefi_exclusion_and_stop_serialization_hold_across_processes() {
    let root = tempfile::tempdir().unwrap();
    let name = vm_name();
    // Distinct Compose paths, permissions, config and cache directories.
    let child_config = winboat_directory(root.path(), "child-winboat", 0o775, &name);
    let parent = winboat_directory(root.path(), "parent-winboat", 0o755, &name);
    let event = |tag: &str, suffix: &str| root.path().join(format!("{tag}.{suffix}"));

    // UEFI recovery in another process excludes ordinary operations.
    let mut recovery = spawn(root.path(), &child_config, "recovery", "exclusive");
    until(|| event("recovery", "held").exists());
    assert_eq!(shared(&parent).unwrap_err(), LockError::Busy);
    assert_eq!(
        probe(&parent),
        Err(LockError::Busy),
        "diagnostics test availability without waiting"
    );
    // A crashed recovery process releases through the kernel, never by unlink.
    recovery.0.kill().unwrap();
    recovery.0.wait().unwrap();
    drop(recovery);
    drop(shared(&parent).unwrap());

    // Ordinary operations in another process exclude recovery but not readers.
    let reader = spawn(root.path(), &child_config, "reader", "shared");
    until(|| event("reader", "held").exists());
    assert_eq!(acquire(&parent, true).unwrap_err(), LockError::Busy);
    drop(shared(&parent).unwrap());
    fs::write(event("reader", "release"), b"").unwrap();
    until(|| event("reader", "released").exists());
    drop(reader);
    drop(acquire(&parent, true).unwrap());

    // Runtime stops serialize across processes and resume after release.
    let stopper = spawn(root.path(), &child_config, "stop", "stop");
    until(|| event("stop", "held").exists());
    assert_eq!(
        runtime_stop(&parent, Duration::from_millis(150))
            .await
            .unwrap_err(),
        LockError::Timeout
    );
    let waiter = tokio::spawn({
        let parent = parent.clone();
        async move { runtime_stop(&parent, Duration::from_secs(15)).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!waiter.is_finished());
    fs::write(event("stop", "release"), b"").unwrap();
    let next = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(next);
    drop(stopper);
    for config in [&child_config, &parent] {
        let directory = Path::new(&config.compose_file).parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn a_live_legacy_holder_is_not_bypassed_after_directory_permissions_change() {
    let root = tempfile::tempdir().unwrap();
    let config = winboat_directory(root.path(), "winboat", 0o755, &vm_name());
    let legacy = legacy_path(&config, Kind::Maintenance);
    let owner = raw_lock(&legacy, true);
    fs::set_permissions(
        Path::new(&config.compose_file).parent().unwrap(),
        fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    assert_eq!(shared(&config).unwrap_err(), LockError::Busy);
    assert_eq!(probe(&config), Err(LockError::Busy));
    // Changing the existing file's mode cannot turn an unknown/live inode into
    // an absent compatibility participant either.
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(shared(&config).unwrap_err(), LockError::Untrusted);
    drop(owner);
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
    drop(shared(&config).unwrap());
    assert_eq!(fs::metadata(&legacy).unwrap().nlink(), 1);
}

#[test]
fn missing_compose_still_coordinates_and_dangling_links_are_not_missing() {
    let root = tempfile::tempdir().unwrap();
    let name = vm_name();
    let missing = config_at(&root.path().join("not-yet.yml"), &name);
    let writer = acquire(&missing, true).unwrap();
    assert_eq!(shared(&missing).unwrap_err(), LockError::Busy);
    drop(writer);
    let alias = root.path().join("dangling.yml");
    symlink(root.path().join("absent.yml"), &alias).unwrap();
    assert_eq!(
        shared(&config_at(&alias, &name)).unwrap_err(),
        LockError::Untrusted
    );
    let dangling_directory = root.path().join("dangling-parent");
    symlink(root.path().join("absent-parent"), &dangling_directory).unwrap();
    assert_eq!(
        shared(&config_at(&dangling_directory.join("compose.yml"), &name)).unwrap_err(),
        LockError::Untrusted
    );
}

#[tokio::test]
async fn cancellation_while_waiting_for_the_legacy_stop_releases_the_private_lock() {
    let root = tempfile::tempdir().unwrap();
    let config = winboat_directory(root.path(), "winboat", 0o755, &vm_name());
    let old_stop = raw_lock(&legacy_path(&config, Kind::RuntimeStop), true);
    assert!(tokio::time::timeout(
        Duration::from_millis(60),
        runtime_stop(&config, Duration::from_secs(30))
    )
    .await
    .is_err());
    let private = linux::open_private(
        &linux::root(),
        &crate::winboat::vm_use::identity_key(&config).unwrap(),
        Kind::RuntimeStop,
    )
    .unwrap();
    assert_eq!(linux::try_lock(&private, true), Ok(()));
    drop(private);
    drop(old_stop);
    drop(runtime_stop(&config, Duration::ZERO).await.unwrap());
}

#[test]
fn private_modes_are_exact_not_just_no_group_permissions() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let key = "b".repeat(64);
    drop(linux::open_private(&directory, &key, Kind::Maintenance).unwrap());
    let file = directory.join(format!("{key}.maintenance.lock"));
    for mode in [0o400, 0o700, 0o1600, 0o4600] {
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            linux::open_private(&directory, &key, Kind::Maintenance).unwrap_err(),
            LockError::Untrusted
        );
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o7777, mode);
    }
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    for mode in [0o500, 0o1700, 0o2700] {
        fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            linux::open_private(&directory, &key, Kind::Maintenance).unwrap_err(),
            LockError::Untrusted
        );
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o7777, mode);
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
}
