import {
  AlertTriangle,
  Download,
  FolderInput,
  LoaderCircle,
  Search,
} from "lucide-react";
import { useEffect, useId, useRef } from "react";
import type { LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import { InstallationProgress } from "../studio/InstallationProgress";
import type { useProjectLauncher } from "./useProjectLauncher";

export function ProjectLaunchAssistant({
  t,
  localization,
  launcher,
}: {
  t: Translate;
  localization: LocalizationBundle;
  launcher: ReturnType<typeof useProjectLauncher>;
}) {
  const state = launcher.assistant;
  const closeAssistant = launcher.closeAssistant;
  const activeProjectPath = state?.project.mprPath;
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    if (!activeProjectPath) return undefined;
    const previouslyFocused =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    cancelRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeAssistant();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        dialogRef.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      );
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previouslyFocused?.focus();
    };
  }, [activeProjectPath, closeAssistant]);

  if (!state) return null;

  const canContinue =
    Boolean(state.selectedVersion) &&
    launcher.studioLaunchReady &&
    !launcher.connectedRemoteAppVersion &&
    (launcher.selectedInstalled || launcher.selectedDownloadable) &&
    state.lookupState !== "loading" &&
    !launcher.actionBusy &&
    !launcher.isInstalling &&
    (!launcher.safetyRequired || state.safetyAcknowledged);
  const progress =
    launcher.downloadProgress?.version === state.selectedVersion
      ? launcher.downloadProgress
      : null;

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) launcher.closeAssistant();
      }}
    >
      <div
        ref={dialogRef}
        className="project-launch-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <header>
          <span className="dialog-symbol" aria-hidden="true">
            <Download size={21} />
          </span>
          <div>
            <span className="micro-label">{t("project-launch-eyebrow")}</span>
            <h2 id={titleId}>
              {t("project-launch-title", { project: state.project.name })}
            </h2>
          </div>
        </header>
        <p id={descriptionId}>{t("project-launch-description")}</p>

        {launcher.connectedRemoteAppVersion && (
          <div className="project-launch-blocked" role="alert">
            <AlertTriangle size={18} aria-hidden="true" />
            <span>
              <strong>
                {t("studio-connected-session-blocks-title", {
                  version: launcher.connectedRemoteAppVersion,
                })}
              </strong>
              {t("studio-connected-session-blocks-detail", {
                version: launcher.connectedRemoteAppVersion,
              })}
            </span>
          </div>
        )}

        {state.project.location === "explicit-host-selection" && (
          <div className="external-project-share-notice" role="status">
            <FolderInput size={18} aria-hidden="true" />
            <span>
              <strong>{t("external-project-share-title")}</strong>
              {t("external-project-share-detail")}
            </span>
          </div>
        )}

        <dl className="project-launch-requirement">
          <div>
            <dt>{t("project-launch-required-version")}</dt>
            <dd>{state.project.version ?? t("version-unknown")}</dd>
          </div>
          {state.project.preferredVersion && (
            <div>
              <dt>{t("project-launch-remembered-version")}</dt>
              <dd>{state.project.preferredVersion}</dd>
            </div>
          )}
        </dl>

        <label className="project-version-select">
          <span>{t("project-launch-select-version")}</span>
          <select
            value={state.selectedVersion}
            onChange={(event) => launcher.selectVersion(event.target.value)}
            disabled={launcher.actionBusy || launcher.isInstalling}
          >
            <option value="">{t("project-launch-select-placeholder")}</option>
            {launcher.versionOptions.map((version) => (
              <option value={version} key={version}>
                {version}
              </option>
            ))}
          </select>
        </label>

        <form
          className="project-version-lookup"
          onSubmit={(event) => {
            event.preventDefault();
            launcher.lookupVersion();
          }}
        >
          <label>
            <span>{t("project-launch-exact-version")}</span>
            <input
              value={state.versionInput}
              onChange={(event) => launcher.setVersionInput(event.target.value)}
              placeholder={t("project-launch-version-placeholder")}
              disabled={launcher.actionBusy || launcher.isInstalling}
            />
          </label>
          <button
            type="submit"
            className="button secondary compact"
            disabled={
              !state.versionInput.trim() ||
              state.lookupState === "loading" ||
              launcher.actionBusy ||
              launcher.isInstalling
            }
          >
            {state.lookupState === "loading" ? (
              <LoaderCircle size={15} className="spin" />
            ) : (
              <Search size={15} />
            )}
            {t("project-launch-find-exact")}
          </button>
        </form>

        {state.lookupState === "error" && state.lookupError && (
          <div className="inline-error" role="alert">
            <AlertTriangle size={16} />
            <span>{state.lookupError}</span>
            <button type="button" onClick={launcher.lookupVersion}>
              {t("action-retry")}
            </button>
          </div>
        )}

        {state.selectedVersion && state.lookupState !== "loading" && (
          <div className="project-launch-selection" aria-live="polite">
            <strong>{state.selectedVersion}</strong>
            <span>
              {launcher.selectedInstalled
                ? t("project-launch-ready")
                : launcher.selectedDownloadable
                  ? t("project-launch-install-required")
                  : t("project-launch-version-not-resolved")}
            </span>
          </div>
        )}

        {launcher.safetyRequired && (
          <div className="project-launch-safety">
            <AlertTriangle size={18} aria-hidden="true" />
            <span>
              <strong>{t("project-launch-mismatch-title")}</strong>
              {t("project-launch-mismatch-detail")}
              <label>
                <input
                  type="checkbox"
                  checked={state.safetyAcknowledged}
                  onChange={(event) =>
                    launcher.setSafetyAcknowledged(event.target.checked)
                  }
                  disabled={launcher.actionBusy || launcher.isInstalling}
                />
                {t("project-launch-mismatch-acknowledge")}
              </label>
            </span>
          </div>
        )}

        {progress && (
          <InstallationProgress
            t={t}
            localization={localization}
            progress={progress}
            isInstalling={launcher.isInstalling}
            onCancel={() => void launcher.cancelDownload()}
          />
        )}

        <footer>
          <button
            ref={cancelRef}
            type="button"
            className="button secondary"
            onClick={launcher.closeAssistant}
            disabled={launcher.actionBusy || launcher.isInstalling}
          >
            {t("action-cancel")}
          </button>
          <button
            type="button"
            data-testid="continue-project-launch"
            className="button primary"
            onClick={launcher.continueAssistant}
            disabled={!canContinue}
          >
            {launcher.actionBusy ? (
              <LoaderCircle size={16} className="spin" />
            ) : launcher.selectedInstalled ? null : (
              <Download size={16} />
            )}
            {launcher.selectedInstalled
              ? t("project-launch-open")
              : t("project-launch-install-open")}
          </button>
        </footer>
      </div>
    </div>
  );
}
