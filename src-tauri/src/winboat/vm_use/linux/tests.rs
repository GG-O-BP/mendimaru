use super::*;
use crate::models::ContainerRuntime;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Instant;

const CHILD: &str = "winboat::vm_use::linux::tests::vm_use_process";

fn config(name: &str) -> AppConfig {
    AppConfig {
        language_preference: "en-US".into(),
        winboat_setup_pending: false,
        winboat_executable: "fixture".into(),
        compose_file: "missing-compose.yml".into(),
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

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "VM lease process barrier timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

struct Process(Child);
impl Process {
    fn finish(&mut self) {
        until(|| self.0.try_wait().unwrap().is_some());
        assert!(self.0.wait().unwrap().success());
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    root: tempfile::TempDir,
    name: String,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            name: crate::contracts::secure_identifier("vm").unwrap(),
        }
    }
    fn spawn(&self, tag: &str, mode: &str) -> Process {
        let private = self.root.path().join(tag);
        fs::create_dir_all(&private).unwrap();
        Process(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD, "--nocapture"])
                .env("MENDIMARU_VM_FIXTURE_ROOT", self.root.path())
                .env("MENDIMARU_VM_FIXTURE_NAME", &self.name)
                .env("MENDIMARU_VM_FIXTURE_TAG", tag)
                .env("MENDIMARU_VM_FIXTURE_MODE", mode)
                .env("MENDIMARU_CONFIG_DIR", private.join("config"))
                .env("MENDIMARU_CACHE_DIR", private.join("cache"))
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    fn wait(&self, tag: &str, event: &str) {
        until(|| self.root.path().join(format!("{tag}.{event}")).exists());
    }
    fn release(&self, tag: &str) {
        fs::write(self.root.path().join(format!("{tag}.release")), b"").unwrap();
    }
    fn generation(&self, tag: &str) -> Vec<u8> {
        fs::read(self.root.path().join(format!("{tag}.generation"))).unwrap()
    }
}

#[test]
fn vm_use_process() {
    let Ok(root) = std::env::var("MENDIMARU_VM_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let tag = std::env::var("MENDIMARU_VM_FIXTURE_TAG").unwrap();
    let mode = std::env::var("MENDIMARU_VM_FIXTURE_MODE").unwrap();
    let mut config = config(&std::env::var("MENDIMARU_VM_FIXTURE_NAME").unwrap());
    // Distinct configurations, caches, worktrees and Compose filenames share VM identity.
    config.compose_file = root
        .join(&tag)
        .join("compose.yml")
        .to_string_lossy()
        .into_owned();
    fs::write(&config.compose_file, format!("services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    labels:\n      io.winboat.managed: 'true'\n    container_name: {}\n    volumes:\n      - data:/storage\n", config.container_name)).unwrap();
    let event = |suffix: &str| root.join(format!("{tag}.{suffix}"));
    tauri::async_runtime::block_on(async {
        fs::write(event("started"), b"").unwrap();
        if mode == "cancel" {
            assert!(tokio::time::timeout(
                Duration::from_millis(75),
                acquire(&config, Mode::Exclusive, WAIT)
            )
            .await
            .is_err());
            return;
        }
        let requested = if mode.starts_with("reader") || mode == "pid-reuse" {
            Mode::Shared
        } else {
            Mode::Exclusive
        };
        if mode == "timeout" {
            let start = Instant::now();
            let error = acquire(&config, requested, Duration::from_millis(100))
                .await
                .err()
                .unwrap();
            assert_eq!(error, BUSY);
            assert!(start.elapsed() >= Duration::from_millis(100));
            assert!(start.elapsed() < Duration::from_secs(1));
            return;
        }
        let lease = acquire(&config, requested, WAIT).await.unwrap();
        let inode = lease.inner._file.metadata().unwrap().ino();
        assert_eq!(lease.inner.owner.pid, std::process::id());
        let mut generation = [0u8; 16];
        let size = lease.inner._file.read_at(&mut generation, 0).unwrap();
        fs::write(event("generation"), &generation[..size]).unwrap();
        if mode == "pid-reuse" {
            let spoof = Arc::new(Owned {
                _file: lease.inner._file.try_clone().unwrap(),
                key: lease.inner.key.clone(),
                mode: Mode::Shared,
                owner: ProcessIdentity {
                    pid: std::process::id(),
                    start: lease.inner.owner.start + 1,
                },
            });
            HELD.scope(vec![spoof], async {
                assert_eq!(
                    acquire(&config, Mode::Shared, Duration::ZERO)
                        .await
                        .err()
                        .unwrap(),
                    UNTRUSTED
                );
            })
            .await;
            // A stale PID/start record, or arbitrary stale bytes, never evicts
            // the actual reader. There is no stale-owner file deletion path.
            lease
                .inner
                ._file
                .write_all_at(b"stale-pid-record", 0)
                .unwrap();
        }
        lease
            .run(async {
                let nested = acquire(&config, requested, Duration::ZERO).await.unwrap();
                assert_eq!(nested.inner._file.metadata().unwrap().ino(), inode);
                if requested == Mode::Shared {
                    assert_eq!(
                        acquire(&config, Mode::Exclusive, Duration::ZERO)
                            .await
                            .err()
                            .unwrap(),
                        UPGRADE
                    );
                }
                fs::write(event("ready"), b"").unwrap();
                while !event("release").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
    });
}

#[test]
fn processes_share_vm_across_caches_and_keep_readers_parallel_writers_exclusive() {
    let f = Fixture::new();
    let mut a = f.spawn("a", "reader");
    f.wait("a", "ready");
    let mut b = f.spawn("b", "reader");
    f.wait("b", "ready");
    assert_eq!(f.generation("a"), f.generation("b"));
    let mut w = f.spawn("w", "writer");
    f.wait("w", "started");
    f.spawn("t", "timeout").finish();
    f.spawn("c", "cancel").finish();
    assert!(!f.root.path().join("w.ready").exists());
    f.release("a");
    a.finish();
    assert!(!f.root.path().join("w.ready").exists());
    f.release("b");
    b.finish();
    f.wait("w", "ready");
    let mut r = f.spawn("r", "reader");
    f.wait("r", "started");
    f.spawn("t2", "timeout").finish();
    assert!(!f.root.path().join("r.ready").exists());
    f.release("w");
    w.finish();
    f.wait("r", "ready");
    assert_eq!(f.generation("w"), f.generation("r"));
    assert_ne!(f.generation("a"), f.generation("r"));
    f.release("r");
    r.finish();
}

#[test]
fn writer_competition_and_owner_crash_preserve_one_management_identity() {
    let f = Fixture::new();
    let a = f.spawn("a", "writer");
    f.wait("a", "ready");
    let mut b = f.spawn("b", "writer");
    f.wait("b", "started");
    f.spawn("t", "timeout").finish();
    assert!(!f.root.path().join("b.ready").exists());
    drop(a);
    f.wait("b", "ready");
    assert_ne!(f.generation("a"), f.generation("b"));
    f.release("b");
    b.finish();
}

#[test]
fn stale_pid_start_identity_cannot_remove_a_live_reader_and_crash_releases_it() {
    let f = Fixture::new();
    let reader = f.spawn("r", "pid-reuse");
    f.wait("r", "ready");
    f.spawn("t", "timeout").finish();
    drop(reader);
    let mut writer = f.spawn("w", "writer");
    f.wait("w", "ready");
    f.release("w");
    writer.finish();
}

#[test]
fn untrusted_lock_entries_are_refused_and_never_removed() {
    let directory = directory().unwrap();
    for kind in ["symlink", "hardlink", "public", "directory", "fifo"] {
        let f = Fixture::new();
        let key = key(&config(&f.name)).unwrap();
        let name = format!("{key}.lock");
        let target = f.root.path().join("target");
        fs::write(&target, b"preserve").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let path = std::path::PathBuf::from(format!(
            "/tmp/mendimaru-vm-use-{}/{}",
            unsafe { libc::geteuid() },
            name
        ));
        match kind {
            "symlink" => symlink(&target, &path).unwrap(),
            "hardlink" => fs::hard_link(&target, &path).unwrap(),
            "public" => {
                fs::write(&path, b"").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            }
            "directory" => fs::create_dir(&path).unwrap(),
            "fifo" => {
                let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        assert_eq!(open(&directory, &key).err().unwrap(), UNTRUSTED, "{kind}");
        assert!(fs::symlink_metadata(&path).is_ok());
        assert_eq!(fs::read(&target).unwrap(), b"preserve");
        if kind == "directory" {
            fs::remove_dir(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn management_identity_rejects_mismatch_and_keeps_its_inode_after_release() {
    let f = Fixture::new();
    let mut config = config(&f.name);
    config.compose_file = f
        .root
        .path()
        .join("compose.yml")
        .to_string_lossy()
        .into_owned();
    fs::write(&config.compose_file, "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - data:/storage\n").unwrap();
    assert_eq!(key(&config).unwrap_err(), UNTRUSTED);
    fs::remove_file(&config.compose_file).unwrap();
    tauri::async_runtime::block_on(async {
        let a = acquire(&config, Mode::Exclusive, Duration::ZERO)
            .await
            .unwrap();
        let inode = a.inner._file.metadata().unwrap().ino();
        use std::os::fd::AsRawFd;
        assert_ne!(
            unsafe { libc::fcntl(a.inner._file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(a);
        let b = acquire(&config, Mode::Shared, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(b.inner._file.metadata().unwrap().ino(), inode);
    });
}

#[test]
fn shared_namespace_refuses_public_and_symlinked_directories() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(trusted_directory(&private).is_ok());
    let alias = root.path().join("alias");
    symlink(&private, &alias).unwrap();
    assert_eq!(trusted_directory(&alias).err().unwrap(), UNTRUSTED);
    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(trusted_directory(&private).err().unwrap(), UNTRUSTED);
    assert!(private.exists());
}

#[test]
fn waiter_revalidates_management_name_after_another_owner_changes_compose() {
    let f = Fixture::new();
    let mut config = config(&f.name);
    config.compose_file = f
        .root
        .path()
        .join("compose.yml")
        .to_string_lossy()
        .into_owned();
    let compose = format!("services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    labels:\n      io.winboat.managed: 'true'\n    container_name: {}\n    volumes:\n      - data:/storage\n", f.name);
    fs::write(&config.compose_file, &compose).unwrap();
    tauri::async_runtime::block_on(async {
        let owner = acquire(&config, Mode::Exclusive, Duration::ZERO)
            .await
            .unwrap();
        let waiting = acquire(&config, Mode::Shared, WAIT);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut waiting)
                .await
                .is_err()
        );
        fs::write(&config.compose_file, compose.replace(&f.name, "changed-vm")).unwrap();
        drop(owner);
        assert_eq!(waiting.await.err().unwrap(), UNTRUSTED);
    });
}
