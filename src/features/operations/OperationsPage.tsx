import {
  AlertTriangle,
  CheckCircle2,
  Clock3,
  FolderOpen,
  History,
  LoaderCircle,
  RefreshCw,
  RotateCcw,
  Trash2,
  XCircle,
} from "lucide-react";
import type {
  LocalizationBundle,
  OperationRecord,
  OperationStage,
  OperationState,
} from "../../domain/types";
import type { MessageKey, Translate } from "../../i18n";
import {
  EmptyState,
  PageTitle,
  SectionHeader,
} from "../../shared/components/LayoutPrimitives";
import {
  useLocalizedDates,
  useLocalizedNumbers,
} from "../../shared/hooks/useLocalizedValues";

const STATE_KEYS: Record<OperationState, MessageKey> = {
  running: "operation-state-running",
  succeeded: "operation-state-succeeded",
  failed: "operation-state-failed",
  cancelled: "operation-state-cancelled",
  interrupted: "operation-state-interrupted",
};

const STAGE_KEYS: Record<OperationStage, MessageKey> = {
  starting: "operation-stage-starting",
  preparing: "operation-stage-preparing",
  checking: "operation-stage-checking",
  connecting: "operation-stage-connecting",
  downloading: "operation-stage-downloading",
  downloaded: "operation-stage-downloaded",
  ready: "operation-stage-ready",
  staging: "operation-stage-staging",
  installing: "operation-stage-installing",
  finalizing: "operation-stage-finalizing",
  verifying: "operation-stage-verifying",
  launching: "operation-stage-launching",
  uninstalling: "operation-stage-uninstalling",
  completed: "operation-stage-completed",
  interrupted: "operation-stage-interrupted",
};

const REASON_KEYS: Record<string, MessageKey> = {
  operation_interrupted: "operation-reason-operation-interrupted",
  legacy_report_untrusted: "operation-reason-legacy-report-untrusted",
  config_load_failed: "operation-reason-config-load-failed",
  download_cancelled: "operation-reason-download-cancelled",
  install_failed: "operation-reason-install-failed",
  unsupported_capability: "operation-reason-unsupported-capability",
  backend_mismatch: "operation-reason-backend-mismatch",
  invalid_request: "operation-reason-invalid-request",
  precondition_failed: "operation-reason-precondition-failed",
  operation_failed: "operation-reason-operation-failed",
};

export interface OperationsPageModel {
  operations: OperationRecord[];
  loading: boolean;
  isBusy: (key: string) => boolean;
  onRefresh: () => void;
  onRetry: (operation: OperationRecord) => void;
  onClear: () => void;
  onOpenLogs: () => void;
}

