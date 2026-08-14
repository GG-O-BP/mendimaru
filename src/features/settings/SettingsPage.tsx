import { CheckCircle2, Info, LoaderCircle, RefreshCw } from "lucide-react";
import type { AppConfig, ContainerRuntime } from "../../domain/types";
import type { Translate } from "../../i18n";
import { PageTitle } from "../../shared/components/LayoutPrimitives";

export type PathField = "sharedDirectory" | "composeFile" | "winboatExecutable";

export interface SettingsPageModel {
  config: AppConfig;
  changed: boolean;
  mountMatches: boolean;
  applyNow: boolean;
  isBusy: (key: string) => boolean;
  onChange: (config: AppConfig) => void;
  onChoose: (field: PathField) => void;
  onApplyNow: (value: boolean) => void;
  onSave: () => void;
  onRedetect: () => void;
}

export function SettingsPage({
  t,
  model,
}: {
  t: Translate;
  model: SettingsPageModel;
}) {
  const { config } = model;
  return (
    <div className="settings-page">
      <PageTitle
        eyebrow={t("settings-eyebrow")}
        title={t("settings-title")}
        description={t("settings-description")}
      />

      <div className="settings-route" aria-label={t("settings-route-aria")}>
        <span className="active">
          <b>01</b>
          {t("settings-step-winboat")}
        </span>
        <i aria-hidden="true" />
        <span className={model.mountMatches ? "complete" : "active"}>
          <b>02</b>
          {t("settings-step-workspace")}
        </span>
      </div>

      <section
        className="section-card settings-card"
        aria-labelledby="winboat-settings-heading"
      >
        <div className="settings-heading">
          <span className="settings-number" aria-hidden="true">
            01
          </span>
          <div>
            <span className="micro-label">
              {t("settings-step-environment")}
            </span>
            <h2 id="winboat-settings-heading">WinBoat</h2>
            <p>{t("settings-winboat-description")}</p>
          </div>
          <button
            type="button"
            className="button secondary compact"
            onClick={model.onRedetect}
            disabled={model.isBusy("redetect")}
          >
            <RefreshCw
              size={15}
              className={model.isBusy("redetect") ? "spin" : ""}
            />
            {t("action-auto-detect")}
          </button>
        </div>
        <PathInput
          label={t("settings-winboat-executable")}
          browseLabel={t("action-browse")}
          value={config.winboatExecutable}
          onChange={(value) =>
            model.onChange({ ...config, winboatExecutable: value })
          }
          onBrowse={() => model.onChoose("winboatExecutable")}
        />
        <PathInput
          label={t("settings-compose-file")}
          browseLabel={t("action-browse")}
          value={config.composeFile}
          onChange={(value) =>
            model.onChange({ ...config, composeFile: value })
          }
          onBrowse={() => model.onChoose("composeFile")}
        />
        <label className="simple-field runtime-field">
          <span>{t("settings-container-runtime")}</span>
          <select
            value={config.containerRuntime}
            onChange={(event) =>
              model.onChange({
                ...config,
                containerRuntime: event.target.value as ContainerRuntime,
              })
            }
          >
            <option value="docker">Docker</option>
            <option value="podman">Podman</option>
          </select>
        </label>
      </section>

      <section
        className="section-card settings-card"
        aria-labelledby="workspace-settings-heading"
      >
        <div className="settings-heading">
          <span className="settings-number amber" aria-hidden="true">
            02
          </span>
          <div>
            <span className="micro-label">{t("settings-step-cargo")}</span>
            <h2 id="workspace-settings-heading">
              {t("settings-workspace-title")}
            </h2>
            <p>{t("settings-workspace-description")}</p>
          </div>
          <span
            className={`mount-state ${model.mountMatches ? "ok" : "pending"}`}
          >
            {model.mountMatches ? (
              <CheckCircle2 size={14} />
            ) : (
              <Info size={14} />
            )}
            {model.mountMatches ? t("mount-connected") : t("mount-pending")}
          </span>
        </div>
        <PathInput
          label={t("settings-shared-directory")}
          browseLabel={t("action-browse")}
          value={config.sharedDirectory}
          onChange={(value) =>
            model.onChange({ ...config, sharedDirectory: value })
          }
          onBrowse={() => model.onChoose("sharedDirectory")}
        />
        <label className="apply-row">
          <input
            type="checkbox"
            checked={model.applyNow}
            onChange={(event) => model.onApplyNow(event.target.checked)}
          />
          <span>
            <strong>{t("settings-apply-now-title")}</strong>
            <small>{t("settings-apply-now-detail")}</small>
          </span>
        </label>
      </section>

      <div
        className={`settings-actions ${model.changed ? "changed" : ""}`}
        aria-live="polite"
      >
        <span>
          {model.changed ? <Info size={17} /> : <CheckCircle2 size={17} />}
          {model.changed ? t("settings-unsaved") : t("settings-saved")}
        </span>
        <button
          type="button"
          className="button primary"
          onClick={model.onSave}
          disabled={!model.changed || model.isBusy("save-settings")}
        >
          {model.isBusy("save-settings") ? (
            <LoaderCircle size={16} className="spin" />
          ) : (
            <CheckCircle2 size={16} />
          )}
          {t("action-save-settings")}
        </button>
      </div>
    </div>
  );
}

function PathInput({
  label,
  browseLabel,
  value,
  onChange,
  onBrowse,
}: {
  label: string;
  browseLabel: string;
  value: string;
  onChange: (value: string) => void;
  onBrowse: () => void;
}) {
  return (
    <label className="simple-field path-field">
      <span>{label}</span>
      <div>
        <input
          value={value}
          onChange={(event) => onChange(event.target.value)}
          spellCheck={false}
        />
        <button type="button" onClick={onBrowse}>
          {browseLabel}
        </button>
      </div>
    </label>
  );
}
