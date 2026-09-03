import {
  ArrowDown,
  ArrowUp,
  Ban,
  CheckCircle2,
  LoaderCircle,
  RotateCcw,
  Trash2,
  XCircle,
} from "lucide-react";
import type {
  InstallQueueItem,
  InstallQueueState,
  LocalizationBundle,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  localizedNumber,
  useLocalizedBytes,
} from "../../shared/hooks/useLocalizedValues";

const TERMINAL_STATES: ReadonlySet<InstallQueueState> = new Set([
  "succeeded",
  "failed",
  "cancelled",
]);

const ACTIVE_STATES: ReadonlySet<InstallQueueState> = new Set([
  "downloading",
  "staging",
  "installing",
]);

export function InstallQueuePanel({
  t,
  localization,
  items,
  onCancel,
  onDiscard,
  onRetry,
  onMove,
  onRemove,
}: {
  t: Translate;
  localization: LocalizationBundle;
  items: InstallQueueItem[];
  onCancel: (itemId: string) => void;
  onDiscard: (itemId: string) => void;
  onRetry: (itemId: string) => void;
  onMove: (itemId: string, up: boolean) => void;
  onRemove: (itemId: string) => void;
}) {
  if (items.length === 0) return null;

  return (
    <div className="install-queue" data-testid="install-queue">
      <h4>{t("install-queue-title")}</h4>
      <ul>
        {items.map((item, index) => (
          <QueueRow
            key={item.id}
            t={t}
            localization={localization}
            item={item}
            firstPending={index === firstPendingIndex(items)}
            lastPending={index === lastPendingIndex(items)}
            onCancel={onCancel}
            onDiscard={onDiscard}
            onRetry={onRetry}
            onMove={onMove}
            onRemove={onRemove}
          />
        ))}
      </ul>
    </div>
  );
}

function QueueRow({
  t,
  localization,
  item,
  firstPending,
  lastPending,
  onCancel,
  onDiscard,
  onRetry,
  onMove,
  onRemove,
}: {
  t: Translate;
  localization: LocalizationBundle;
  item: InstallQueueItem;
  firstPending: boolean;
  lastPending: boolean;
  onCancel: (itemId: string) => void;
  onDiscard: (itemId: string) => void;
  onRetry: (itemId: string) => void;
  onMove: (itemId: string, up: boolean) => void;
  onRemove: (itemId: string) => void;
}) {
  const active = ACTIVE_STATES.has(item.state);
  const terminal = TERMINAL_STATES.has(item.state);
  const queued = item.state === "queued";
  const [downloadedLabel, totalLabel] = useLocalizedBytes(
    [item.downloadedBytes ?? 0, item.totalBytes ?? 0],
    localization,
  );
  const percentage = Math.round(
    Math.min(100, Math.max(0, item.percentage ?? 0)),
  );

  return (
    <li
      className={`install-queue-item ${item.state}`}
      data-version={item.version}
    >
      <div className="install-queue-summary">
        {active ? (
          <LoaderCircle size={16} className="spin" />
        ) : item.state === "succeeded" ? (
          <CheckCircle2 size={16} />
        ) : terminal ? (
          <XCircle size={16} />
        ) : null}
        <strong>{item.version}</strong>
        <span className={`install-queue-state ${item.state}`}>
          {t(`install-queue-state-${item.state}`)}
        </span>
        {active && item.totalBytes ? (
          <span className="install-queue-progress-label">
            {downloadedLabel} / {totalLabel}
          </span>
        ) : null}
      </div>
      {active && (
        <progress max={100} value={percentage || 1}>
          {localizedNumber(localization, percentage)}%
        </progress>
      )}
      {item.state === "failed" && item.message ? (
        <p className="install-queue-message" role="alert">
          {item.message}
        </p>
      ) : null}
      <div className="install-queue-actions">
        {queued && !firstPending && (
          <button
            type="button"
            className="icon-button compact"
            aria-label={t("install-queue-move-up", {
              version: item.version,
            })}
            onClick={() => onMove(item.id, true)}
          >
            <ArrowUp size={14} />
          </button>
        )}
        {queued && !lastPending && (
          <button
            type="button"
            className="icon-button compact"
            aria-label={t("install-queue-move-down", {
              version: item.version,
            })}
            onClick={() => onMove(item.id, false)}
          >
            <ArrowDown size={14} />
          </button>
        )}
        {(queued || active) && (
          <>
            <button
              type="button"
              className="button compact quiet"
              onClick={() => onCancel(item.id)}
            >
              <Ban size={14} />
              {t("install-queue-cancel-keep")}
            </button>
            <button
              type="button"
              className="button compact quiet"
              onClick={() => onDiscard(item.id)}
            >
              <Trash2 size={14} />
              {t("install-queue-cancel-discard")}
            </button>
          </>
        )}
        {(item.state === "failed" || item.state === "cancelled") && (
          <button
            type="button"
            className="button compact quiet"
            onClick={() => onRetry(item.id)}
          >
            <RotateCcw size={14} />
            {t("action-retry")}
          </button>
        )}
        {terminal && (
          <button
            type="button"
            className="icon-button compact"
            aria-label={t("install-queue-remove", { version: item.version })}
            onClick={() => onRemove(item.id)}
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>
    </li>
  );
}

function firstPendingIndex(items: InstallQueueItem[]) {
  return items.findIndex((item) => item.state === "queued");
}

function lastPendingIndex(items: InstallQueueItem[]) {
  let last = -1;
  items.forEach((item, index) => {
    if (item.state === "queued") last = index;
  });
  return last;
}
