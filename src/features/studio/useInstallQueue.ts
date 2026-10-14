import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { tauriApi } from "../../api/tauri";
import type { InstallQueueItem, InstallQueueState } from "../../domain/types";
import type { StudioInstallationDependencies } from "./dependencies";

const TERMINAL_QUEUE_STATES: ReadonlySet<InstallQueueState> = new Set([
  "succeeded",
  "failed",
  "cancelled",
]);

export function useInstallQueue({
  t,
  notify,
  runAction,
  onWarning,
  refreshInstalled,
}: StudioInstallationDependencies & {
  refreshInstalled?: () => Promise<unknown>;
}) {
  const [items, setItems] = useState<InstallQueueItem[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const succeededSeen = useRef<Set<string>>(new Set());

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let receivedChange = false;
    void (async () => {
      try {
        const stop = await tauriApi.onInstallQueueChanged((incoming) => {
          receivedChange = true;
          if (!disposed) {
            setItems(incoming);
            setLoadError(null);
          }
        });
        if (disposed) {
          stop();
          return;
        }
        unsubscribe = stop;
      } catch (error) {
        if (!disposed) onWarning(String(error));
      }
      if (disposed) return;
      try {
        const next = await tauriApi.getInstallQueue();
        if (!disposed && !receivedChange) {
          setItems(next);
          setLoadError(null);
        }
      } catch (error) {
        if (!disposed && !receivedChange) setLoadError(String(error));
      }
    })();
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [onWarning]);

  useEffect(() => {
    const nextSucceeded = items.filter(
      (item) =>
        item.state === "succeeded" && !succeededSeen.current.has(item.id),
    );
    if (nextSucceeded.length === 0) return;
    nextSucceeded.forEach((item) => succeededSeen.current.add(item.id));
    void refreshInstalled?.();
    nextSucceeded.forEach((item) =>
      notify("success", t("toast-install-complete", { version: item.version })),
    );
  }, [items, notify, refreshInstalled, t]);

  const cancelItem = useCallback(
    (itemId: string, keepPartial: boolean) =>
      runAction(`queue-cancel-${itemId}`, async () => {
        await tauriApi.cancelInstallQueueItem(itemId, keepPartial);
        if (keepPartial) {
          notify("info", t("toast-queue-cancel-requested"));
        } else {
          notify("info", t("toast-queue-discard-requested"));
        }
      }),
    [notify, runAction, t],
  );

  const retryItem = useCallback(
    (itemId: string) =>
      runAction(`queue-retry-${itemId}`, async () => {
        await tauriApi.retryInstallQueueItem(itemId);
        notify("success", t("toast-queue-retry-requested"));
      }),
    [notify, runAction, t],
  );

  const moveItem = useCallback(
    (itemId: string, up: boolean) =>
      runAction(`queue-move-${itemId}`, async () => {
        await tauriApi.moveInstallQueueItem(itemId, up);
      }),
    [runAction],
  );

  const removeItem = useCallback(
    (itemId: string) =>
      runAction(`queue-remove-${itemId}`, async () => {
        await tauriApi.removeInstallQueueItem(itemId);
      }),
    [runAction],
  );

  const activeVersions = useMemo(
    () =>
      new Set(
        items
          .filter((item) => !TERMINAL_QUEUE_STATES.has(item.state))
          .map((item) => item.version),
      ),
    [items],
  );

  return {
    items,
    loadError,
    activeVersions,
    cancelItem,
    retryItem,
    moveItem,
    removeItem,
  };
}
