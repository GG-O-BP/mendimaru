use crate::config;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::system_info::GetInfoParams;
#[cfg(target_os = "linux")]
use chromiumoxide::cdp::browser_protocol::system_info::GetProcessInfoParams;
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use std::future::Future;
use std::io;
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;
use tempfile::{Builder as TempDirBuilder, TempDir};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const PROFILE_PREFIX: &str = "mendimaru-marketplace-";
const PROFILE_RANDOM_BYTES: usize = 16;
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(60);
const SECURITY_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const PROBE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const HANDLER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(100);
const CLEANUP_RETRIES: usize = 50;
const LAUNCH_RETRIES: usize = 3;
const LAUNCH_RETRY_DELAY: Duration = Duration::from_millis(500);

const FORBIDDEN_BROWSER_SWITCHES: [&str; 7] = [
    "allow-running-insecure-content",
    "disable-namespace-sandbox",
    "disable-seccomp-filter-sandbox",
    "disable-setuid-sandbox",
    "disable-web-security",
    "no-sandbox",
    "single-process",
];

pub(super) struct BrowserSession {
    browser: Option<Browser>,
    handler_task: Option<JoinHandle<()>>,
    profile: Option<ProfileDirectory>,
    #[cfg(target_os = "linux")]
    sandboxed_renderer_pids: Vec<u32>,
}

impl BrowserSession {
    pub(super) async fn new() -> Result<Self, String> {
        let chrome_path =
            chrome_executable().ok_or_else(|| crate::tr!("error-browser-required"))?;
        let profile_root = std::env::temp_dir();
        retry_browser_launch(|| Self::launch_in(Path::new(&chrome_path), &profile_root)).await
    }

