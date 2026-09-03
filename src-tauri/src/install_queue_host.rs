use crate::app_paths::AppPaths;
use crate::downloads::{InstallQueueHost, InstallQueueWorker, DOWNLOAD_EVENT, INSTALL_QUEUE_EVENT};
use crate::models::{AppConfig, DownloadProgress, InstallQueueItem};

pub(crate) struct TauriInstallQueueHost {
    app: tauri::AppHandle,
}

impl TauriInstallQueueHost {
    pub(crate) fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl InstallQueueHost for TauriInstallQueueHost {
    fn emit_download_progress(&self, progress: DownloadProgress) {
        let _ = tauri::Emitter::emit(&self.app, DOWNLOAD_EVENT, progress);
    }

    fn emit_queue_changed(&self, items: Vec<InstallQueueItem>) {
        let _ = tauri::Emitter::emit(&self.app, INSTALL_QUEUE_EVENT, items);
    }

    fn install_context(&self) -> Result<(AppPaths, AppConfig), String> {
        let paths = AppPaths::from_app(&self.app)?;
        let config = crate::application::load_config(&paths).map_err(|error| error.message)?;
        Ok((paths, config))
    }

    fn spawn_worker(&self, worker: InstallQueueWorker) {
        drop(tauri::async_runtime::spawn(async move {
            worker.await;
        }));
    }
}
