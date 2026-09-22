//! Caller-owned coordination for Studio UI jobs sharing one WinBoat helper.
//!
//! #21 provides the persistent helper, its authenticated channel and the
//! session keeper. This layer decides how multiple callers may overlap when
//! they reuse that helper for the same Studio, the same VM desktop, or both:
//!
//! * Observations (`capabilities`, `tree`, `find`, `screenshot` and semantic
//!   `wait`) only read state and take shared VM use: they are the parallel
//!   candidates across sessions and desktop scope. The guest bridge owns one
//!   request mailbox per session (#21), so requests for one Studio —
//!   observation or mutation alike — serialize in strict keeper-accept order;
//!   a semantic wait holds its turn for its whole bounded poll.
//! * Session state changes (`invoke`, `click`, `set-value`, `release`,
//!   `reconnect`) serialize per session with everything else and take
//!   exclusive VM use.
//! * Focus and keyboard input contend for the single interactive desktop:
//!   they are exclusive across every session and window of the same VM,
//!   enforced across processes with an advisory same-UID flock.
//!
//! The queue itself never opens an RDP connection, tears a VM down, retargets
//! another session or guesses a window. Caller cancellation removes only the
//! caller's own queued or running job. An RDP loss stays a bounded job
//! failure; only the keeper's monitor observation of the Studio process may
//! conclude that the session itself ended (#148, #150).
use crate::contracts::{BackendError, UiActionKind};
use crate::ui_automation::{error, Operation, Request};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::oneshot;

/// Recent jobs kept for diagnostics; waiting never depends on this history.
pub(crate) const HISTORY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Concurrency {
    /// Read-only state observation; may overlap other observations.
    Observation,
    /// State change confined to one Studio session; serial per session.
    Session,
    /// Focus or keyboard input; exclusive across the interactive desktop.
    Desktop,
}

/// How one UI request may overlap other callers of the shared helper. The
/// classification follows the #21 provider semantics: `click`, `invoke` and
/// `set-value` use window-targeted UIA patterns without stealing desktop
/// focus, while `focus` and `keyboard-input` act on the foreground.
pub(crate) fn concurrency(operation: Operation, action: Option<UiActionKind>) -> Concurrency {
    match operation {
        Operation::Capabilities
        | Operation::Tree
        | Operation::Find
        | Operation::Screenshot
        | Operation::Wait => Concurrency::Observation,
        Operation::Release | Operation::Reconnect => Concurrency::Session,
        Operation::Action => match action {
            Some(UiActionKind::Focus | UiActionKind::KeyboardInput) => Concurrency::Desktop,
            Some(_) | None => Concurrency::Session,
        },
    }
}

/// VM use stays conservative: only read-only observations share the VM with
/// other participants; every state change or foreground action excludes them.
pub(crate) fn vm_mode(class: Concurrency) -> crate::winboat::vm_use::Mode {
    match class {
        Concurrency::Observation => crate::winboat::vm_use::Mode::Shared,
        Concurrency::Session | Concurrency::Desktop => crate::winboat::vm_use::Mode::Exclusive,
    }
}

/// Same-UID caller identity captured with each job. Kernel credentials
/// authorize the socket; this identity only labels ownership and queue
/// records. It is never persisted and never used to signal or kill a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caller {
    pub(crate) pid: u32,
    pub(crate) start: Option<u64>,
}

impl Caller {
    #[cfg(target_os = "linux")]
    pub(crate) fn self_identity() -> Self {
        Self {
            pid: std::process::id(),
            start: process_start("/proc/self/stat"),
        }
    }

    /// `None` rejects peers that are not the same effective user. A missing
    /// start tick only downgrades the label to unverified; authorization
    /// never depends on it.
    #[cfg(target_os = "linux")]
    pub(crate) fn peer(stream: &tokio::net::UnixStream) -> Option<Self> {
        let credentials = stream.peer_cred().ok()?;
        if credentials.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        let pid = u32::try_from(credentials.pid()?).ok()?;
        let start = process_start(&format!("/proc/{pid}/stat"));
        Some(Self { pid, start })
    }
}

