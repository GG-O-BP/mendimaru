import { useEffect, useRef, useState } from "react";
import type {
  DownloadProgress,
  DownloadState,
  LocalizationBundle,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  localizedNumber,
  useLocalizedBytes,
} from "../../shared/hooks/useLocalizedValues";

const TERMINAL_PROGRESS_STATES = new Set<DownloadState>([
  "installed",
  "failed",
  "cancelled",
]);
const INSTALLATION_STAGE_KEYS = [
  "progress-stage-prepare",
  "progress-stage-download",
  "progress-stage-staging",
  "progress-stage-install",
  "progress-stage-verify",
] as const;
const LOCALIZED_PROGRESS_STATES = new Set<DownloadState>([
  "starting",
  "preparing",
  "checking",
  "connecting",
  "downloading",
  "downloaded",
  "ready",
  "staging",
  "installing",
  "finalizing",
  "verifying",
  "installed",
  "cancelled",
]);

function installationStageIndex(state: DownloadState, percentage: number) {
  if (["starting", "preparing", "checking"].includes(state)) return 0;
  if (["connecting", "downloading", "downloaded", "ready"].includes(state)) {
    return 1;
  }
  if (state === "staging") return 2;
  if (["installing", "finalizing"].includes(state)) return 3;
  if (["verifying", "installed"].includes(state)) return 4;
  if (percentage < 10) return 0;
  if (percentage < 60) return 1;
  if (percentage < 68) return 2;
  if (percentage < 97) return 3;
  return 4;
}

function clampPercentage(value: number) {
  return Math.min(100, Math.max(0, value));
}

export function InstallationProgress({
  t,
  localization,
  progress,
  isInstalling,
  onCancel,
}: {
  t: Translate;
  localization: LocalizationBundle;
  progress: DownloadProgress;
  isInstalling: boolean;
  onCancel: () => void;
}) {
  const [displayedPercentage, setDisplayedPercentage] = useState(() =>
    clampPercentage(progress.percentage ?? 2),
  );
  const previousState = useRef(progress.state);
  const reportedPercentage = clampPercentage(progress.percentage ?? 0);
  const isActive = !TERMINAL_PROGRESS_STATES.has(progress.state);
  const [downloadedLabel, totalLabel] = useLocalizedBytes(
    [progress.downloadedBytes, progress.totalBytes],
    localization,
  );

  useEffect(() => {
    const restarted =
      progress.state === "starting" && previousState.current !== "starting";
    setDisplayedPercentage((current) => {
      if (restarted) return Math.max(2, reportedPercentage);
      return progress.state === "installed"
        ? 100
        : Math.max(current, reportedPercentage);
    });
    previousState.current = progress.state;
  }, [progress.state, reportedPercentage]);

  const boundedPercentage =
    progress.state === "installed" ? 100 : Math.min(99, displayedPercentage);
  const roundedPercentage = Math.round(boundedPercentage);
  const currentStage = installationStageIndex(
    progress.state,
    boundedPercentage,
  );
  const description = progressDescription(
    progress,
    t,
    downloadedLabel,
    totalLabel,
  );
  const progressLabel =
    progress.state === "installed"
      ? t("progress-complete", {
          percentage: localizedNumber(localization, 100),
        })
      : progress.state === "failed"
        ? t("progress-failed-short")
        : progress.state === "cancelled"
          ? t("progress-cancelled-short")
          : t(
              progress.estimated ? "progress-approximate" : "progress-percent",
              {
                percentage: localizedNumber(localization, roundedPercentage),
              },
            );

  return (
    <div
      className={`download-bar ${progress.state}`}
      aria-live="polite"
      aria-busy={isActive}
    >
      <div className="download-copy">
        <strong>Studio Pro {progress.version}</strong>
        <span>{description}</span>
      </div>
      <div className="progress-visual">
        <div
          className="progress-track"
          role="progressbar"
          aria-label={t("progress-aria", { version: progress.version })}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={roundedPercentage}
          aria-valuetext={`${description} ${progressLabel}`}
        >
          <span
            className={isActive ? "active" : ""}
            style={{ width: `${Math.max(2, boundedPercentage)}%` }}
          />
          <div className="progress-boundaries" aria-hidden="true">
            {[10, 58, 68, 97].map((boundary) => (
              <i key={boundary} style={{ insetInlineStart: `${boundary}%` }} />
            ))}
          </div>
        </div>
        <div className="progress-stages" aria-hidden="true">
          {INSTALLATION_STAGE_KEYS.map((labelKey, index) => (
            <span
              className={
                progress.state === "installed" || index < currentStage
                  ? "complete"
                  : index === currentStage
                    ? "current"
                    : ""
              }
              key={labelKey}
            >
              <i />
              {t(labelKey)}
            </span>
          ))}
        </div>
      </div>
      <b className="progress-percentage">{progressLabel}</b>
      {isInstalling &&
        ["connecting", "downloading"].includes(progress.state) && (
          <button type="button" onClick={onCancel}>
            {t("action-cancel")}
          </button>
        )}
    </div>
  );
}

function progressDescription(
  progress: DownloadProgress,
  t: Translate,
  downloadedLabel: string,
  totalLabel: string,
) {
  const message = LOCALIZED_PROGRESS_STATES.has(progress.state)
    ? t(`progress-${progress.state}`)
    : progress.message;
  if (progress.state !== "downloading" || progress.totalBytes == null) {
    return message;
  }
  return `${message} ${downloadedLabel} / ${totalLabel}`;
}
