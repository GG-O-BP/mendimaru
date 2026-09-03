use std::fmt;
use std::future::pending;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Notify;

const DEFAULT_TERMINATION_GRACE: Duration = Duration::from_millis(250);
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandPolicy {
    pub(crate) timeout: Duration,
    pub(crate) output_limit: usize,
    pub(crate) termination_grace: Duration,
}

impl CommandPolicy {
    pub(crate) const PROBE: Self = Self::new(Duration::from_secs(2), 64 * 1024);
    pub(crate) const STATUS: Self = Self::new(Duration::from_secs(5), 256 * 1024);

    pub(crate) const fn new(timeout: Duration, output_limit: usize) -> Self {
        Self {
            timeout,
            output_limit,
            termination_grace: DEFAULT_TERMINATION_GRACE,
        }
    }

    #[cfg(all(test, unix))]
    const fn with_termination_grace(mut self, termination_grace: Duration) -> Self {
        self.termination_grace = termination_grace;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    notification: Arc<Notify>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notification.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            let notification = self.notification.notified();
            if self.is_cancelled() {
                return;
            }
            notification.await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandFailureKind {
    Spawn,
    Wait,
    Timeout,
    Cancelled,
    Cleanup,
}

#[derive(Debug)]
pub(crate) struct CommandFailure {
    kind: CommandFailureKind,
    operation: String,
    source: Option<io::Error>,
}

impl CommandFailure {
    pub(crate) fn new(
        kind: CommandFailureKind,
        operation: &str,
        source: Option<io::Error>,
    ) -> Self {
        Self {
            kind,
            operation: operation.to_string(),
            source,
        }
    }

    pub(crate) const fn kind(&self) -> CommandFailureKind {
        self.kind
    }
}

impl fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            CommandFailureKind::Spawn => "could not be started",
            CommandFailureKind::Wait => "could not be observed",
            CommandFailureKind::Timeout => "reached its deadline",
            CommandFailureKind::Cancelled => "was cancelled",
            CommandFailureKind::Cleanup => "could not be fully cleaned up",
        };
        write!(formatter, "{} {reason}", self.operation)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CommandFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug)]
pub(crate) struct CommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

enum Completion {
    Exited(io::Result<ExitStatus>),
    TimedOut,
    Cancelled,
}

pub(crate) async fn output(
    mut command: Command,
    policy: CommandPolicy,
    cancellation: Option<&CancellationToken>,
    operation: &str,
) -> Result<CommandOutput, CommandFailure> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .spawn()
        .map_err(|error| CommandFailure::new(CommandFailureKind::Spawn, operation, Some(error)))?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(CommandFailure::new(
                CommandFailureKind::Cleanup,
                operation,
                Some(error),
            ));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_and_reap(&tree, &mut child, policy.termination_grace).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Wait,
                operation,
                None,
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_and_reap(&tree, &mut child, policy.termination_grace).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Wait,
                operation,
                None,
            ));
        }
    };
    let stdout_task = tokio::spawn(read_bounded(stdout, policy.output_limit));
    let stderr_task = tokio::spawn(read_bounded(stderr, policy.output_limit));
    let timeout = tokio::time::sleep(policy.timeout);
    tokio::pin!(timeout);
    let cancelled = async {
        match cancellation {
            Some(cancellation) => cancellation.cancelled().await,
            None => pending::<()>().await,
        }
    };
    tokio::pin!(cancelled);

    let completion = tokio::select! {
        result = child.wait() => Completion::Exited(result),
        () = &mut timeout => Completion::TimedOut,
        () = &mut cancelled => Completion::Cancelled,
    };

    let status = match completion {
        Completion::Exited(Ok(status)) => status,
        Completion::Exited(Err(error)) => {
            terminate_and_reap(&tree, &mut child, policy.termination_grace).await;
            drain_after_termination(stdout_task, stderr_task).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Wait,
                operation,
                Some(error),
            ));
        }
        Completion::TimedOut => {
            terminate_and_reap(&tree, &mut child, policy.termination_grace).await;
            drain_after_termination(stdout_task, stderr_task).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Timeout,
                operation,
                None,
            ));
        }
        Completion::Cancelled => {
            terminate_and_reap(&tree, &mut child, policy.termination_grace).await;
            drain_after_termination(stdout_task, stderr_task).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Cancelled,
                operation,
                None,
            ));
        }
    };

    let (stdout, stderr) = match tokio::time::timeout(PIPE_DRAIN_TIMEOUT, async {
        tokio::try_join!(join_capture(stdout_task), join_capture(stderr_task))
    })
    .await
    {
        Ok(Ok(captured)) => captured,
        Ok(Err(error)) => {
            tree.terminate();
            tree.reap_descendants(policy.termination_grace).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Wait,
                operation,
                Some(error),
            ));
        }
        Err(_) => {
            tree.terminate();
            tree.reap_descendants(policy.termination_grace).await;
            return Err(CommandFailure::new(
                CommandFailureKind::Cleanup,
                operation,
                None,
            ));
        }
    };

    Ok(CommandOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

pub(crate) fn output_sync(
    command: std::process::Command,
    policy: CommandPolicy,
    cancellation: Option<CancellationToken>,
    operation: &str,
) -> Result<CommandOutput, CommandFailure> {
    let operation = operation.to_string();
    let thread_operation = operation.clone();
    std::thread::Builder::new()
        .name("bounded-command".to_string())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    CommandFailure::new(CommandFailureKind::Spawn, &thread_operation, Some(error))
                })?
                .block_on(output(
                    Command::from(command),
                    policy,
                    cancellation.as_ref(),
                    &thread_operation,
                ))
        })
        .map_err(|error| CommandFailure::new(CommandFailureKind::Spawn, &operation, Some(error)))?
        .join()
        .map_err(|_| CommandFailure::new(CommandFailureKind::Wait, &operation, None))?
}