/// Linux process start ticks from `/proc`, the same identity the session ID
/// already embeds for Studio processes.
#[cfg(target_os = "linux")]
fn process_start(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
        .filter(|ticks| *ticks > 0)
}

/// One coordinated job: the caller, the exact Studio session it addresses
/// (PID and start ticks are part of the session ID) and the window it named,
/// when it named one.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) arrival: u64,
    pub(crate) session_id: String,
    pub(crate) operation: Operation,
    pub(crate) concurrency: Concurrency,
    // Ownership labels connecting the request to its caller and the exact
    // Studio PID/start/window identity. The queue never reads them to decide
    // anything; #155's verification surfacing and the test record do.
    #[allow(dead_code)]
    pub(crate) caller: Caller,
    #[allow(dead_code)]
    pub(crate) window_id: Option<String>,
}

impl Job {
    pub(crate) fn new(
        arrival: u64,
        request: &Request,
        caller: Caller,
    ) -> Result<Self, BackendError> {
        let operation = request.operation;
        let id = crate::contracts::secure_identifier("ui")
            .map_err(|_| error(operation, "ui-provider-failed"))?;
        Ok(Self {
            id,
            arrival,
            session_id: request.session_id.clone(),
            operation,
            concurrency: concurrency(operation, request.action),
            caller,
            window_id: request.window_id.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobState {
    Queued,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct JobRecord {
    pub(crate) job: Job,
    pub(crate) state: JobState,
    #[allow(dead_code)]
    pub(crate) enqueued_at: chrono::DateTime<chrono::Utc>,
}

/// The guest bridge owns one request mailbox per session (#21), so exactly
/// one in-flight guest request may exist per session regardless of class.
/// Parallel observation candidates therefore apply across sessions and at the
/// desktop scope, never inside one session's channel.
#[derive(Debug, Default)]
struct GateState {
    busy: bool,
    waiters: VecDeque<Entry>,
}

#[derive(Debug)]
struct Entry {
    arrival: u64,
    slot: Slot,
}

#[derive(Debug)]
enum Slot {
    /// A keeper-accepted arrival whose request line has not been resolved to
    /// a job yet. It conservatively blocks later same-session arrivals so a
    /// slow reader cannot be overtaken.
    Reserved,
    Filled {
        channel: oneshot::Sender<Arc<Turn>>,
    },
}

/// Per-session admission gate. Entries keep strict arrival order: a filled
/// read entry admits together with the reads that follow it, while a write
/// entry (session mutation or foreground action) admits alone.
#[derive(Debug, Default)]
struct SessionGate {
    state: Mutex<GateState>,
}

/// One admitted turn. Releasing the final reference frees the gate, including
/// when a granted-but-unpolled admission is dropped after caller death.
struct Turn {
    gate: Arc<SessionGate>,
}

impl std::fmt::Debug for Turn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Turn").finish()
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        SessionGate::turn_completed(&self.gate);
    }
}

impl SessionGate {
    /// Runs one lock scope. Granted or unwound turns are returned and dropped
    /// after unlocking; dropping them under the lock would re-enter it.
    fn locked<T>(
        gate: &Arc<Self>,
        apply: impl FnOnce(&mut GateState, &mut Vec<Arc<Turn>>) -> T,
    ) -> T {
        let mut state = gate.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut deferred = Vec::new();
        let result = apply(&mut state, &mut deferred);
        drop(state);
        drop(deferred);
        result
    }

    fn reserve(&self, arrival: u64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.waiters.push_back(Entry {
            arrival,
            slot: Slot::Reserved,
        });
    }

    fn fill(self: &Arc<Self>, arrival: u64) -> Option<oneshot::Receiver<Arc<Turn>>> {
        let (sender, receiver) = oneshot::channel();
        Self::locked(self, |state, deferred| {
            let entry = state
                .waiters
                .iter_mut()
                .find(|entry| entry.arrival == arrival)?;
            entry.slot = Slot::Filled { channel: sender };
            Self::pump(self, state, deferred);
            Some(receiver)
        })
    }

    fn remove(self: &Arc<Self>, arrival: u64) {
        Self::locked(self, |state, deferred| {
            state.waiters.retain(|entry| entry.arrival != arrival);
            Self::pump(self, state, deferred);
        });
    }

    fn turn_completed(gate: &Arc<Self>) {
        Self::locked(gate, |state, deferred| {
            state.busy = false;
            Self::pump(gate, state, deferred);
        });
    }

    fn pump(gate: &Arc<SessionGate>, state: &mut GateState, deferred: &mut Vec<Arc<Turn>>) {
        loop {
            match state.waiters.front() {
                None => break,
                Some(Entry {
                    slot: Slot::Reserved,
                    ..
                }) => break,
                Some(Entry {
                    slot: Slot::Filled { .. },
                    ..
                }) => {}
            }
            if state.busy {
                break;
            }
            let Some(Entry { slot, .. }) = state.waiters.pop_front() else {
                break;
            };
            let Slot::Filled { channel } = slot else {
                break;
            };
            state.busy = true;
            let turn = Arc::new(Turn {
                gate: Arc::clone(gate),
            });
            match channel.send(Arc::clone(&turn)) {
                // The receiver owns this grant; the local reference only
                // decrements it when dropped below.
                Ok(()) => {}
                // The caller vanished between fill and grant; both references
                // drop after unlock and release the turn through `Turn::drop`.
                Err(returned) => deferred.push(returned),
            }
            deferred.push(turn);
        }
    }
}

#[derive(Default)]
struct Registry {
    history: VecDeque<JobRecord>,
    running: usize,
}

/// Process-wide coordination state. Every UI request path in this process —
/// keeper socket callers and in-process owners alike — meets here.
#[derive(Default)]
pub(crate) struct Coordinator {
    arrivals: AtomicU64,
    sessions: Mutex<HashMap<String, Arc<SessionGate>>>,
    registry: Mutex<Registry>,
}

static COORDINATOR: OnceLock<Arc<Coordinator>> = OnceLock::new();

pub(crate) fn global() -> Arc<Coordinator> {
    Arc::clone(COORDINATOR.get_or_init(|| Arc::new(Coordinator::default())))
}

/// A keeper-accepted arrival that has not become a job yet. Dropping it
/// without enqueueing frees its queue position, so an unread or invalid
/// connection can never block later callers.
pub(crate) struct Reservation {
    coordinator: Arc<Coordinator>,
    session_id: String,
    arrival: u64,
    consumed: bool,
}

impl Reservation {
    pub(crate) fn arrival(&self) -> u64 {
        self.arrival
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.consumed {
            self.coordinator
                .remove_queued(&self.session_id, self.arrival);
        }
    }
}

pub(crate) struct Ticket {
    coordinator: Arc<Coordinator>,
    job: Job,
    receiver: oneshot::Receiver<Arc<Turn>>,
}

impl Ticket {
    #[allow(dead_code)]
    pub(crate) fn job(&self) -> &Job {
        &self.job
    }

    /// Waits for this job's turn, then for the desktop foreground scope when
    /// the job needs one. Both waits honour the request's own deadline and
    /// the caller's cancellation; only this job is removed on either.
    pub(crate) async fn admit(
        mut self,
        config: &crate::models::AppConfig,
        deadline: tokio::time::Instant,
        cancellation: Option<&crate::process::CancellationToken>,
    ) -> Result<Entered, BackendError> {
        #[cfg(not(target_os = "linux"))]
        let _ = config;
        let job = self.job.clone();
        let cancelled = async {
            match cancellation {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let turn = tokio::select! {
            biased;
            _ = cancelled => {
                self.coordinator.remove_queued(&job.session_id, job.arrival);
                self.coordinator.record_state(&job.id, JobState::Cancelled);
                return Err(error(job.operation, "ui-cancelled"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                self.coordinator.remove_queued(&job.session_id, job.arrival);
                self.coordinator
                    .record_state(&job.id, JobState::Failed("ui-coordination-busy".into()));
                return Err(error(job.operation, "ui-coordination-busy"));
            }
            turn = &mut self.receiver => {
                turn.map_err(|_| error(job.operation, "ui-session-unavailable"))?
            }
        };
        #[cfg(target_os = "linux")]
        let desktop = if job.concurrency == Concurrency::Desktop {
            match crate::winboat::vm_use::desktop_scope(config, deadline).await {
                Ok(guard) => Some(guard),
                Err(reason) => {
                    let mapped = if reason == crate::winboat::vm_use::DESKTOP_BUSY {
                        "ui-coordination-busy"
                    } else {
                        "ui-bridge-untrusted"
                    };
                    self.coordinator
                        .record_state(&job.id, JobState::Failed(mapped.into()));
                    return Err(error(job.operation, mapped));
                }
            }
        } else {
            None
        };
        self.coordinator.record_state(&job.id, JobState::Running);
        Ok(Entered {
            coordinator: Arc::clone(&self.coordinator),
            #[cfg(target_os = "linux")]
            _desktop: desktop,
            _turn: Some(turn),
            job,
            finished: false,
        })
    }
}

impl std::fmt::Debug for Entered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Entered")
            .field("job", &self.job.id)
            .field("concurrency", &self.job.concurrency)
            .finish()
    }
}

/// An admitted job. The desktop scope releases before the session turn
/// (field drop order), matching the acquisition order inside `admit`.
pub(crate) struct Entered {
    coordinator: Arc<Coordinator>,
    // Held for RAII release in declaration order: the desktop scope drops
    // before the session turn, matching the acquisition order in `admit`.
    #[cfg(target_os = "linux")]
    _desktop: Option<crate::winboat::vm_use::DesktopGuard>,
    _turn: Option<Arc<Turn>>,
    job: Job,
    finished: bool,
}

impl Entered {
    pub(crate) fn job(&self) -> &Job {
        &self.job
    }

    /// Records the job outcome. The session turn still releases only when
    /// `Entered` drops, so an unwinding caller cannot strand later jobs.
    pub(crate) fn finish(&mut self, reason: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let state = match reason {
            None => JobState::Completed,
            Some(reason) => JobState::Failed(reason.to_string()),
        };
        self.coordinator.record_state(&self.job.id, state);
    }
}

impl Drop for Entered {
    fn drop(&mut self) {
        if !self.finished {
            self.coordinator
                .record_state(&self.job.id, JobState::Failed("unwound".into()));
        }
    }
}

impl Coordinator {
    fn gate(&self, session_id: &str) -> Arc<SessionGate> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        sessions.entry(session_id.to_string()).or_default().clone()
    }

    /// Reserves the next arrival for this session in keeper-accept order.
    pub(crate) fn reserve(self: &Arc<Self>, session_id: &str) -> (u64, Reservation) {
        let arrival = self.arrivals.fetch_add(1, Ordering::SeqCst) + 1;
        self.gate(session_id).reserve(arrival);
        (
            arrival,
            Reservation {
                coordinator: Arc::clone(self),
                session_id: session_id.to_string(),
                arrival,
                consumed: false,
            },
        )
    }

    pub(crate) fn enqueue(
        self: &Arc<Self>,
        mut reservation: Reservation,
        job: Job,
    ) -> Result<Ticket, BackendError> {
        if reservation.session_id != job.session_id || reservation.arrival != job.arrival {
            return Err(error(job.operation, "ui-invalid-request"));
        }
        let receiver = self
            .gate(&job.session_id)
            .fill(reservation.arrival)
            .ok_or_else(|| error(job.operation, "ui-session-unavailable"))?;
        reservation.consumed = true;
        self.record(job.clone(), JobState::Queued);
        Ok(Ticket {
            coordinator: Arc::clone(self),
            job,
            receiver,
        })
    }

    fn remove_queued(&self, session_id: &str, arrival: u64) {
        self.gate(session_id).remove(arrival);
    }

    fn record(&self, job: Job, state: JobState) {
        let mut registry = self.registry.lock().unwrap_or_else(|p| p.into_inner());
        registry.history.push_back(JobRecord {
            job,
            state,
            enqueued_at: chrono::Utc::now(),
        });
        while registry.history.len() > HISTORY {
            registry.history.pop_front();
        }
    }

    fn record_state(&self, id: &str, state: JobState) {
        let mut registry = self.registry.lock().unwrap_or_else(|p| p.into_inner());
        let Some(index) = registry
            .history
            .iter()
            .rposition(|record| record.job.id == id)
        else {
            return;
        };
        let previous = registry.history[index].state.clone();
        if previous == JobState::Queued && state == JobState::Running {
            registry.running += 1;
        } else if previous == JobState::Running && state != JobState::Running {
            registry.running = registry.running.saturating_sub(1);
        }
        registry.history[index].state = state;
    }

    #[allow(dead_code)]
    pub(crate) fn snapshot(&self) -> Vec<JobRecord> {
        self.registry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .history
            .iter()
            .cloned()
            .collect()
    }

    /// Best-effort drain before keeper shutdown; running jobs keep their own
    /// bounded deadlines either way.
    pub(crate) async fn wait_idle(&self, deadline: tokio::time::Instant) -> bool {
        loop {
            let running = self
                .registry
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .running;
            if running == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::BackendErrorCode;

    const SESSION: &str = "studio-4242-639250850131064367";
    const OTHER_SESSION: &str = "studio-4243-639250850131064368";

    fn fixture_caller() -> Caller {
        Caller {
            pid: 4242,
            start: Some(639250850131064367),
        }
    }

    fn fixture_request(session: &str, operation: Operation) -> Request {
        let mut request = Request::new(session, operation);
        request.timeout_ms = 500;
        request
    }

    fn config() -> crate::models::AppConfig {
        fixture_config("ui-coordination-test")
    }

    fn fixture_config(name: &str) -> crate::models::AppConfig {
        let suffix = format!("{}-{name}", std::process::id());
        crate::models::AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: format!("/tmp/{suffix}-compose.yml"),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: suffix,
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: "/tmp".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn deadline(ms: u64) -> tokio::time::Instant {
        tokio::time::Instant::now() + Duration::from_millis(ms)
    }

    fn coordinator() -> Arc<Coordinator> {
        Arc::new(Coordinator::default())
    }

    fn fresh_coordinator() -> Arc<Coordinator> {
        Arc::new(Coordinator::default())
    }

    /// Enqueues a job as the keeper would: reserve at accept, fill after the
    /// request line resolves.
    fn submit(coordinator: &Arc<Coordinator>, session: &str, request: &Request) -> Ticket {
        let (arrival, reservation) = coordinator.reserve(session);
        let job = Job::new(arrival, request, fixture_caller()).expect("job");
        coordinator.enqueue(reservation, job).expect("ticket")
    }

    #[test]
    fn job_classes_follow_the_provider_action_semantics() {
        for operation in [
            Operation::Capabilities,
            Operation::Tree,
            Operation::Find,
            Operation::Screenshot,
            Operation::Wait,
        ] {
            assert_eq!(
                concurrency(operation, None),
                Concurrency::Observation,
                "{operation:?} observes state"
            );
        }
        for operation in [Operation::Release, Operation::Reconnect] {
            assert_eq!(concurrency(operation, None), Concurrency::Session);
        }
        assert_eq!(
            concurrency(Operation::Action, Some(UiActionKind::Focus)),
            Concurrency::Desktop
        );
        assert_eq!(
            concurrency(Operation::Action, Some(UiActionKind::KeyboardInput)),
            Concurrency::Desktop
        );
        for action in [
            UiActionKind::Invoke,
            UiActionKind::Click,
            UiActionKind::SetValue,
        ] {
            assert_eq!(
                concurrency(Operation::Action, Some(action)),
                Concurrency::Session
            );
        }
        assert_eq!(
            vm_mode(Concurrency::Observation),
            crate::winboat::vm_use::Mode::Shared
        );
        assert_eq!(
            vm_mode(Concurrency::Session),
            crate::winboat::vm_use::Mode::Exclusive
        );
        assert_eq!(
            vm_mode(Concurrency::Desktop),
            crate::winboat::vm_use::Mode::Exclusive
        );
    }

    #[test]
    fn coordination_busy_is_a_retryable_precondition_failure() {
        let failure = error(Operation::Action, "ui-coordination-busy");
        assert_eq!(failure.code, BackendErrorCode::PreconditionFailed);
        assert_eq!(failure.message, "ui-coordination-busy");
        assert!(failure.retryable);
        let unknown = error(Operation::Tree, "ui-coordination-busy-not-a-reason");
        assert_eq!(unknown.message, "ui-provider-failed");
        assert!(!unknown.retryable);
    }

    #[tokio::test]
    async fn same_session_jobs_run_in_accept_order_even_when_tasks_race() {
        let coordinator = coordinator();
        let config = config();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        // Reserve in accept order 1, 2, 3 but let the tasks fill out of
        // order 3, 1, 2: admission must still follow accept order.
        let mut reserves = Vec::new();
        for _ in 0..3 {
            reserves.push(coordinator.reserve(SESSION));
        }
        reserves.swap(0, 2);
        reserves.swap(1, 2);
        for (arrival, reservation) in reserves {
            let coordinator = Arc::clone(&coordinator);
            let config = config.clone();
            let order = Arc::clone(&order);
            tasks.push(tokio::spawn(async move {
                let mut request = fixture_request(SESSION, Operation::Action);
                request.element_id = Some(format!("{}:1.2", "a".repeat(32)));
                request.action = Some(UiActionKind::Invoke);
                let job = Job::new(arrival, &request, fixture_caller()).expect("job");
                let ticket = coordinator.enqueue(reservation, job).expect("ticket");
                let mut entered = ticket
                    .admit(&config, deadline(2_000), None)
                    .await
                    .expect("turn");
                order.lock().unwrap().push(arrival);
                entered.finish(None);
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }
        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
        assert!(coordinator.wait_idle(deadline(0)).await);
    }

    #[tokio::test]
    async fn same_session_requests_serialize_while_other_sessions_admit() {
        let coordinator = coordinator();
        let config = config();
        let mut first = submit(
            &coordinator,
            SESSION,
            &fixture_request(SESSION, Operation::Tree),
        )
        .admit(&config, deadline(2_000), None)
        .await
        .expect("first observation admits");
        // The guest bridge has one request mailbox per session, so even a
        // second observation waits for the holder — in arrival order.
        let second = submit(
            &coordinator,
            SESSION,
            &fixture_request(SESSION, Operation::Screenshot),
        );
        let queued_config = config.clone();
        let blocked =
            tokio::spawn(async move { second.admit(&queued_config, deadline(2_000), None).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !blocked.is_finished(),
            "one in-flight guest request per session, observation or not"
        );
        // A different session's observation is a parallel candidate: it
        // admits while the first session is still held.
        let other = submit(
            &coordinator,
            OTHER_SESSION,
            &fixture_request(OTHER_SESSION, Operation::Tree),
        )
        .admit(&config, deadline(2_000), None)
        .await
        .expect("another session admits in parallel");
        drop(other);
        first.finish(None);
        drop(first);
        blocked
            .await
            .expect("task")
            .expect("queued request runs after the holder releases");
    }

    #[tokio::test]
    async fn cancelling_a_queued_job_removes_only_that_callers_job() {
        let coordinator = coordinator();
        let config = config();
        let holder_request = {
            let mut request = fixture_request(SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "a".repeat(32)));
            request.action = Some(UiActionKind::Invoke);
            request
        };
        let held = submit(&coordinator, SESSION, &holder_request)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("holder runs");

        let cancellation = crate::process::CancellationToken::default();
        let queued_request = {
            let mut request = fixture_request(SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "b".repeat(32)));
            request.action = Some(UiActionKind::Invoke);
            request
        };
        let ticket = submit(&coordinator, SESSION, &queued_request);
        let queued_id = ticket.job().id.clone();
        let config_for_task = config.clone();
        let token = cancellation.clone();
        let queued = tokio::spawn(async move {
            ticket
                .admit(&config_for_task, deadline(2_000), Some(&token))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancellation.cancel();
        let error = queued
            .await
            .expect("task")
            .expect_err("cancelled while queued");
        assert_eq!(error.message, "ui-cancelled");

        // The unrelated caller's job still owns its turn and completes.
        let mut held = held;
        held.finish(None);
        drop(held);
        let witness_request = fixture_request(SESSION, Operation::Tree);
        let mut witness = submit(&coordinator, SESSION, &witness_request)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("queue keeps serving other callers");
        witness.finish(None);
        drop(witness);
        let states: Vec<_> = coordinator
            .snapshot()
            .into_iter()
            .filter(|record| record.job.id == queued_id)
            .collect();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].state, JobState::Cancelled);
        assert!(coordinator.wait_idle(deadline(0)).await);
    }

    #[tokio::test]
    async fn queue_wait_is_bounded_by_the_requests_own_deadline() {
        let coordinator = coordinator();
        let config = config();
        let holder_request = {
            let mut request = fixture_request(SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "a".repeat(32)));
            request.action = Some(UiActionKind::Invoke);
            request
        };
        let held = submit(&coordinator, SESSION, &holder_request)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("holder runs");
        let late_request = fixture_request(SESSION, Operation::Tree);
        let error = submit(&coordinator, SESSION, &late_request)
            .admit(&config, deadline(20), None)
            .await
            .expect_err("bounded queue wait");
        assert_eq!(error.message, "ui-coordination-busy");
        assert!(error.retryable);
        // The running job is untouched by another caller's expiry.
        let mut held = held;
        held.finish(None);
        drop(held);
        assert_eq!(
            coordinator
                .snapshot()
                .into_iter()
                .filter(|record| record.job.operation == Operation::Action)
                .count(),
            1
        );
        let follow_up = fixture_request(SESSION, Operation::Tree);
        submit(&coordinator, SESSION, &follow_up)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("queue drains after bounded failure");
    }

    #[tokio::test]
    async fn provider_failures_release_the_turn_and_keep_ownership_bounded() {
        let coordinator = coordinator();
        let config = config();
        for reason in [
            "ui-helper-exited",
            "ui-session-unavailable",
            "ui-modal-blocked",
            "ui-foreground-lost",
        ] {
            let request = fixture_request(SESSION, Operation::Wait);
            let mut entered = submit(&coordinator, SESSION, &request)
                .admit(&config, deadline(2_000), None)
                .await
                .expect("job admits");
            entered.finish(Some(reason));
            let states: Vec<_> = coordinator
                .snapshot()
                .iter()
                .map(|record| record.state.clone())
                .collect();
            assert_eq!(states.last(), Some(&JobState::Failed(reason.to_string())));
            // Releasing the failed job's turn lets the next arrival run.
            drop(entered);
            let next = fixture_request(SESSION, Operation::Tree);
            submit(&coordinator, SESSION, &next)
                .admit(&config, deadline(2_000), None)
                .await
                .expect("a bounded failure never strands the queue");
        }
        assert!(coordinator.wait_idle(deadline(0)).await);
    }

    #[tokio::test]
    async fn dropped_reservations_and_other_sessions_never_block_admission() {
        let coordinator = coordinator();
        let config = config();
        let (_, dropped) = coordinator.reserve(SESSION);
        drop(dropped);
        let request = fixture_request(SESSION, Operation::Tree);
        submit(&coordinator, SESSION, &request)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("a dropped reservation frees its position");

        // A different session admits independently; jobs never leave their
        // own session's gate.
        let mine = fixture_request(SESSION, Operation::Tree);
        let mine = submit(&coordinator, SESSION, &mine)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("first session admits");
        let other = fixture_request(OTHER_SESSION, Operation::Tree);
        let other = submit(&coordinator, OTHER_SESSION, &other)
            .admit(&config, deadline(2_000), None)
            .await
            .expect("other session admits independently");
        assert_ne!(mine.job().session_id, other.job().session_id);
    }

    #[tokio::test]
    async fn history_stays_bounded_and_running_jobs_are_counted() {
        let coordinator = coordinator();
        let config = config();
        for index in 0..(HISTORY + 8) {
            let mut request = fixture_request(SESSION, Operation::Find);
            request.selector = Some(crate::ui_automation::Selector {
                name: Some(format!("element-{index}")),
                ..Default::default()
            });
            let mut entered = submit(&coordinator, SESSION, &request)
                .admit(&config, deadline(2_000), None)
                .await
                .expect("job admits");
            entered.finish(None);
        }
        assert!(coordinator.snapshot().len() <= HISTORY);
        assert!(coordinator.wait_idle(deadline(0)).await);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn foreground_jobs_exclude_each_other_across_sessions_and_processes() {
        let coordinator = coordinator();
        let shared = config();
        let first_request = {
            let mut request = fixture_request(SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "a".repeat(32)));
            request.action = Some(UiActionKind::KeyboardInput);
            request.value = Some("F5".into());
            request
        };
        let first = submit(&coordinator, SESSION, &first_request)
            .admit(&shared, deadline(2_000), None)
            .await
            .expect("first foreground action holds the desktop");
        // A separate keeper coordinator (separate process analogue) contends
        // for the same desktop through its own flock.
        let second_coordinator = fresh_coordinator();
        let second_request = {
            let mut request = fixture_request(OTHER_SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "b".repeat(32)));
            request.action = Some(UiActionKind::Focus);
            request
        };
        let ticket = submit(&second_coordinator, OTHER_SESSION, &second_request);
        let blocked_config = shared.clone();
        let blocked =
            tokio::spawn(async move { ticket.admit(&blocked_config, deadline(60), None).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !blocked.is_finished(),
            "focus contention is excluded at desktop scope, even in another window"
        );
        let error = blocked
            .await
            .expect("task")
            .expect_err("desktop stays busy");
        assert_eq!(error.message, "ui-coordination-busy");
        drop(first);
        let third_request = {
            let mut request = fixture_request(OTHER_SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "c".repeat(32)));
            request.action = Some(UiActionKind::Focus);
            request
        };
        submit(&second_coordinator, OTHER_SESSION, &third_request)
            .admit(&shared, deadline(2_000), None)
            .await
            .expect("desktop frees after the foreground action completes");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn separate_desktops_do_not_exclude_each_other() {
        let coordinator = coordinator();
        let first_config = config();
        let second_config = fixture_config("ui-coordination-other");
        let first_request = {
            let mut request = fixture_request(SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "a".repeat(32)));
            request.action = Some(UiActionKind::Focus);
            request
        };
        let _first = submit(&coordinator, SESSION, &first_request)
            .admit(&first_config, deadline(2_000), None)
            .await
            .expect("first desktop holds its foreground action");
        let second_request = {
            let mut request = fixture_request(OTHER_SESSION, Operation::Action);
            request.element_id = Some(format!("{}:1.2", "b".repeat(32)));
            request.action = Some(UiActionKind::Focus);
            request
        };
        submit(&coordinator, OTHER_SESSION, &second_request)
            .admit(&second_config, deadline(2_000), None)
            .await
            .expect("a different VM desktop excludes independently");
    }
}
