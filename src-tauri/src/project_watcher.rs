use crate::models::AppConfig;
use notify::{RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const PROJECTS_CHANGED_EVENT: &str = "workspace-projects-changed";
const EVENT_DEBOUNCE: Duration = Duration::from_millis(500);

struct WatcherState {
    root: PathBuf,
    _watcher: notify::RecommendedWatcher,
}

static WATCHER: OnceLock<Mutex<Option<WatcherState>>> = OnceLock::new();
static EVENT_SENDER: OnceLock<Sender<PathBuf>> = OnceLock::new();

pub(crate) fn refresh(app: &AppHandle, config: &AppConfig) -> bool {
    let Ok(root) = std::fs::canonicalize(&config.shared_directory) else {
        return false;
    };
    let watcher_cell = WATCHER.get_or_init(|| Mutex::new(None));
    let Ok(mut current) = watcher_cell.lock() else {
        return false;
    };
    if current.as_ref().is_some_and(|state| state.root == root) {
        return true;
    }

    let event_sender = event_sender(app);
    let callback_root = root.clone();
    let mut watcher =
        match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if result.is_err() {
                mark_failed(&callback_root);
            }
            let _ = event_sender.send(callback_root.clone());
        }) {
            Ok(watcher) => watcher,
            Err(_) => return false,
        };
    if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
        return false;
    }

    *current = Some(WatcherState {
        root,
        _watcher: watcher,
    });
    true
}

fn mark_failed(root: &PathBuf) {
    let Some(watcher_cell) = WATCHER.get() else {
        return;
    };
    let Ok(mut current) = watcher_cell.lock() else {
        return;
    };
    if current.as_ref().is_some_and(|state| state.root == *root) {
        *current = None;
    }
}

fn event_sender(app: &AppHandle) -> Sender<PathBuf> {
    EVENT_SENDER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<PathBuf>();
            let event_app = app.clone();
            thread::Builder::new()
                .name("workspace-project-events".into())
                .spawn(move || debounce_events(event_app, receiver))
                .expect("the workspace event worker can start");
            sender
        })
        .clone()
}

fn debounce_events(app: AppHandle, receiver: Receiver<PathBuf>) {
    while let Ok(mut root) = receiver.recv() {
        let mut deadline = Instant::now() + EVENT_DEBOUNCE;
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(timeout) {
                Ok(next_root) => {
                    root = next_root;
                    deadline = Instant::now() + EVENT_DEBOUNCE;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        let _ = app.emit(PROJECTS_CHANGED_EVENT, root.to_string_lossy().to_string());
    }
}
