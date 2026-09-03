use super::DownloadCancellation;
use crate::models::{InstallQueueItem, InstallQueueState};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

const MAX_QUEUE_ITEMS: usize = 32;
const MAX_QUEUE_STORE_BYTES: u64 = 256 * 1024;

pub const INSTALL_QUEUE_EVENT: &str = "install-queue-changed";

#[derive(Default)]
pub struct InstallQueue {
    inner: Arc<QueueInner>,
}

impl InstallQueue {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn without_worker() -> Self {
        let queue = Self::default();
        queue.inner.worker_enabled.store(false, Ordering::SeqCst);
        queue
    }

    pub fn set_app(&self, app: tauri::AppHandle) {
        if let Ok(mut state) = self.lock_state() {
            state.app = Some(app);
        }
    }

    pub fn restore(&self, store_path: std::path::PathBuf) -> Result<(), String> {
        let mut state = self.lock_state()?;
        state.store_path = Some(store_path.clone());
        let items = load_store(&store_path)?;
        for mut item in items {
            if !item.state.is_terminal() {
                item.state = InstallQueueState::Queued;
                item.percentage = None;
                item.downloaded_bytes = None;
                item.total_bytes = None;
                item.updated_at = Utc::now();
            }
            state.items.push(item);
        }
        persist(&store_path, &state.items)?;
        let snapshot = state.items.clone();
        drop(state);
        self.publish(snapshot);
        self.ensure_worker();
        Ok(())
    }

