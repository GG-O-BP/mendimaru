import { useCallback, useEffect, useRef, useState } from "react";
import { tauriApi } from "../../api/tauri";
import type { NvramRecoveryPreview } from "../../domain/types";
import type { EnvironmentDependencies } from "./dependencies";

export function useNvramRecovery(
  dependencies: EnvironmentDependencies,
  refreshStatus: () => Promise<void>,
) {
  const { t, notify, runAction, requestConfirmation } = dependencies;
  const [rollbackPreview, setRollbackPreview] =
    useState<NvramRecoveryPreview | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const confirm = useCallback(
    (preview: NvramRecoveryPreview, restore: boolean) => {
      requestConfirmation({
        title: t(
          restore ? "confirm-nvram-restore-title" : "confirm-nvram-title",
        ),
        description: t("confirm-nvram-description", {
          target: preview.targetPath,
          backup: preview.backupPath,
          original: preview.originalPath,
          bytes: String(preview.bytes),
          sha: preview.sha256,
        }),
        confirmLabel: t(
          restore ? "action-nvram-restore" : "action-nvram-recover",
        ),
        danger: true,
        action: () =>
          runAction("recover-winboat-nvram", async () => {
            const result = restore
              ? await tauriApi.restoreWinBoatNvram(preview.id)
              : await tauriApi.recoverWinBoatNvram(preview.id);
            if (!mounted.current) return;
            setRollbackPreview(result.rollbackRequired ? preview : null);
            notify(
              result.ready ? "success" : result.rolledBack ? "info" : "error",
              t(
                result.ready
                  ? "toast-nvram-ready"
                  : result.rolledBack
                    ? "toast-nvram-rolled-back"
                    : "toast-nvram-rollback-required",
              ),
            );
            await refreshStatus();
          }),
      });
    },
    [notify, refreshStatus, requestConfirmation, runAction, t],
  );

  const recoverNvram = useCallback(
    () =>
      runAction("preview-winboat-nvram", async () => {
        const preview = await tauriApi.previewWinBoatNvram();
        if (mounted.current) confirm(preview, false);
      }),
    [confirm, runAction],
  );

  return {
    recoverNvram,
    rollbackRequired: rollbackPreview !== null,
    restoreNvram: () => {
      if (rollbackPreview) confirm(rollbackPreview, true);
    },
  };
}