export function OperationsPage({
  t,
  localization,
  model,
}: {
  t: Translate;
  localization: LocalizationBundle;
  model: OperationsPageModel;
}) {
  const [count] = useLocalizedNumbers([model.operations.length], localization);
  const dates = useLocalizedDates(
    model.operations.flatMap((operation) => [
      operation.startedAt,
      operation.finishedAt,
    ]),
    localization,
  );
  const hasTerminal = model.operations.some(
    (operation) => operation.state !== "running",
  );
  const hasLogs = model.operations.some((operation) => operation.logAvailable);

  return (
    <div className="operations-page">
      <PageTitle
        eyebrow={t("operations-eyebrow")}
        title={t("operations-title")}
        description={t("operations-description")}
      />

      <section className="section-card" aria-labelledby="operations-heading">
        <SectionHeader
          id="operations-heading"
          title={t("operations-history")}
          count={count}
          meta={t("operations-history-meta")}
          action={
            <div className="operation-toolbar">
              <button
                type="button"
                className="button secondary compact"
                onClick={model.onOpenLogs}
                disabled={!hasLogs || model.isBusy("open-operation-logs")}
              >
                <FolderOpen size={16} />
                {t("action-open-operation-logs")}
              </button>
              <button
                type="button"
                className="button quiet compact"
                onClick={model.onClear}
                disabled={
                  !hasTerminal || model.isBusy("clear-operation-history")
                }
              >
                <Trash2 size={16} />
                {t("action-clear-operation-history")}
              </button>
              <button
                type="button"
                className="icon-button"
                onClick={model.onRefresh}
                disabled={model.loading}
                title={t("refresh-operation-history")}
                aria-label={t("refresh-operation-history")}
              >
                <RefreshCw size={17} className={model.loading ? "spin" : ""} />
              </button>
            </div>
          }
        />

        {model.operations.length === 0 ? (
          <EmptyState
            icon={History}
            title={t("operations-empty-title")}
            detail={t("operations-empty-detail")}
          />
        ) : (
          <div className="operation-list">
            {model.operations.map((operation, index) => (
              <OperationItem
                key={operation.id}
                t={t}
                operation={operation}
                startedAt={dates[index * 2]}
                finishedAt={dates[index * 2 + 1]}
                busy={model.isBusy(`retry-operation-${operation.id}`)}
                onRetry={() => model.onRetry(operation)}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function OperationItem({
  t,
  operation,
  startedAt,
  finishedAt,
  busy,
  onRetry,
}: {
  t: Translate;
  operation: OperationRecord;
  startedAt: string;
  finishedAt: string;
  busy: boolean;
  onRetry: () => void;
}) {
  const progress = Math.round(operation.percentage ?? 0);
  return (
    <article className={`operation-item ${operation.state}`}>
      <span className="operation-state-icon" aria-hidden="true">
        {stateIcon(operation.state)}
      </span>
      <div className="operation-main">
        <div className="operation-heading">
          <div>
            <strong>
              {t(`operation-kind-${operation.kind}` as MessageKey)} ·{" "}
              {operation.targetVersion}
            </strong>
            {operation.protectedProject && (
              <span>{t("operation-project-protected")}</span>
            )}
          </div>
          <span className={`operation-state ${operation.state}`}>
            {t(STATE_KEYS[operation.state])}
          </span>
        </div>
        <code className="operation-id" title={operation.id}>
          {operation.id}
        </code>
        <div className="operation-progress-row">
          <span>{t(STAGE_KEYS[operation.stage])}</span>
          {operation.percentage !== undefined ? (
            <>
              <div
                className="operation-progress-track"
                role="progressbar"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progress}
              >
                <i style={{ width: `${progress}%` }} />
              </div>
              <b>
                {operation.estimated && t("operation-estimated")}
                {progress}%
              </b>
            </>
          ) : (
            <b>{t("operation-progress-unavailable")}</b>
          )}
        </div>
        <div className="operation-timestamps">
          <span>{t("operation-started-at", { date: startedAt })}</span>
          {finishedAt && (
            <span>{t("operation-finished-at", { date: finishedAt })}</span>
          )}
        </div>
        {operation.error && (
          <div className="operation-error" role="status">
            <AlertTriangle size={15} />
            <span>
              {t(
                REASON_KEYS[operation.error.reason] ??
                  "operation-reason-unknown",
              )}
              {operation.error.exitCode !== undefined &&
                ` · ${t("operation-exit-code", { code: operation.error.exitCode })}`}
            </span>
            <code>{operation.error.code}</code>
          </div>
        )}
      </div>
      <div className="operation-actions">
        <button
          type="button"
          className="button secondary compact"
          onClick={onRetry}
          disabled={!operation.retryable || busy}
          title={
            operation.retryable ? undefined : t("operation-retry-unavailable")
          }
        >
          {busy ? (
            <LoaderCircle size={15} className="spin" />
          ) : (
            <RotateCcw size={15} />
          )}
          {t("action-retry-operation")}
        </button>
      </div>
    </article>
  );
}

function stateIcon(state: OperationState) {
  switch (state) {
    case "running":
      return <Clock3 size={21} />;
    case "succeeded":
      return <CheckCircle2 size={21} />;
    case "failed":
      return <XCircle size={21} />;
    case "cancelled":
    case "interrupted":
      return <AlertTriangle size={21} />;
  }
}