    pub fn enqueue(
        &self,
        version: String,
        force_redownload: bool,
        is_retry: bool,
        retry_of: Option<String>,
    ) -> Result<InstallQueueItem, String> {
        crate::platform::validate_version(&version)?;
        let mut state = self.lock_state()?;
        if state.items.len() >= MAX_QUEUE_ITEMS {
            return Err("the install queue is full".to_string());
        }
        if state
            .items
            .iter()
            .any(|item| !item.state.is_terminal() && item.version == version)
        {
            return Err(format!("{version} is already queued or running"));
        }
        let now = Utc::now();
        let item = InstallQueueItem {
            id: crate::contracts::secure_identifier("install-queue")
                .map_err(|error| error.message)?,
            version,
            force_redownload,
            retry_of,
            state: InstallQueueState::Queued,
            downloaded_bytes: None,
            total_bytes: None,
            percentage: None,
            message: None,
            created_at: now,
            updated_at: now,
        };
        state.items.push(item.clone());
        if is_retry {
            // Retries re-enter through the front so the user's retry is next.
            let index = state.items.len() - 1;
            let mut insert_at = 0;
            while insert_at < index && state.items[insert_at].state != InstallQueueState::Queued {
                insert_at += 1;
            }
            let item = state.items.remove(index);
            state.items.insert(insert_at, item);
        }
        let store_path = state.store_path.clone();
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            persist(path, &snapshot)?;
        }
        drop(state);
        self.publish(snapshot);
        self.ensure_worker();
        Ok(item)
    }

    pub fn list(&self) -> Vec<InstallQueueItem> {
        self.lock_state()
            .map(|state| state.items.clone())
            .unwrap_or_default()
    }

    pub fn subscribe(&self) -> watch::Receiver<Vec<InstallQueueItem>> {
        self.inner.watch.subscribe()
    }

    pub async fn wait_for_terminal(&self, item_id: &str) -> InstallQueueItem {
        let mut receiver = self.subscribe();
        loop {
            let terminal = receiver
                .borrow_and_update()
                .iter()
                .find(|item| item.id == item_id)
                .filter(|item| item.state.is_terminal())
                .cloned();
            if let Some(item) = terminal {
                return item;
            }
            if receiver.changed().await.is_err() {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if let Some(item) = self
                    .list()
                    .into_iter()
                    .find(|item| item.id == item_id && item.state.is_terminal())
                {
                    return item;
                }
            }
        }
    }

    pub fn cancel(&self, item_id: &str, keep_partial: bool) -> Result<bool, String> {
        let mut state = self.lock_state()?;
        let store_path = state.store_path.clone();
        if let Some(current) = &state.current {
            if current.item_id == item_id {
                let cancelled = current.cancellation.cancel(!keep_partial);
                drop(state);
                if cancelled {
                    self.publish(self.list());
                }
                return Ok(cancelled);
            }
        }
        let Some(index) = state
            .items
            .iter()
            .position(|item| item.id == item_id && item.state == InstallQueueState::Queued)
        else {
            return Ok(false);
        };
        state.items[index].state = InstallQueueState::Cancelled;
        state.items[index].message = Some("the queued install was cancelled".to_string());
        state.items[index].updated_at = Utc::now();
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            persist(path, &snapshot)?;
        }
        drop(state);
        self.publish(snapshot);
        Ok(true)
    }

    pub fn cancel_current(&self, keep_partial: bool) -> bool {
        let current = self.lock_state().ok().and_then(|state| {
            state
                .current
                .as_ref()
                .map(|current| current.cancellation.clone())
        });
        match current {
            Some(cancellation) => cancellation.cancel(!keep_partial),
            None => false,
        }
    }

    pub fn retry(&self, item_id: &str) -> Result<InstallQueueItem, String> {
        let mut state = self.lock_state()?;
        let store_path = state.store_path.clone();
        let Some(index) = state.items.iter().position(|item| item.id == item_id) else {
            return Err("the install queue item was not found".to_string());
        };
        if !matches!(
            state.items[index].state,
            InstallQueueState::Failed | InstallQueueState::Cancelled
        ) {
            return Err("only failed or cancelled installs can be retried".to_string());
        }
        state.items[index].state = InstallQueueState::Queued;
        state.items[index].message = None;
        state.items[index].downloaded_bytes = None;
        state.items[index].total_bytes = None;
        state.items[index].percentage = None;
        state.items[index].updated_at = Utc::now();
        let item = state.items[index].clone();
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            persist(path, &snapshot)?;
        }
        drop(state);
        self.publish(snapshot);
        self.ensure_worker();
        Ok(item)
    }

    pub fn remove(&self, item_id: &str) -> Result<(), String> {
        let mut state = self.lock_state()?;
        let store_path = state.store_path.clone();
        let Some(index) = state.items.iter().position(|item| item.id == item_id) else {
            return Err("the install queue item was not found".to_string());
        };
        if !state.items[index].state.is_terminal() {
            return Err("only finished installs can be removed".to_string());
        }
        state.items.remove(index);
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            persist(path, &snapshot)?;
        }
        drop(state);
        self.publish(snapshot);
        Ok(())
    }

    pub fn move_item(&self, item_id: &str, up: bool) -> Result<(), String> {
        let mut state = self.lock_state()?;
        let store_path = state.store_path.clone();
        let queued_positions = state
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.state == InstallQueueState::Queued)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(position) = queued_positions
            .iter()
            .position(|index| state.items[*index].id == item_id)
        else {
            return Err("only pending installs can be reordered".to_string());
        };
        let target = if up {
            position
                .checked_sub(1)
                .ok_or_else(|| "the install is already next".to_string())?
        } else {
            position + 1
        };
        if target >= queued_positions.len() {
            return Err("the install is already last".to_string());
        }
        let source_index = queued_positions[position];
        let target_index = queued_positions[target];
        state.items.swap(source_index, target_index);
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            persist(path, &snapshot)?;
        }
        drop(state);
        self.publish(snapshot);
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, QueueState>, String> {
        self.inner
            .state
            .lock()
            .map_err(|_| "the install queue state was poisoned".to_string())
    }

    fn publish(&self, items: Vec<InstallQueueItem>) {
        let _ = self.inner.watch.send(items);
    }

    fn ensure_worker(&self) {
        if !self.inner.worker_enabled.load(Ordering::SeqCst) {
            return;
        }
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        tauri::async_runtime::spawn(async move {
            worker_loop(inner).await;
        });
    }
}

fn begin_next(inner: &QueueInner) -> Option<(InstallQueueItem, Arc<DownloadCancellation>)> {
    {
        let app = inner.state.lock().ok().and_then(|state| state.app.clone());
        let mut state = inner.state.lock().ok()?;
        let index = state
            .items
            .iter()
            .position(|item| item.state == InstallQueueState::Queued)?;
        state.items[index].state = InstallQueueState::Downloading;
        state.items[index].updated_at = Utc::now();
        let item = state.items[index].clone();
        let cancellation = Arc::new(DownloadCancellation::new());
        state.current = Some(CurrentInstall {
            item_id: item.id.clone(),
            cancellation: Arc::clone(&cancellation),
        });
        let store_path = state.store_path.clone();
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            let _ = persist(path, &snapshot);
        }
        drop(state);
        let _ = inner.watch.send(snapshot.clone());
        emit_snapshot(app.as_ref(), &snapshot);
        Some((item, cancellation))
    }
}