    async fn launch_in(chrome_path: &Path, profile_root: &Path) -> Result<Self, String> {
        let profile = ProfileDirectory::create_in(profile_root)
            .map_err(|error| crate::tr!("error-browser-profile-create", error = error))?;
        let browser_config = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(profile.path())
            .new_headless_mode()
            .args(browser_arguments())
            .build()
            .map_err(|error| crate::tr!("error-browser-config", error = error))?;
        let (browser, mut handler) = match Browser::launch(browser_config).await {
            Ok(launched) => launched,
            Err(error) => {
                #[cfg(target_os = "linux")]
                let _ = terminate_profile_processes(profile.path(), SHUTDOWN_TIMEOUT).await;
                return Err(browser_launch_error(&error.to_string()));
            }
        };
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    continue;
                }
            }
        });
        let mut session = Self {
            browser: Some(browser),
            handler_task: Some(handler_task),
            profile: Some(profile),
            #[cfg(target_os = "linux")]
            sandboxed_renderer_pids: Vec::new(),
        };
        if let Err(error) = session.verify_security().await {
            let _ = session.cleanup().await;
            return Err(error);
        }
        Ok(session)
    }

    async fn verify_security(&mut self) -> Result<(), String> {
        let browser = self.browser.as_ref().expect("launched browser is present");
        let command_line_result = timeout(
            SECURITY_CHECK_TIMEOUT,
            browser.execute(GetInfoParams::default()),
        )
        .await;
        #[cfg(target_os = "linux")]
        let command_line = command_line_result
            .map_err(|_| sandbox_failure_message(SandboxFailure::VerificationUnavailable))?
            .map_err(|_| sandbox_failure_message(SandboxFailure::VerificationUnavailable))?
            .result
            .command_line;
        #[cfg(not(target_os = "linux"))]
        let command_line = command_line_result
            .ok()
            .and_then(Result::ok)
            .map(|response| response.result.command_line)
            .unwrap_or_default();

        #[cfg(target_os = "linux")]
        if command_line.is_empty() {
            return Err(sandbox_failure_message(
                SandboxFailure::VerificationUnavailable,
            ));
        }
        if FORBIDDEN_BROWSER_SWITCHES
            .iter()
            .any(|name| command_line_has_switch(&command_line, name))
        {
            return Err(sandbox_failure_message(SandboxFailure::UnsafeSwitch));
        }

        #[cfg(target_os = "linux")]
        {
            self.sandboxed_renderer_pids = verify_linux_renderer_sandbox(browser)
                .await
                .map_err(sandbox_failure_message)?;
        }
        Ok(())
    }

    pub(super) async fn navigate(&self, url: &str) -> Result<Page, String> {
        self.navigate_with_timeout(url, NAVIGATION_TIMEOUT).await
    }

    async fn navigate_with_timeout(
        &self,
        url: &str,
        navigation_timeout: Duration,
    ) -> Result<Page, String> {
        let browser = self.browser.as_ref().expect("launched browser is present");
        let page = timeout(navigation_timeout, browser.new_page("about:blank"))
            .await
            .map_err(|_| crate::tr!("error-marketplace-connection-timeout"))?
            .map_err(|error| crate::tr!("error-browser-page", error = error))?;
        timeout(navigation_timeout, page.goto(url))
            .await
            .map_err(|_| crate::tr!("error-marketplace-connection-timeout"))?
            .map_err(|error| crate::tr!("error-marketplace-open", error = error))?;
        Ok(page)
    }

    pub(super) async fn cleanup(mut self) -> Result<(), String> {
        let browser = self.browser.take();
        let handler_task = self.handler_task.take();
        let profile = self.profile.take();
        shutdown_resources(browser, handler_task, profile).await
    }

    #[cfg(target_os = "linux")]
    async fn cleanup_for_probe(mut self) -> Result<(), String> {
        let mut browser = self.browser.take();
        let handler_task = self.handler_task.take();
        let profile = self.profile.take();

        #[cfg(target_os = "linux")]
        let profile_path = profile.as_ref().map(|profile| profile.path().to_path_buf());
        let browser_stopped = match browser.as_mut() {
            Some(browser) => {
                let _ = browser.force_kill();
                matches!(
                    timeout(PROBE_SHUTDOWN_TIMEOUT, browser.wait_for_exit()).await,
                    Ok(Ok(()))
                )
            }
            None => true,
        };
        drop(browser);

        #[cfg(target_os = "linux")]
        let descendants_stopped = match profile_path.as_deref() {
            Some(profile_path) => {
                terminate_profile_processes(profile_path, PROBE_SHUTDOWN_TIMEOUT).await
            }
            None => true,
        };
        #[cfg(not(target_os = "linux"))]
        let descendants_stopped = true;
        #[cfg(target_os = "linux")]
        let browser_stopped = browser_stopped || descendants_stopped;

        if let Some(handler_task) = handler_task {
            handler_task.abort();
            let _ = timeout(HANDLER_SHUTDOWN_TIMEOUT, handler_task).await;
        }
        let profile_removed = match profile {
            Some(profile) => profile.cleanup().await,
            None => true,
        };

        if browser_stopped && descendants_stopped && profile_removed {
            Ok(())
        } else {
            Err(crate::tr!("error-browser-cleanup"))
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    fn profile_path(&self) -> PathBuf {
        self.profile
            .as_ref()
            .expect("profile is present")
            .path()
            .to_path_buf()
    }

    #[cfg(all(test, target_os = "linux"))]
    fn sandboxed_renderer_pids(&self) -> &[u32] {
        &self.sandboxed_renderer_pids
    }
}

async fn retry_browser_launch<T, F, Fut>(mut launch: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let mut last_error = None;
    for _ in 0..LAUNCH_RETRIES {
        match launch().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(LAUNCH_RETRY_DELAY).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| crate::tr!("error-browser-required")))
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let browser = self.browser.take();
        let handler_task = self.handler_task.take();
        let profile = self.profile.take();
        if browser.is_none() && handler_task.is_none() && profile.is_none() {
            return;
        }

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = shutdown_resources(browser, handler_task, profile).await;
            });
        } else {
            if let Some(handler_task) = handler_task {
                handler_task.abort();
            }
            drop(browser);
            drop(profile);
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) async fn sandbox_available() -> bool {
    let Ok(session) = BrowserSession::new().await else {
        return false;
    };
    session.cleanup_for_probe().await.is_ok()
}

