import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type {
  ConfirmationState,
  OperationRecord,
  ToastKind,
} from "../../domain/types";
import type { Translate } from "../../i18n";

const ACTIVE_REFRESH_INTERVAL_MS = 1_500;
const IDLE_REFRESH_INTERVAL_MS = 15_000;

export function useOperations({
  t,
  notify,
  requestConfirmation,
  runAction,
  isBusy,
  onWarning,
}: {
  t: Translate;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
  runAction: (key: string, action: () => Promise<void>) => Promise<void>;
  isBusy: (key: string) => boolean;
  onWarning: (message: string | null) => void;
}) {
  const [operations, setOperations] = useState<OperationRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const refreshSequence = useRef(0);
  const hasRunningOperations = operations.some(
    (operation) => operation.state === "running",
  );

  const refresh = useCallback(
    async (silent = false) => {
      const sequence = ++refreshSequence.current;
      if (!silent) setLoading(true);
      try {
        const next = await tauriApi.getOperations();
        if (sequence === refreshSequence.current) setOperations(next);
      } catch (error) {
        if (!silent && sequence === refreshSequence.current) {
          onWarning(errorText(error, t));
        }
      } finally {
        if (sequence === refreshSequence.current) setLoading(false);
      }
    },
    [onWarning, t],
  );

  useEffect(() => {
    const initialRefresh = window.setTimeout(() => void refresh(true), 0);
    const interval = window.setInterval(
      () => void refresh(true),
      hasRunningOperations
        ? ACTIVE_REFRESH_INTERVAL_MS
        : IDLE_REFRESH_INTERVAL_MS,
    );
    return () => {
      refreshSequence.current += 1;
      window.clearTimeout(initialRefresh);
      window.clearInterval(interval);
    };
  }, [hasRunningOperations, refresh]);

  const retry = useCallback(
    (operation: OperationRecord) => {
      if (!operation.retryable || !operation.finishedAt) return;
      void runAction(`retry-operation-${operation.id}`, async () => {
        await tauriApi.retryOperation(operation.id);
        await refresh();
        notify("success", t("toast-operation-retried"));
      });
    },
    [notify, refresh, runAction, t],
  );

  const clear = useCallback(() => {
    requestConfirmation({
      title: t("confirm-clear-operation-history-title"),
      description: t("confirm-clear-operation-history-description"),
      confirmLabel: t("action-clear-operation-history"),
      action: () =>
        runAction("clear-operation-history", async () => {
          const removed = await tauriApi.clearOperationHistory();
          await refresh();
          notify("success", t("toast-operation-history-cleared", { removed }));
        }),
    });
  }, [notify, refresh, requestConfirmation, runAction, t]);

  const openLogs = useCallback(
    () =>
      runAction("open-operation-logs", async () => {
        await tauriApi.openOperationLogs();
      }),
    [runAction],
  );

  return {
    operations,
    loading,
    refresh,
    retry,
    clear,
    openLogs,
    isBusy,
  };
}