fn finish_current(
    inner: &QueueInner,
    item_id: &str,
    state_result: InstallQueueState,
    message: Option<String>,
    operation_id: Option<String>,
) {
    {
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        state.current = None;
        if let Some(item) = state.items.iter_mut().find(|item| item.id == item_id) {
            item.state = state_result;
            item.message = match (&state_result, operation_id) {
                (InstallQueueState::Succeeded, Some(operation_id)) => Some(operation_id),
                (InstallQueueState::Succeeded, None) => {
                    Some("the Studio Pro installation completed".to_string())
                }
                _ => message,
            };
            item.updated_at = Utc::now();
            if state_result.is_terminal() {
                item.percentage = None;
            }
        }
        let store_path = state.store_path.clone();
        let snapshot = state.items.clone();
        if let Some(path) = &store_path {
            let _ = persist(path, &snapshot);
        }
        drop(state);
        let _ = inner.watch.send(snapshot.clone());
        let app = state_app(inner);
        emit_snapshot(app.as_ref(), &snapshot);
    }
}

fn update_progress(inner: &QueueInner, item_id: &str, progress: &crate::models::DownloadProgress) {
    {
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        let Some(item) = state.items.iter_mut().find(|item| item.id == item_id) else {
            return;
        };
        item.state = match progress.state {
            crate::models::DownloadState::Staging => InstallQueueState::Staging,
            crate::models::DownloadState::Installing
            | crate::models::DownloadState::Finalizing
            | crate::models::DownloadState::Verifying => InstallQueueState::Installing,
            crate::models::DownloadState::Installed => InstallQueueState::Succeeded,
            _ => InstallQueueState::Downloading,
        };
        item.downloaded_bytes = Some(progress.downloaded_bytes);
        item.total_bytes = progress.total_bytes;
        item.percentage = progress.percentage;
        item.message = Some(progress.message.clone());
        item.updated_at = Utc::now();
        let snapshot = state.items.clone();
        drop(state);
        let _ = inner.watch.send(snapshot.clone());
        let app = state_app(inner);
        emit_snapshot(app.as_ref(), &snapshot);
        if let Some(app) = app {
            let _ = tauri::Emitter::emit(
                &app,
                super::DOWNLOAD_EVENT,
                crate::models::DownloadProgress {
                    version: progress_version(inner, item_id),
                    state: progress.state,
                    downloaded_bytes: progress.downloaded_bytes,
                    total_bytes: progress.total_bytes,
                    percentage: progress.percentage,
                    estimated: progress.estimated,
                    message: progress.message.clone(),
                },
            );
        }
    }
}

fn state_app(inner: &QueueInner) -> Option<tauri::AppHandle> {
    inner.state.lock().ok().and_then(|state| state.app.clone())
}

fn progress_version(inner: &QueueInner, item_id: &str) -> String {
    inner
        .state
        .lock()
        .ok()
        .and_then(|state| {
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .map(|item| item.version.clone())
        })
        .unwrap_or_default()
}

fn emit_snapshot(app: Option<&tauri::AppHandle>, items: &[InstallQueueItem]) {
    if let Some(app) = app {
        let _ = tauri::Emitter::emit(app, INSTALL_QUEUE_EVENT, items.to_vec());
    }
}

struct CurrentInstall {
    item_id: String,
    cancellation: Arc<DownloadCancellation>,
}

struct QueueState {
    items: Vec<InstallQueueItem>,
    store_path: Option<std::path::PathBuf>,
    current: Option<CurrentInstall>,
    app: Option<tauri::AppHandle>,
}

struct QueueInner {
    state: Mutex<QueueState>,
    watch: watch::Sender<Vec<InstallQueueItem>>,
    running: AtomicBool,
    worker_enabled: AtomicBool,
}