async fn shutdown_resources(
    mut browser: Option<Browser>,
    handler_task: Option<JoinHandle<()>>,
    profile: Option<ProfileDirectory>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let profile_path = profile.as_ref().map(|profile| profile.path().to_path_buf());
    let browser_stopped = match browser.as_mut() {
        Some(browser) => bounded_browser_shutdown(browser, SHUTDOWN_TIMEOUT).await,
        None => true,
    };
    drop(browser);

    #[cfg(target_os = "linux")]
    let descendants_stopped = match profile_path.as_deref() {
        Some(profile_path) => terminate_profile_processes(profile_path, SHUTDOWN_TIMEOUT).await,
        None => true,
    };
    #[cfg(not(target_os = "linux"))]
    let descendants_stopped = true;
    #[cfg(target_os = "linux")]
    let browser_stopped = browser_stopped || descendants_stopped;

    if let Some(handler_task) = handler_task {
        handler_task.abort();
        let _ = timeout(HANDLER_SHUTDOWN_TIMEOUT, handler_task).await;
    }
    let profile_removed = match profile {
        Some(profile) => profile.cleanup().await,
        None => true,
    };
    #[cfg(target_os = "linux")]
    reap_adopted_browser_zombies();
    #[cfg(target_os = "linux")]
    start_adopted_browser_zombie_monitor(SHUTDOWN_TIMEOUT);

    if browser_stopped && descendants_stopped && profile_removed {
        Ok(())
    } else {
        Err(crate::tr!("error-browser-cleanup"))
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectChildProcess {
    pid: i32,
    name: String,
    state: char,
}

#[cfg(target_os = "linux")]
fn reap_adopted_browser_zombies() {
    for zombie in direct_child_processes()
        .into_iter()
        .filter(|process| process.state == 'Z' && browser_helper_name(&process.name))
    {
        // SAFETY: this waits only for a direct browser-helper child that /proc already
        // reports as a zombie. It cannot terminate a live process. The application does
        // not directly launch these commands; Linux subreaper adoption is what makes
        // exited Chromium/WebKit helpers our children.
        let _ = unsafe { libc::waitpid(zombie.pid, std::ptr::null_mut(), libc::WNOHANG) };
    }
}

#[cfg(target_os = "linux")]
fn start_adopted_browser_zombie_monitor(shutdown_timeout: Duration) {
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    tokio::spawn(async move {
        monitor_adopted_browser_zombies(&GENERATION, generation, shutdown_timeout).await;
    });
}

#[cfg(target_os = "linux")]
async fn monitor_adopted_browser_zombies(
    current_generation: &'static AtomicU64,
    generation: u64,
    shutdown_timeout: Duration,
) {
    let deadline = Instant::now() + shutdown_timeout;
    while Instant::now() < deadline {
        if current_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        reap_adopted_browser_zombies();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    reap_adopted_browser_zombies();
}

#[cfg(target_os = "linux")]
fn direct_child_processes() -> Vec<DirectChildProcess> {
    let Ok(self_pid) = i32::try_from(std::process::id()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let (name, state, parent_pid) = proc_stat_identity(&stat)?;
            (parent_pid == self_pid).then_some(DirectChildProcess { pid, name, state })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn proc_stat_identity(stat: &str) -> Option<(String, char, i32)> {
    let opening_parenthesis = stat.find('(')?;
    let closing_parenthesis = stat.rfind(')')?;
    let name = stat
        .get(opening_parenthesis + 1..closing_parenthesis)?
        .to_owned();
    let mut fields = stat.get(closing_parenthesis + 2..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let parent_pid = fields.next()?.parse().ok()?;
    Some((name, state, parent_pid))
}

#[cfg(target_os = "linux")]
fn browser_helper_name(name: &str) -> bool {
    name == "cat" || name.starts_with("chrome") || name.starts_with("chromium")
}

type LifecycleFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;

trait BrowserLifecycle {
    fn request_close(&mut self) -> LifecycleFuture<'_>;
    fn force_kill(&mut self) -> Result<(), ()>;
    fn wait_for_exit(&mut self) -> LifecycleFuture<'_>;
}

impl BrowserLifecycle for Browser {
    fn request_close(&mut self) -> LifecycleFuture<'_> {
        Box::pin(async move { Browser::close(self).await.map(|_| ()).map_err(|_| ()) })
    }

    fn force_kill(&mut self) -> Result<(), ()> {
        match self.get_mut_child() {
            Some(child) => child.as_mut_inner().start_kill().map_err(|_| ()),
            None => Ok(()),
        }
    }

    fn wait_for_exit(&mut self) -> LifecycleFuture<'_> {
        Box::pin(async move { Browser::wait(self).await.map(|_| ()).map_err(|_| ()) })
    }
}

async fn bounded_browser_shutdown<B: BrowserLifecycle>(
    browser: &mut B,
    shutdown_timeout: Duration,
) -> bool {
    let close_succeeded = matches!(
        timeout(shutdown_timeout, browser.request_close()).await,
        Ok(Ok(()))
    );
    if !close_succeeded {
        let _ = browser.force_kill();
    }

    if matches!(
        timeout(shutdown_timeout, browser.wait_for_exit()).await,
        Ok(Ok(()))
    ) {
        return true;
    }

    let _ = browser.force_kill();
    matches!(
        timeout(shutdown_timeout, browser.wait_for_exit()).await,
        Ok(Ok(()))
    )
}

#[derive(Debug)]
struct ProfileDirectory {
    directory: Option<TempDir>,
}

impl ProfileDirectory {
    fn create_in(root: &Path) -> io::Result<Self> {
        create_profile_directory(root, PROFILE_PREFIX, PROFILE_RANDOM_BYTES)
    }

    fn path(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("profile directory is present")
            .path()
    }

    async fn cleanup(mut self) -> bool {
        let Some(directory) = self.directory.take() else {
            return true;
        };
        let path = directory.path().to_path_buf();
        drop(directory);

        for attempt in 0..CLEANUP_RETRIES {
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => return true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
                Err(_) if attempt + 1 < CLEANUP_RETRIES => {
                    tokio::time::sleep(CLEANUP_RETRY_DELAY).await;
                }
                Err(_) => return false,
            }
        }
        false
    }
}

fn create_profile_directory(
    root: &Path,
    prefix: &str,
    random_bytes: usize,
) -> io::Result<ProfileDirectory> {
    let mut builder = TempDirBuilder::new();
    builder.prefix(prefix).rand_bytes(random_bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir_in(root)?;
    validate_profile_directory(directory.path(), root)?;
    Ok(ProfileDirectory {
        directory: Some(directory),
    })
}

fn validate_profile_directory(path: &Path, root: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "browser profile must be a direct directory",
        ));
    }

    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    if canonical_path.parent() != Some(canonical_root.as_path()) {
        return Err(io::Error::other(
            "browser profile must be directly inside the temporary directory",
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // SAFETY: geteuid has no preconditions and only reads the current process identity.
        let effective_user = unsafe { libc::geteuid() };
        if metadata.uid() != effective_user {
            return Err(io::Error::other(
                "browser profile must be owned by the current user",
            ));
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(io::Error::other("browser profile permissions must be 0700"));
        }
    }
    Ok(())
}

fn browser_arguments() -> Vec<&'static str> {
    vec![
        "disable-dev-shm-usage",
        "disable-gpu",
        "disable-extensions",
        "disable-background-timer-throttling",
        "disable-default-apps",
        "disable-sync",
        "no-first-run",
        "no-default-browser-check",
    ]
}

fn command_line_has_switch(command_line: &str, expected: &str) -> bool {
    command_line.split_whitespace().any(|part| {
        let part = part.trim_matches(['\'', '"']);
        if !part.starts_with('-') {
            return false;
        }
        part.trim_start_matches('-')
            .split_once('=')
            .map_or(part.trim_start_matches('-'), |(name, _)| name)
            .eq_ignore_ascii_case(expected)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SandboxFailure {
    UnsafeSwitch,
    VerificationUnavailable,
    #[cfg(target_os = "linux")]
    RendererUnconfined,
}

fn sandbox_failure_message(failure: SandboxFailure) -> String {
    match failure {
        SandboxFailure::UnsafeSwitch => crate::tr!("error-browser-sandbox-disabled"),
        SandboxFailure::VerificationUnavailable => crate::tr!("error-browser-sandbox-unavailable"),
        #[cfg(target_os = "linux")]
        SandboxFailure::RendererUnconfined => crate::tr!("error-browser-sandbox-unavailable"),
    }
}

fn browser_launch_error(error: &str) -> String {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("no usable sandbox")
        || normalized.contains("failed to move to new namespace")
        || normalized.contains("zygote_host_impl_linux")
    {
        sandbox_failure_message(SandboxFailure::VerificationUnavailable)
    } else {
        crate::tr!("error-browser-start", error = error)
    }
}

#[cfg(target_os = "linux")]
async fn verify_linux_renderer_sandbox(browser: &Browser) -> Result<Vec<u32>, SandboxFailure> {
    let _page = timeout(SECURITY_CHECK_TIMEOUT, browser.new_page("about:blank"))
        .await
        .map_err(|_| SandboxFailure::VerificationUnavailable)?
        .map_err(|_| SandboxFailure::VerificationUnavailable)?;
    let deadline = Instant::now() + SECURITY_CHECK_TIMEOUT;

    loop {
        let process_info = timeout(
            SECURITY_CHECK_TIMEOUT,
            browser.execute(GetProcessInfoParams::default()),
        )
        .await
        .map_err(|_| SandboxFailure::VerificationUnavailable)?
        .map_err(|_| SandboxFailure::VerificationUnavailable)?
        .result
        .process_info;
        let renderer_pids = process_info
            .into_iter()
            .filter(|process| process.r#type.eq_ignore_ascii_case("renderer"))
            .filter_map(|process| u32::try_from(process.id).ok())
            .collect::<Vec<_>>();

        let mut verified = Vec::new();
        let mut saw_unconfined = false;
        for pid in renderer_pids {
            match renderer_process_is_sandboxed(pid).await {
                Ok(true) => verified.push(pid),
                Ok(false) => saw_unconfined = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(SandboxFailure::VerificationUnavailable),
            }
        }
        if saw_unconfined {
            return Err(SandboxFailure::RendererUnconfined);
        }
        if !verified.is_empty() {
            return Ok(verified);
        }
        if Instant::now() >= deadline {
            return Err(SandboxFailure::VerificationUnavailable);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(target_os = "linux")]
async fn renderer_process_is_sandboxed(pid: u32) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let process_root = PathBuf::from(format!("/proc/{pid}"));
    let metadata = tokio::fs::metadata(&process_root).await?;
    // SAFETY: geteuid has no preconditions and only reads the current process identity.
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    let status = tokio::fs::read_to_string(process_root.join("status")).await?;
    if !proc_status_has_renderer_confinement(&status) {
        return Ok(false);
    }

    let renderer_pid_namespace = tokio::fs::read_link(process_root.join("ns/pid")).await?;
    let renderer_user_namespace = tokio::fs::read_link(process_root.join("ns/user")).await?;
    let host_pid_namespace = tokio::fs::read_link("/proc/self/ns/pid").await?;
    let host_user_namespace = tokio::fs::read_link("/proc/self/ns/user").await?;
    Ok(renderer_pid_namespace != host_pid_namespace
        || renderer_user_namespace != host_user_namespace)
}

#[cfg(target_os = "linux")]
fn proc_status_has_renderer_confinement(status: &str) -> bool {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            line.strip_prefix(name)
                .and_then(|value| value.trim().parse::<u32>().ok())
        })
    };
    field("NoNewPrivs:") == Some(1) && field("Seccomp:") == Some(2)
}

#[cfg(target_os = "linux")]
async fn terminate_profile_processes(profile: &Path, shutdown_timeout: Duration) -> bool {
    let deadline = Instant::now() + shutdown_timeout;
    loop {
        let pids = profile_process_ids(profile);
        if pids.is_empty() {
            return true;
        }
        for pid in pids {
            // SAFETY: the PID was read from /proc and is additionally scoped to this session's
            // unguessable user-data-dir argument. SIGKILL does not dereference process memory.
            let result = unsafe { libc::kill(pid, libc::SIGKILL) };
            if result == -1 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return false;
            }
        }
        if Instant::now() >= deadline {
            return profile_process_ids(profile).is_empty();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(target_os = "linux")]
fn profile_process_ids(profile: &Path) -> Vec<i32> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    // SAFETY: geteuid has no preconditions and only reads the current process identity.
    let effective_user = unsafe { libc::geteuid() };
    let expected_profile = profile.as_os_str().as_bytes();
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            if pid == i32::try_from(std::process::id()).ok()? {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if metadata.uid() != effective_user {
                return None;
            }
            let command_line = std::fs::read(entry.path().join("cmdline")).ok()?;
            command_line_uses_profile(&command_line, expected_profile).then_some(pid)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn command_line_uses_profile(command_line: &[u8], expected_profile: &[u8]) -> bool {
    const PREFIX: &[u8] = b"--user-data-dir=";
    command_line.split(|byte| *byte == 0).any(|argument| {
        argument
            .strip_prefix(PREFIX)
            .is_some_and(|profile| profile == expected_profile)
    })
}

pub(super) fn chrome_executable() -> Option<String> {
    if let Ok(custom) = std::env::var("MENDIMARU_CHROME_PATH") {
        if Path::new(&custom).is_file() {
            return Some(custom);
        }
    }
    #[cfg(target_os = "windows")]
    {
        for candidate in windows_browser_candidates() {
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    config::find_binary(&[
        #[cfg(target_os = "windows")]
        "msedge",
        #[cfg(target_os = "windows")]
        "chrome",
        "google-chrome-stable",
        "google-chrome",
        "chromium",
        "chromium-browser",
    ])
}

#[cfg(target_os = "windows")]
fn windows_browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let root = PathBuf::from(program_files);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(root.join(r"Chromium\Application\chrome.exe"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        let root = PathBuf::from(program_files_x86);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local_app_data);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(root.join(r"Chromium\Application\chrome.exe"));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_arguments_follow_the_chromiumoxide_security_contract() {
        let arguments = browser_arguments();
        assert!(arguments.iter().all(|argument| !argument.starts_with('-')));
        for forbidden in FORBIDDEN_BROWSER_SWITCHES {
            assert!(
                !arguments.contains(&forbidden),
                "unsafe switch: {forbidden}"
            );
        }
        assert!(arguments.contains(&"disable-extensions"));
        assert!(arguments.contains(&"no-first-run"));
    }

    #[test]
    fn detects_forbidden_switches_in_native_and_malformed_command_lines() {
        assert!(command_line_has_switch(
            "chrome --headless=new --no-sandbox --user-data-dir=/tmp/profile",
            "no-sandbox"
        ));
        assert!(command_line_has_switch(
            "chrome ----disable-setuid-sandbox",
            "disable-setuid-sandbox"
        ));
        assert!(!command_line_has_switch(
            "chrome --headless=new --user-data-dir=/tmp/no-sandbox-profile",
            "no-sandbox"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proc_identity_after_parenthesized_names() {
        assert_eq!(
            proc_stat_identity("123 (name with ) paren) Z 42 1 2 3"),
            Some(("name with ) paren".to_owned(), 'Z', 42))
        );
        assert_eq!(proc_stat_identity("malformed"), None);
        assert_eq!(proc_stat_identity("123 (name) Q nope"), None);
        assert!(browser_helper_name("cat"));
        assert!(browser_helper_name("chrome_crashpad"));
        assert!(!browser_helper_name("powershell"));
    }

    #[test]
    fn profile_directories_are_random_private_and_direct_children() {
        let root = tempfile::tempdir().expect("temporary test root");
        let first = ProfileDirectory::create_in(root.path()).expect("first profile");
        let second = ProfileDirectory::create_in(root.path()).expect("second profile");

        assert_ne!(first.path(), second.path());
        assert_eq!(first.path().parent(), Some(root.path()));
        assert_eq!(second.path().parent(), Some(root.path()));
        assert!(first
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(PROFILE_PREFIX)));
        validate_profile_directory(first.path(), root.path()).expect("first profile is valid");
        validate_profile_directory(second.path(), root.path()).expect("second profile is valid");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(first.path())
                    .expect("profile metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn rejects_a_precreated_profile_without_changing_it() {
        let root = tempfile::tempdir().expect("temporary test root");
        let claimed = root.path().join(PROFILE_PREFIX);
        std::fs::create_dir(&claimed).expect("attacker directory fixture");
        let sentinel = claimed.join("sentinel");
        std::fs::write(&sentinel, b"unchanged").expect("sentinel fixture");

        let error = create_profile_directory(root.path(), PROFILE_PREFIX, 0)
            .expect_err("precreated directory must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel remains"),
            b"unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_profile_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary test root");
        let outside = tempfile::tempdir().expect("outside fixture");
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"unchanged").expect("sentinel fixture");
        let claimed = root.path().join(PROFILE_PREFIX);
        symlink(outside.path(), &claimed).expect("symlink fixture");

        let error = create_profile_directory(root.path(), PROFILE_PREFIX, 0)
            .expect_err("symlink must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&sentinel).expect("sentinel remains"),
            b"unchanged"
        );
        assert!(std::fs::symlink_metadata(&claimed)
            .expect("symlink remains")
            .file_type()
            .is_symlink());
    }

    #[tokio::test]
    async fn launch_failure_cleans_the_owned_profile() {
        crate::i18n::initialize("en-US").expect("localization initializes");
        let root = tempfile::tempdir().expect("temporary test root");
        let missing_browser = root.path().join("missing-browser");

        assert!(
            BrowserSession::launch_in(&missing_browser, root.path())
                .await
                .is_err(),
            "missing browser launch fails"
        );
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("test root is readable")
                .count(),
            0,
            "launch failure must not leave a profile"
        );
    }

    #[tokio::test]
    async fn transient_browser_launch_failures_are_retried() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let expected = attempts.clone();
        let value: u8 = retry_browser_launch(|| {
            let attempts = attempts.clone();
            async move {
                if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err("transient websocket timeout".to_string())
                } else {
                    Ok(42)
                }
            }
        })
        .await
        .expect("second launch succeeds");

        assert_eq!(value, 42);
        assert_eq!(
            expected.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "a transient launch failure must be retried without caller retry logic"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unusable_linux_sandbox_fails_safely_and_cleans_the_profile() {
        use std::os::unix::fs::PermissionsExt;

        crate::i18n::initialize("en-US").expect("localization initializes");
        let executable_root = tempfile::tempdir().expect("temporary executable root");
        let fake_browser = executable_root.path().join("fake-browser");
        std::fs::write(
            &fake_browser,
            "#!/bin/sh\necho 'No usable sandbox!' >&2\nexit 1\n",
        )
        .expect("write fake browser");
        std::fs::set_permissions(&fake_browser, std::fs::Permissions::from_mode(0o700))
            .expect("make fake browser executable");
        let profile_root = tempfile::tempdir().expect("temporary profile root");

        let error = BrowserSession::launch_in(&fake_browser, profile_root.path())
            .await
            .err()
            .expect("sandbox launch fails");
        assert_eq!(
            error,
            crate::tr!("error-browser-sandbox-unavailable"),
            "sandbox launch errors are structured and actionable"
        );
        assert_eq!(
            std::fs::read_dir(profile_root.path())
                .expect("profile root is readable")
                .count(),
            0,
            "sandbox launch failure must not leave a profile"
        );
    }

    #[derive(Default)]
    struct FakeLifecycle {
        events: Vec<&'static str>,
        close_never_completes: bool,
        wait_never_completes_once: bool,
    }

    impl BrowserLifecycle for FakeLifecycle {
        fn request_close(&mut self) -> LifecycleFuture<'_> {
            self.events.push("close");
            if self.close_never_completes {
                Box::pin(std::future::pending())
            } else {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        fn force_kill(&mut self) -> Result<(), ()> {
            self.events.push("kill");
            Ok(())
        }

        fn wait_for_exit(&mut self) -> LifecycleFuture<'_> {
            self.events.push("wait");
            if std::mem::take(&mut self.wait_never_completes_once) {
                Box::pin(std::future::pending())
            } else {
                Box::pin(std::future::ready(Ok(())))
            }
        }
    }

    #[tokio::test]
    async fn close_timeout_forces_kill_then_wait_before_profile_cleanup() {
        let mut lifecycle = FakeLifecycle {
            close_never_completes: true,
            ..FakeLifecycle::default()
        };
        assert!(bounded_browser_shutdown(&mut lifecycle, Duration::ZERO).await);
        lifecycle.events.push("profile-cleanup");
        assert_eq!(
            lifecycle.events,
            ["close", "kill", "wait", "profile-cleanup"]
        );
    }

    #[tokio::test]
    async fn wait_timeout_forces_kill_and_waits_again_before_cleanup() {
        let mut lifecycle = FakeLifecycle {
            wait_never_completes_once: true,
            ..FakeLifecycle::default()
        };
        assert!(bounded_browser_shutdown(&mut lifecycle, Duration::ZERO).await);
        lifecycle.events.push("profile-cleanup");
        assert_eq!(
            lifecycle.events,
            ["close", "wait", "kill", "wait", "profile-cleanup"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn requires_seccomp_filter_and_no_new_privileges() {
        let confined = "Name:\tchrome\nNoNewPrivs:\t1\nSeccomp:\t2\n";
        let no_seccomp = "NoNewPrivs:\t1\nSeccomp:\t0\n";
        let privileges = "NoNewPrivs:\t0\nSeccomp:\t2\n";
        assert!(proc_status_has_renderer_confinement(confined));
        assert!(!proc_status_has_renderer_confinement(no_seccomp));
        assert!(!proc_status_has_renderer_confinement(privileges));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profile_process_matching_requires_the_exact_user_data_directory() {
        let profile = b"/tmp/mendimaru-marketplace-random";
        assert!(command_line_uses_profile(
            b"/usr/bin/chrome\0--type=renderer\0--user-data-dir=/tmp/mendimaru-marketplace-random\0",
            profile
        ));
        assert!(!command_line_uses_profile(
            b"/usr/bin/chrome\0--type=renderer\0--user-data-dir=/tmp/other\0",
            profile
        ));
        assert!(!command_line_uses_profile(
            b"/usr/bin/unrelated\0/tmp/mendimaru-marketplace-random\0",
            profile
        ));
    }

    #[cfg(target_os = "linux")]
    fn serve_once(body: String) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("set fixture timeout");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
        });
        (format!("http://{address}/catalog"), server)
    }

    #[cfg(target_os = "linux")]
    fn serve_stalled_navigation() -> (
        String,
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::Read;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking fixture");
        let address = listener.local_addr().expect("fixture address");
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if stop_receiver.try_recv().is_ok() || Instant::now() >= deadline {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .expect("set fixture timeout");
                        let mut request = [0_u8; 4096];
                        let _ = stream.read(&mut request);
                        let _ = stop_receiver.recv_timeout(Duration::from_secs(5));
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        (
            format!("http://{address}/never-completes"),
            stop_sender,
            server,
        )
    }

    #[cfg(target_os = "linux")]
    async fn assert_processes_are_gone(pids: &[u32]) {
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            if pids
                .iter()
                .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "sandboxed renderer process leaked: {pids:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[cfg(target_os = "linux")]
    async fn assert_profile_is_removed(profile: &Path) {
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT + HANDLER_SHUTDOWN_TIMEOUT;
        loop {
            if !profile.exists() && profile_process_ids(profile).is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "browser profile or process leaked: {}",
                profile.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "launches the installed Linux Chromium sandbox against local fixtures"]
    async fn live_linux_marketplace_browser_security_gate() {
        use chromiumoxide::cdp::browser_protocol::page::CrashParams;

        crate::i18n::initialize("en-US").expect("localization initializes");
        let _guard = super::super::SCRAPE_LOCK.lock().await;
        let host_fixture = tempfile::tempdir().expect("host fixture");
        let sentinel = host_fixture.path().join("outside-browser-profile");
        std::fs::write(&sentinel, b"unchanged").expect("write host sentinel");
        let sentinel_url = format!("file://{}", sentinel.display());
        let fixture = format!(
            r##"<!doctype html>
            <div class="widget-datagrid-content">
              <div class="widget-datagrid-grid-body" role="rowgroup">
                <div class="tr" role="row">
                  <div role="gridcell"><div><div><a href="#">11.13.0</a><span>Latest</span></div></div></div>
                  <div role="gridcell"><span>August 25, 2026</span></div>
                  <div role="gridcell"><a href="https://example.invalid/release">Release Notes</a></div>
                </div>
              </div>
            </div>
            <iframe srcdoc="<script>fetch('{sentinel_url}', {{method: 'PUT', body: 'changed'}}).catch(() => {{}})</script>"></iframe>"##
        );
        let (fixture_url, fixture_server) = serve_once(fixture);

        let session = BrowserSession::new()
            .await
            .expect("sandboxed Chromium starts");
        let profile = session.profile_path();
        let renderer_pids = session.sandboxed_renderer_pids().to_vec();
        assert!(!renderer_pids.is_empty(), "renderer sandbox was verified");
        let page = session
            .navigate(&fixture_url)
            .await
            .expect("local catalog fixture opens");
        let content = page.content().await.expect("fixture HTML is readable");
        let versions = crate::marketplace::parser::parse_datagrid_html(&content)
            .expect("fixture catalog parses");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "11.13.0");

        let _ = timeout(Duration::from_secs(2), page.execute(CrashParams::default())).await;
        session.cleanup().await.expect("crashed renderer cleanup");
        fixture_server.join().expect("fixture server exits");
        assert_eq!(
            std::fs::read(&sentinel).expect("host sentinel remains"),
            b"unchanged"
        );
        assert!(!profile.exists(), "profile is removed after renderer crash");
        assert!(profile_process_ids(&profile).is_empty());
        assert_processes_are_gone(&renderer_pids).await;

        let (stalled_url, stop_server, stalled_server) = serve_stalled_navigation();
        let session = BrowserSession::new()
            .await
            .expect("second sandboxed Chromium starts");
        let profile = session.profile_path();
        let renderer_pids = session.sandboxed_renderer_pids().to_vec();
        let navigation = session
            .navigate_with_timeout(&stalled_url, Duration::from_millis(500))
            .await;
        assert!(navigation.is_err(), "stalled navigation must time out");
        let _ = stop_server.send(());
        stalled_server.join().expect("stalled fixture server exits");
        session.cleanup().await.expect("navigation timeout cleanup");
        assert!(!profile.exists(), "profile is removed after timeout");
        assert!(profile_process_ids(&profile).is_empty());
        assert_processes_are_gone(&renderer_pids).await;

        let session = BrowserSession::new()
            .await
            .expect("third sandboxed Chromium starts");
        let profile = session.profile_path();
        let renderer_pids = session.sandboxed_renderer_pids().to_vec();
        drop(session);
        assert_profile_is_removed(&profile).await;
        assert_processes_are_gone(&renderer_pids).await;
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn includes_edge_and_chrome_windows_install_locations() {
        let candidates = windows_browser_candidates();
        assert!(candidates.iter().any(|path| {
            path.to_string_lossy()
                .ends_with(r"Microsoft\Edge\Application\msedge.exe")
        }));
        assert!(candidates.iter().any(|path| {
            path.to_string_lossy()
                .ends_with(r"Google\Chrome\Application\chrome.exe")
        }));
    }
}
