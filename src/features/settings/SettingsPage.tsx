import {
  AlertTriangle,
  CheckCircle2,
  CircleX,
  Copy,
  Download,
  Info,
  LoaderCircle,
  Plus,
  RefreshCw,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
import type {
  AppConfig,
  ContainerRuntime,
  EnvironmentDiagnostic,
  EnvironmentDiagnosticAction,
  EnvironmentDiagnosticId,
} from "../../domain/types";
import type { MessageKey, Translate } from "../../i18n";
import { PageTitle } from "../../shared/components/LayoutPrimitives";
import {
  ADVANCED_SETTING_FIELDS,
  validateAdvancedSettings,
  type AdvancedSettingField,
} from "./advancedSettings";
import { diagnosticText } from "./environmentDiagnostics";

export type PathField = "sharedDirectory" | "composeFile" | "winboatExecutable";

export interface SettingsPageModel {
  config: AppConfig;
  nativeWindows: boolean;
  changed: boolean;
  mountMatches: boolean;
  applyNow: boolean;
  diagnostics: EnvironmentDiagnostic[];
  isBusy: (key: string) => boolean;
  onChange: (config: AppConfig) => void;
  onChoose: (field: PathField) => void;
  onAddStudioPath: () => void;
  onRemoveStudioPath: (index: number) => void;
  onApplyNow: (value: boolean) => void;
  onSave: () => void;
  onRedetect: () => void;
  onDetectField: (field: AdvancedSettingField) => void;
  onRestoreAdvancedDefaults: () => void;
  onTestConnection: () => void;
  connectionTest: {
    online: boolean;
    endpoint: string;
  } | null;
  onDiagnosticAction: (
    action: EnvironmentDiagnosticAction,
    id: EnvironmentDiagnosticId,
  ) => void;
  onCopyDiagnosticReport: () => void;
  onExportDiagnosticReport: () => void;
}

const ADVANCED_LABEL_KEYS: Record<AdvancedSettingField, MessageKey> = {
  containerName: "settings-advanced-container-name",
  apiUrl: "settings-advanced-api-url",
  rdpHost: "settings-advanced-rdp-host",
  rdpPort: "settings-advanced-rdp-port",
  windowsSharedDirectory: "settings-advanced-windows-shared-directory",
  freerdpBinary: "settings-advanced-freerdp-binary",
  mendixInstallRoot: "settings-advanced-mendix-install-root",
  mendixDataRoot: "settings-advanced-mendix-data-root",
  startupTimeoutSeconds: "settings-advanced-startup-timeout-seconds",
};

export function SettingsPage({
  t,
  model,
}: {
  t: Translate;
  model: SettingsPageModel;
}) {
  const { config } = model;
  const advancedErrors = validateAdvancedSettings(config, t);
  return (
    <div
      className="settings-page"
      data-testid={
        model.nativeWindows ? "settings-native-page" : "settings-winboat-page"
      }
    >
      <PageTitle
        eyebrow={t("settings-eyebrow")}
        title={t("settings-title")}
        description={
          model.nativeWindows
            ? t("settings-native-page-description")
            : t("settings-description")
        }
      />

      <div className="settings-route" aria-label={t("settings-route-aria")}>
        <span className="active">
          <b>01</b>
          {model.nativeWindows
            ? t("settings-step-native")
            : t("settings-step-winboat")}
        </span>
        <i aria-hidden="true" />
        <span className={model.mountMatches ? "complete" : "active"}>
          <b>02</b>
          {t("settings-step-workspace")}
        </span>
      </div>

      <section
        className="section-card settings-card"
        aria-labelledby="environment-settings-heading"
      >
        <div className="settings-heading">
          <span className="settings-number" aria-hidden="true">
            01
          </span>
          <div>
            <span className="micro-label">
              {t("settings-step-environment")}
            </span>
            <h2 id="environment-settings-heading">
              {model.nativeWindows ? t("settings-native-title") : "WinBoat"}
            </h2>
            <p>
              {model.nativeWindows
                ? t("settings-native-description")
                : t("settings-winboat-description")}
            </p>
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
        {model.nativeWindows ? (
          <div className="custom-studio-paths">
            <div className="custom-path-heading">
              <div>
                <strong>{t("settings-custom-versions-title")}</strong>
                <span>{t("settings-custom-versions-description")}</span>
              </div>
              <button
                type="button"
                className="button secondary compact"
                onClick={model.onAddStudioPath}
              >
                <Plus size={15} />
                {t("action-add-studio-path")}
              </button>
            </div>
            {config.windowsStudioPaths.length === 0 ? (
              <p className="custom-path-empty">
                {t("settings-custom-versions-empty")}
              </p>
            ) : (
              <div className="custom-path-list">
                {config.windowsStudioPaths.map((path, index) => (
                  <label key={`${path}-${index}`}>
                    <span>{t("settings-custom-version-path")}</span>
                    <div>
                      <input
                        value={path}
                        onChange={(event) => {
                          const paths = [...config.windowsStudioPaths];
                          paths[index] = event.target.value;
                          model.onChange({
                            ...config,
                            windowsStudioPaths: paths,
                          });
                        }}
                        spellCheck={false}
                      />
                      <button
                        type="button"
                        title={t("action-remove-studio-path")}
                        aria-label={t("action-remove-studio-path")}
                        onClick={() => model.onRemoveStudioPath(index)}
                      >
                        <Trash2 size={16} />
                      </button>
                    </div>
                  </label>
                ))}
              </div>
            )}
          </div>
        ) : (
          <>
            <PathInput
              id="winboat-executable-input"
              label={t("settings-winboat-executable")}
              browseLabel={t("action-browse")}
              value={config.winboatExecutable}
              onChange={(value) =>
                model.onChange({ ...config, winboatExecutable: value })
              }
              onBrowse={() => model.onChoose("winboatExecutable")}
            />
            <PathInput
              id="compose-file-input"
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
                id="container-runtime-input"
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
          </>
        )}
        {!model.nativeWindows && (
          <details
            className="advanced-settings"
            data-testid="advanced-settings"
          >
            <summary>
              <SlidersHorizontal size={16} aria-hidden="true" />
              <span>
                <strong>{t("settings-advanced-title")}</strong>
                <small>{t("settings-advanced-description")}</small>
              </span>
            </summary>
            <div className="advanced-settings-body">
              <div className="advanced-settings-actions">
                <button
                  type="button"
                  className="button secondary compact"
                  onClick={model.onRestoreAdvancedDefaults}
                  disabled={model.isBusy("restore-advanced-defaults")}
                >
                  <RotateCcw size={15} />
                  {t("action-restore-defaults")}
                </button>
                <button
                  type="button"
                  className="button secondary compact"
                  onClick={model.onTestConnection}
                  disabled={
                    Object.keys(advancedErrors).length > 0 ||
                    model.isBusy("test-settings-connection")
                  }
                >
                  {model.isBusy("test-settings-connection") ? (
                    <LoaderCircle size={15} className="spin" />
                  ) : (
                    <RefreshCw size={15} />
                  )}
                  {t("action-test-connection")}
                </button>
              </div>
              <p className="advanced-settings-warning">
                <AlertTriangle size={15} aria-hidden="true" />
                {t("settings-advanced-warning")}
              </p>
              <div className="advanced-field-list">
                {ADVANCED_SETTING_FIELDS.map((field) => (
                  <AdvancedSettingInput
                    key={field}
                    field={field}
                    value={config[field]}
                    label={t(ADVANCED_LABEL_KEYS[field])}
                    error={advancedErrors[field]}
                    busy={model.isBusy(`detect-setting-${field}`)}
                    onChange={(value) =>
                      model.onChange({ ...config, [field]: value })
                    }
                    onDetect={() => model.onDetectField(field)}
                    detectLabel={t("action-auto-detect")}
                  />
                ))}
              </div>
              {model.connectionTest && (
                <p
                  className={`connection-test-result ${
                    model.connectionTest.online ? "online" : "offline"
                  }`}
                  data-testid="connection-test-result"
                >
                  {model.connectionTest.online
                    ? t("settings-connection-online")
                    : t("settings-connection-offline")}
                  <small>{model.connectionTest.endpoint}</small>
                </p>
              )}
            </div>
          </details>
        )}
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
              {model.nativeWindows
                ? t("settings-native-workspace-title")
                : t("settings-workspace-title")}
            </h2>
            <p>
              {model.nativeWindows
                ? t("settings-native-workspace-description")
                : t("settings-workspace-description")}
            </p>
          </div>
          <span
            className={`mount-state ${model.mountMatches ? "ok" : "pending"}`}
          >
            {model.mountMatches ? (
              <CheckCircle2 size={14} />
            ) : (
              <Info size={14} />
            )}
            {model.nativeWindows
              ? model.mountMatches
                ? t("workspace-ready")
                : t("workspace-missing")
              : model.mountMatches
                ? t("mount-connected")
                : t("mount-pending")}
          </span>
        </div>
        <PathInput
          id="shared-directory-input"
          testId="workspace-path"
          label={
            model.nativeWindows
              ? t("settings-native-workspace-directory")
              : t("settings-shared-directory")
          }
          browseLabel={t("action-browse")}
          value={config.sharedDirectory}
          onChange={(value) =>
            model.onChange({ ...config, sharedDirectory: value })
          }
          onBrowse={() => model.onChoose("sharedDirectory")}
        />
        {!model.nativeWindows && (
          <label className="apply-row">
            <input
              id="apply-mount-input"
              type="checkbox"
              checked={model.applyNow}
              onChange={(event) => model.onApplyNow(event.target.checked)}
            />
            <span>
              <strong>{t("settings-apply-now-title")}</strong>
              <small>{t("settings-apply-now-detail")}</small>
            </span>
          </label>
        )}
      </section>

      <section
        className="section-card settings-card diagnostic-card"
        aria-labelledby="environment-diagnostics-heading"
      >
        <div className="settings-heading diagnostic-heading">
          <span
            className="settings-number diagnostic-number"
            aria-hidden="true"
          >
            03
          </span>
          <div>
            <span className="micro-label">{t("diagnostics-eyebrow")}</span>
            <h2 id="environment-diagnostics-heading">
              {t("diagnostics-title")}
            </h2>
            <p>{t("diagnostics-description")}</p>
          </div>
          <div className="diagnostic-report-actions">
            <button
              type="button"
              className="button secondary compact"
              onClick={model.onCopyDiagnosticReport}
              disabled={model.isBusy("copy-environment-report")}
            >
              <Copy size={15} />
              {t("action-copy-diagnostic-report")}
            </button>
            <button
              type="button"
              className="button secondary compact"
              onClick={model.onExportDiagnosticReport}
              disabled={model.isBusy("export-environment-report")}
            >
              <Download size={15} />
              {t("action-export-diagnostic-report")}
            </button>
          </div>
        </div>
        <div className="diagnostic-list" role="list">
          {model.diagnostics.map((diagnostic) => {
            const text = diagnosticText(diagnostic, t);
            const busyKey = diagnosticActionBusyKey(diagnostic.action);
            return (
              <article
                key={diagnostic.id}
                className={`diagnostic-item ${diagnostic.status}`}
                role="listitem"
              >
                <span className="diagnostic-icon" aria-hidden="true">
                  {diagnostic.status === "success" ? (
                    <CheckCircle2 size={18} />
                  ) : diagnostic.status === "warning" ? (
                    <AlertTriangle size={18} />
                  ) : (
                    <CircleX size={18} />
                  )}
                </span>
                <div>
                  <div className="diagnostic-item-heading">
                    <strong>{text.title}</strong>
                    <span>{text.status}</span>
                  </div>
                  <p>{text.detail}</p>
                </div>
                {diagnostic.action && text.action && (
                  <button
                    type="button"
                    className="button secondary compact"
                    disabled={busyKey ? model.isBusy(busyKey) : false}
                    onClick={() =>
                      model.onDiagnosticAction(
                        diagnostic.action!,
                        diagnostic.id,
                      )
                    }
                  >
                    {text.action}
                  </button>
                )}
              </article>
            );
          })}
        </div>
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
          data-testid="save-settings"
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
  id,
  testId,
  label,
  browseLabel,
  value,
  onChange,
  onBrowse,
}: {
  id?: string;
  testId?: string;
  label: string;
  browseLabel: string;
  value: string;
  onChange: (value: string) => void;
  onBrowse: () => void;
}) {
  return (
    <label className="simple-field path-field" data-testid={testId}>
      <span>{label}</span>
      <div>
        <input
          id={id}
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

function AdvancedSettingInput({
  field,
  label,
  value,
  error,
  busy,
  detectLabel,
  onChange,
  onDetect,
}: {
  field: AdvancedSettingField;
  label: string;
  value: string | number;
  error?: string;
  busy: boolean;
  detectLabel: string;
  onChange: (value: string | number) => void;
  onDetect: () => void;
}) {
  const numeric =
    field === "rdpPort" || field === "startupTimeoutSeconds" ? true : false;
  return (
    <label
      className={`advanced-field ${error ? "invalid" : ""}`}
      data-testid={`advanced-${field}`}
    >
      <span>{label}</span>
      <div>
        <input
          type={numeric ? "number" : "text"}
          value={value}
          min={numeric ? 1 : undefined}
          max={
            field === "rdpPort"
              ? 65_535
              : field === "startupTimeoutSeconds"
                ? 900
                : undefined
          }
          step={numeric ? 1 : undefined}
          aria-invalid={Boolean(error)}
          onChange={(event) =>
            onChange(numeric ? Number(event.target.value) : event.target.value)
          }
          spellCheck={false}
        />
        <button type="button" onClick={onDetect} disabled={busy}>
          {busy ? <LoaderCircle size={14} className="spin" /> : null}
          {detectLabel}
        </button>
      </div>
      {error && (
        <small className="field-error" role="alert">
          {error}
        </small>
      )}
    </label>
  );
}

function diagnosticActionBusyKey(action?: EnvironmentDiagnosticAction) {
  if (action === "redetect") return "redetect";
  if (action === "start-winboat") return "start-windows";
  if (action === "open-winboat") return "open-winboat";
  return null;
}