impl Default for QueueInner {
    fn default() -> Self {
        let (watch, _) = watch::channel(Vec::new());
        Self {
            state: Mutex::new(QueueState {
                items: Vec::new(),
                store_path: None,
                current: None,
                app: None,
            }),
            watch,
            running: AtomicBool::new(false),
            worker_enabled: AtomicBool::new(true),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct InstallQueueStore {
    schema_version: u32,
    items: Vec<InstallQueueItem>,
}

fn load_store(path: &std::path::Path) -> Result<Vec<InstallQueueItem>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not inspect the install queue: {error}")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("the install queue store must be a direct regular file".to_string());
    }
    if metadata.len() > MAX_QUEUE_STORE_BYTES {
        return Err("the install queue store exceeds its safe size limit".to_string());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read the install queue: {error}"))?;
    let store = serde_json::from_slice::<InstallQueueStore>(&bytes)
        .map_err(|error| format!("could not parse the install queue: {error}"))?;
    if store.schema_version != 1 {
        return Err("the install queue schema is unsupported".to_string());
    }
    if store.items.len() > MAX_QUEUE_ITEMS {
        return Err("the install queue exceeds its item limit".to_string());
    }
    Ok(store.items)
}

fn persist(path: &std::path::Path, items: &[InstallQueueItem]) -> Result<(), String> {
    let store = InstallQueueStore {
        schema_version: 1,
        items: items.to_vec(),
    };
    let serialized = serde_json::to_vec_pretty(&store)
        .map_err(|error| format!("could not serialize the install queue: {error}"))?;
    if serialized.len() as u64 > MAX_QUEUE_STORE_BYTES {
        return Err("the install queue store exceeds its safe size limit".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the install queue has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create the install queue directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, &serialized)
        .map_err(|error| format!("could not write the install queue: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("could not replace the install queue: {error}"))
}

async fn worker_loop(inner: Arc<QueueInner>) {
    loop {
        let Some((item, cancellation)) = begin_next(&inner) else {
            inner.running.store(false, Ordering::SeqCst);
            // Re-check after marking idle so an enqueue race cannot strand
            // work; otherwise a future enqueue spawns the next worker.
            let has_pending = inner
                .state
                .lock()
                .map(|state| {
                    state
                        .items
                        .iter()
                        .any(|item| item.state == InstallQueueState::Queued)
                })
                .unwrap_or(false);
            if has_pending && !inner.running.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        };

        let item_id = item.id.clone();
        let version = item.version.clone();
        let force_redownload = item.force_redownload;
        let retry_of = item.retry_of.clone();
        let result = process_item(
            &inner,
            &item_id,
            &version,
            force_redownload,
            retry_of,
            &cancellation,
        )
        .await;
        let (state, message, operation_id) = match result {
            Ok(operation_id) => (InstallQueueState::Succeeded, None, Some(operation_id)),
            Err(QueueInstallError::Cancelled(message)) => {
                (InstallQueueState::Cancelled, Some(message), None)
            }
            Err(QueueInstallError::Failed(message)) => {
                (InstallQueueState::Failed, Some(message), None)
            }
        };
        finish_current(&inner, &item_id, state, message, operation_id);
    }
}

enum QueueInstallError {
    Cancelled(String),
    Failed(String),
}

async fn process_item(
    inner: &Arc<QueueInner>,
    item_id: &str,
    version: &str,
    force_redownload: bool,
    retry_of: Option<String>,
    cancellation: &Arc<DownloadCancellation>,
) -> Result<String, QueueInstallError> {
    let app = inner
        .state
        .lock()
        .ok()
        .and_then(|state| state.app.clone())
        .ok_or_else(|| {
            QueueInstallError::Failed("the install queue has no application handle".to_string())
        })?;
    let paths = crate::app_paths::AppPaths::from_app(&app).map_err(QueueInstallError::Failed)?;
    let config = crate::application::load_config(&paths)
        .map_err(|error| QueueInstallError::Failed(error.message))?;
    let progress_inner = Arc::clone(inner);
    let progress_item_id = item_id.to_string();
    let result = crate::application::execute_install(
        &paths,
        &config,
        version.to_string(),
        force_redownload,
        retry_of,
        cancellation,
        move |progress| {
            update_progress(&progress_inner, &progress_item_id, progress);
        },
    )
    .await;
    match result {
        Ok(operation_id) => Ok(operation_id),
        Err(error) => {
            if error.code == crate::models::CommandErrorCode::DownloadCancelled
                || error.code == crate::models::CommandErrorCode::ExternalProcessCancelled
            {
                Err(QueueInstallError::Cancelled(error.message))
            } else {
                Err(QueueInstallError::Failed(error.message))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_store, persist, InstallQueue, InstallQueueState, MAX_QUEUE_ITEMS};
    use crate::models::InstallQueueItem;
    use chrono::Utc;

    fn store_path(temporary: &std::path::Path) -> std::path::PathBuf {
        temporary.join("install-queue.json")
    }

    fn queue(temporary: &std::path::Path) -> InstallQueue {
        let queue = InstallQueue::without_worker();
        queue
            .restore(store_path(temporary))
            .expect("restore empty queue");
        queue
    }

    #[test]
    fn queue_persists_and_restores_pending_installs() {
        let temporary = tempfile::tempdir().expect("temporary queue directory");
        let first = queue(temporary.path());
        let alpha = first
            .enqueue("11.12.2".to_string(), false, false, None)
            .expect("enqueue alpha");
        first
            .enqueue("11.13.0".to_string(), false, false, None)
            .expect("enqueue beta");

        let restored = load_store(&store_path(temporary.path())).expect("persisted queue");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].version, "11.12.2");

        let second = queue(temporary.path());
        let items = second.list();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, alpha.id);
        assert!(items
            .iter()
            .all(|item| item.state == InstallQueueState::Queued));
    }

    #[test]
    fn restore_converts_interrupted_items_to_queued_and_keeps_terminal_items() {
        let temporary = tempfile::tempdir().expect("temporary queue directory");
        let now = Utc::now();
        let interrupted = InstallQueueItem {
            id: format!("install-queue-{}", "a".repeat(32)),
            version: "11.12.2".to_string(),
            force_redownload: false,
            retry_of: None,
            state: InstallQueueState::Downloading,
            downloaded_bytes: Some(128),
            total_bytes: Some(256),
            percentage: Some(12.0),
            message: None,
            created_at: now,
            updated_at: now,
        };
        let terminal = InstallQueueItem {
            id: format!("install-queue-{}", "b".repeat(32)),
            version: "11.13.0".to_string(),
            force_redownload: false,
            retry_of: None,
            state: InstallQueueState::Succeeded,
            downloaded_bytes: None,
            total_bytes: None,
            percentage: None,
            message: Some("operation-1".to_string()),
            created_at: now,
            updated_at: now,
        };
        persist(
            &store_path(temporary.path()),
            &[interrupted.clone(), terminal.clone()],
        )
        .expect("persist interrupted queue");
        drop(interrupted);

        let restored = queue(temporary.path());
        let items = restored.list();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].state, InstallQueueState::Queued);
        assert_eq!(items[0].downloaded_bytes, None);
        assert_eq!(items[1].state, InstallQueueState::Succeeded);
    }

    #[test]
    fn queue_rejects_duplicates_and_enforces_the_item_limit() {
        let temporary = tempfile::tempdir().expect("temporary queue directory");
        let queue = queue(temporary.path());
        queue
            .enqueue("11.12.2".to_string(), false, false, None)
            .expect("first enqueue");
        assert!(queue
            .enqueue("11.12.2".to_string(), false, false, None)
            .is_err());
        for index in 1..MAX_QUEUE_ITEMS {
            queue
                .enqueue(format!("11.13.{index}"), false, false, None)
                .expect("fill queue");
        }
        assert!(queue
            .enqueue("11.99.0".to_string(), false, false, None)
            .is_err());
    }

    #[test]
    fn queue_supports_cancel_retry_remove_and_reordering() {
        let temporary = tempfile::tempdir().expect("temporary queue directory");
        let queue = queue(temporary.path());
        let alpha = queue
            .enqueue("11.12.2".to_string(), false, false, None)
            .expect("alpha");
        let beta = queue
            .enqueue("11.13.0".to_string(), false, false, None)
            .expect("beta");
        let gamma = queue
            .enqueue("11.14.0".to_string(), false, false, None)
            .expect("gamma");

        queue.move_item(&beta.id, true).expect("move beta up");
        assert_eq!(
            queue
                .list()
                .iter()
                .map(|item| item.version.clone())
                .collect::<Vec<_>>(),
            ["11.13.0", "11.12.2", "11.14.0"]
        );
        assert!(queue.move_item(&beta.id, true).is_err());

        queue.cancel(&gamma.id, true).expect("cancel gamma");
        assert_eq!(queue.list()[2].state, InstallQueueState::Cancelled);
        assert!(
            queue.remove(&alpha.id).is_err(),
            "active items cannot be removed"
        );

        queue.retry(&gamma.id).expect("retry gamma");
        assert_eq!(queue.list()[2].state, InstallQueueState::Queued);
        assert!(queue.move_item(&gamma.id, false).is_err());
        queue.move_item(&gamma.id, true).expect("move gamma up");
        queue
            .move_item(&gamma.id, false)
            .expect("move gamma back down");
        assert!(queue.move_item(&gamma.id, false).is_err());

        queue.cancel(&gamma.id, false).expect("cancel gamma again");
        queue.remove(&gamma.id).expect("remove terminal gamma");
        assert_eq!(queue.list().len(), 2);
        assert!(queue.remove(&alpha.id).is_err());
        assert!(beta.id != alpha.id);
    }

    #[test]
    fn queue_store_rejects_unsafe_sizes_and_schemas() {
        let temporary = tempfile::tempdir().expect("temporary queue directory");
        std::fs::write(
            store_path(temporary.path()),
            br#"{"schemaVersion":2,"items":[]}"#,
        )
        .expect("unsupported schema");
        assert!(InstallQueue::without_worker()
            .restore(store_path(temporary.path()))
            .is_err());
    }
}