async fn read_bounded<R>(mut stream: R, limit: usize) -> io::Result<CapturedStream>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(limit.min(8 * 1024));
    let mut chunk = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        let kept = remaining.min(read);
        bytes.extend_from_slice(&chunk[..kept]);
        truncated |= kept < read;
    }
    Ok(CapturedStream { bytes, truncated })
}

async fn join_capture(
    task: tokio::task::JoinHandle<io::Result<CapturedStream>>,
) -> io::Result<CapturedStream> {
    task.await
        .map_err(|error| io::Error::other(format!("output reader failed: {error}")))?
}

async fn drain_after_termination(
    stdout: tokio::task::JoinHandle<io::Result<CapturedStream>>,
    stderr: tokio::task::JoinHandle<io::Result<CapturedStream>>,
) {
    let _ = tokio::time::timeout(PIPE_DRAIN_TIMEOUT, async {
        let _ = tokio::join!(stdout, stderr);
    })
    .await;
}

async fn terminate_and_reap(
    tree: &ProcessTree,
    child: &mut tokio::process::Child,
    grace: Duration,
) {
    tree.terminate_gracefully();
    let _ = tokio::time::timeout(grace, child.wait()).await;
    tree.terminate();
    let _ = tokio::time::timeout(grace.max(Duration::from_millis(100)), child.wait()).await;
    tree.reap_descendants(grace).await;
}

#[cfg(unix)]
#[derive(Debug)]
struct ProcessTree {
    process_group: i32,
}

