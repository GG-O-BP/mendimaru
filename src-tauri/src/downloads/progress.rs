use crate::models::{DownloadProgress, DownloadState, StudioInstallPhase, StudioInstallProgress};
use tauri::{AppHandle, Emitter};

pub(super) const DOWNLOAD_EVENT: &str = "studio-download-progress";
pub(super) const PREPARING_PROGRESS: f64 = 3.0;
pub(super) const CHECKING_PROGRESS: f64 = 7.0;
pub(super) const DOWNLOAD_PROGRESS_START: f64 = 10.0;
pub(super) const DOWNLOAD_PROGRESS_END: f64 = 58.0;
pub(super) const STAGING_PROGRESS_START: f64 = 60.0;
pub(super) const STAGING_PROGRESS_END: f64 = 68.0;
pub(super) const INSTALL_PROGRESS_START: f64 = STAGING_PROGRESS_END;
pub(super) const INSTALL_PROGRESS_END: f64 = 96.0;
pub(super) const FINALIZING_PROGRESS: f64 = 97.0;
pub(super) const VERIFY_PROGRESS_START: f64 = FINALIZING_PROGRESS;
pub(super) const VERIFY_PROGRESS_END: f64 = 99.0;

pub(super) struct DownloadProgressUpdate<'a> {
    pub(super) version: &'a str,
    pub(super) state: DownloadState,
    pub(super) downloaded_bytes: u64,
    pub(super) total_bytes: Option<u64>,
    pub(super) percentage: Option<f64>,
    pub(super) estimated: bool,
    pub(super) message: String,
}

pub(super) fn emit_progress(app: &AppHandle, update: DownloadProgressUpdate<'_>) {
    let DownloadProgressUpdate {
        version,
        state,
        downloaded_bytes,
        total_bytes,
        percentage,
        estimated,
        message,
    } = update;
    let _ = app.emit(
        DOWNLOAD_EVENT,
        DownloadProgress {
            version: version.to_string(),
            state,
            downloaded_bytes,
            total_bytes,
            percentage,
            estimated,
            message,
        },
    );
}

pub(super) fn emit_install_progress(
    app: &AppHandle,
    version: &str,
    progress: StudioInstallProgress,
) {
    let message = match progress.phase {
        StudioInstallPhase::Staging => crate::tr!("progress-staging"),
        StudioInstallPhase::Installing => crate::tr!("progress-installing"),
        StudioInstallPhase::Finalizing => crate::tr!("progress-finalizing"),
        StudioInstallPhase::Verifying => crate::tr!("progress-verifying"),
    };
    emit_progress(
        app,
        DownloadProgressUpdate {
            version,
            state: progress.phase.download_state(),
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: overall_install_percentage(&progress),
            estimated: progress.estimated,
            message,
        },
    );
}

pub(super) fn overall_download_percentage(downloaded: u64, total: Option<u64>) -> Option<f64> {
    total.filter(|value| *value > 0).map(|value| {
        let downloaded_ratio = (downloaded as f64 / value as f64).clamp(0.0, 1.0);
        DOWNLOAD_PROGRESS_START
            + downloaded_ratio * (DOWNLOAD_PROGRESS_END - DOWNLOAD_PROGRESS_START)
    })
}

fn overall_install_percentage(progress: &StudioInstallProgress) -> Option<f64> {
    let phase = progress.percentage?.clamp(0.0, 100.0) / 100.0;
    Some(match progress.phase {
        StudioInstallPhase::Staging => {
            STAGING_PROGRESS_START + phase * (STAGING_PROGRESS_END - STAGING_PROGRESS_START)
        }
        StudioInstallPhase::Installing => {
            INSTALL_PROGRESS_START + phase * (INSTALL_PROGRESS_END - INSTALL_PROGRESS_START)
        }
        StudioInstallPhase::Finalizing => FINALIZING_PROGRESS,
        StudioInstallPhase::Verifying => {
            VERIFY_PROGRESS_START + phase * (VERIFY_PROGRESS_END - VERIFY_PROGRESS_START)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        overall_download_percentage, overall_install_percentage, DOWNLOAD_PROGRESS_END,
        DOWNLOAD_PROGRESS_START, FINALIZING_PROGRESS, INSTALL_PROGRESS_END, STAGING_PROGRESS_END,
        STAGING_PROGRESS_START, VERIFY_PROGRESS_END,
    };
    use crate::models::{StudioInstallPhase, StudioInstallProgress};

    #[test]
    fn download_percentage_is_mapped_to_the_overall_install_range() {
        assert_eq!(
            overall_download_percentage(0, Some(100)),
            Some(DOWNLOAD_PROGRESS_START)
        );
        assert_eq!(overall_download_percentage(50, Some(100)), Some(34.0));
        assert_eq!(
            overall_download_percentage(100, Some(100)),
            Some(DOWNLOAD_PROGRESS_END)
        );
    }

    #[test]
    fn download_percentage_handles_missing_or_invalid_totals() {
        assert_eq!(overall_download_percentage(10, None), None);
        assert_eq!(overall_download_percentage(10, Some(0)), None);
        assert_eq!(
            overall_download_percentage(120, Some(100)),
            Some(DOWNLOAD_PROGRESS_END)
        );
    }

    #[test]
    fn windows_phases_fill_the_reserved_install_ranges_without_reaching_completion() {
        let progress = |phase, percentage| StudioInstallProgress {
            phase,
            percentage: Some(percentage),
            estimated: false,
        };

        assert_eq!(
            overall_install_percentage(&progress(StudioInstallPhase::Staging, 0.0)),
            Some(STAGING_PROGRESS_START)
        );
        assert_eq!(
            overall_install_percentage(&progress(StudioInstallPhase::Staging, 100.0)),
            Some(STAGING_PROGRESS_END)
        );
        assert_eq!(
            overall_install_percentage(&progress(StudioInstallPhase::Installing, 100.0)),
            Some(INSTALL_PROGRESS_END)
        );
        assert_eq!(
            overall_install_percentage(&progress(StudioInstallPhase::Finalizing, 100.0)),
            Some(FINALIZING_PROGRESS)
        );
        assert_eq!(
            overall_install_percentage(&progress(StudioInstallPhase::Verifying, 100.0)),
            Some(VERIFY_PROGRESS_END)
        );
        const { assert!(VERIFY_PROGRESS_END < 100.0) };
    }
}