#[cfg(unix)]
impl ProcessTree {
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        let process_group = child
            .id()
            .filter(|id| *id <= i32::MAX as u32)
            .ok_or_else(|| io::Error::other("the child process has no valid process group"))?
            as i32;
        enable_subreaper()?;
        Ok(Self { process_group })
    }

    fn terminate_gracefully(&self) {
        signal_process_group(self.process_group, libc::SIGTERM);
    }

    fn terminate(&self) {
        signal_process_group(self.process_group, libc::SIGKILL);
    }

    async fn reap_descendants(&self, grace: Duration) {
        #[cfg(target_os = "linux")]
        {
            let deadline = tokio::time::Instant::now() + grace.max(Duration::from_millis(100));
            loop {
                let waited = unsafe {
                    libc::waitpid(-self.process_group, std::ptr::null_mut(), libc::WNOHANG)
                };
                if waited > 0 {
                    continue;
                }
                if waited == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        break;
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = grace;
    }
}

#[cfg(unix)]
fn signal_process_group(process_group: i32, signal: i32) {
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == -1 {
        let _ = io::Error::last_os_error();
    }
}

#[cfg(target_os = "linux")]
fn enable_subreaper() -> io::Result<()> {
    use std::sync::OnceLock;
    static RESULT: OnceLock<Result<(), i32>> = OnceLock::new();
    match RESULT.get_or_init(|| {
        let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error().raw_os_error().unwrap_or(-1))
        }
    }) {
        Ok(()) => Ok(()),
        Err(code) => Err(io::Error::from_raw_os_error(*code)),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn enable_subreaper() -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
#[derive(Debug)]
struct ProcessTree {
    job: WindowsJob,
}

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("the child process has no handle"))?
            as windows_sys::Win32::Foundation::HANDLE;
        Ok(Self {
            job: WindowsJob::attach(process)?,
        })
    }

    fn terminate_gracefully(&self) {}

    fn terminate(&self) {
        self.job.terminate();
    }

    async fn reap_descendants(&self, grace: Duration) {
        let deadline = tokio::time::Instant::now() + grace.max(Duration::from_millis(100));
        loop {
            match self.job.active_processes() {
                Ok(0) | Err(_) => return,
                Ok(_) if tokio::time::Instant::now() >= deadline => return,
                Ok(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct WindowsJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// Windows kernel handles are process-wide values. Job Object operations are
// safe to invoke from any thread while this owner keeps the handle open.
#[cfg(windows)]
unsafe impl Send for WindowsJob {}

#[cfg(windows)]
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    pub(crate) fn attach(process: windows_sys::Win32::Foundation::HANDLE) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        let assigned = configured != 0 && unsafe { AssignProcessToJobObject(job, process) } != 0;
        if !assigned {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle: job })
    }

    pub(crate) fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }

    pub(crate) fn active_processes(&self) -> io::Result<u32> {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut accounting).cast(),
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(accounting.ActiveProcesses)
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
#[derive(Debug)]
struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn attach(_child: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self)
    }

    fn terminate_gracefully(&self) {}

    fn terminate(&self) {}

    async fn reap_descendants(&self, _grace: Duration) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::{output, CancellationToken, CommandFailureKind, CommandPolicy, PIPE_DRAIN_TIMEOUT};
    use std::time::Duration;
    use tokio::process::Command;

    #[cfg(unix)]
    #[tokio::test]
    async fn deadline_kills_a_non_terminating_command_and_allows_the_next_command() {
        let policy = CommandPolicy::new(Duration::from_millis(100), 1024)
            .with_termination_grace(Duration::from_millis(50));
        let mut hanging = Command::new("sh");
        hanging.args(["-c", "trap '' TERM; while :; do sleep 1; done"]);

        let failure = output(hanging, policy, None, "hanging fixture")
            .await
            .expect_err("fixture times out");
        assert_eq!(failure.kind(), CommandFailureKind::Timeout);

        let mut recovery = Command::new("sh");
        recovery.args(["-c", "printf recovered"]);
        let recovered = output(recovery, policy, None, "recovery fixture")
            .await
            .expect("next command succeeds");
        assert_eq!(recovered.stdout, b"recovered");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_and_reaps_a_term_ignoring_descendant() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let pid_file = temporary.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("trap '' TERM; sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo $! > \"$PID_FILE\"; while :; do sleep 1; done")
            .env("PID_FILE", &pid_file);
        let policy = CommandPolicy::new(Duration::from_millis(150), 1024)
            .with_termination_grace(Duration::from_millis(75));

        let failure = output(command, policy, None, "descendant fixture")
            .await
            .expect_err("fixture times out");
        assert_eq!(failure.kind(), CommandFailureKind::Timeout);
        let pid = std::fs::read_to_string(pid_file)
            .expect("descendant pid was written")
            .trim()
            .parse::<i32>()
            .expect("descendant pid parses");
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "descendant {pid} survived timeout cleanup");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_is_drained_without_exceeding_the_capture_limit() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 4096 ]; do printf 0123456789abcdef; printf fedcba9876543210 >&2; i=$((i+1)); done",
        ]);
        let output = output(
            command,
            CommandPolicy::new(Duration::from_secs(2), 4096),
            None,
            "output flood fixture",
        )
        .await
        .expect("flood fixture completes");

        assert_eq!(output.stdout.len(), 4096);
        assert_eq!(output.stderr.len(), 4096);
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_the_process_tree() {
        let cancellation = CancellationToken::default();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            trigger.cancel();
        });
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do sleep 1; done"]);

        let failure = output(
            command,
            CommandPolicy::new(Duration::from_secs(5), 1024),
            Some(&cancellation),
            "cancel fixture",
        )
        .await
        .expect_err("fixture is cancelled");
        assert_eq!(failure.kind(), CommandFailureKind::Cancelled);
        assert!(PIPE_DRAIN_TIMEOUT < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hanging_process_does_not_starve_short_async_work() {
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do sleep 1; done"]);
        let hanging = tokio::spawn(output(
            command,
            CommandPolicy::new(Duration::from_millis(250), 1024),
            None,
            "starvation fixture",
        ));

        let short = tokio::time::timeout(Duration::from_millis(100), async {
            tokio::task::yield_now().await;
            42
        })
        .await
        .expect("short async work is not starved");
        assert_eq!(short, 42);
        assert_eq!(
            hanging
                .await
                .expect("fixture task joins")
                .expect_err("fixture times out")
                .kind(),
            CommandFailureKind::Timeout
        );
    }
}
