import { useCallback } from "react";
import { tauriApi } from "../../api/tauri";
import type { EnvironmentDependencies } from "./dependencies";

export function useCoordinationRecovery(
  { t, runAction, requestConfirmation, notify }: EnvironmentDependencies,
  refreshStatus: () => Promise<void>,
) {
  const previewRepair = useCallback(
    () =>
      runAction("preview-coordination-permissions", async () => {
        const preview = await tauriApi.previewCoordinationPermissions();
        requestConfirmation({
          title: t("confirm-coordination-repair-title"),
          description: t("confirm-coordination-repair-description", {
            directory: preview.directory,
            files: preview.files.join(", "),
          }),
          confirmLabel: t("confirm-coordination-repair-action"),
          danger: true,
          action: () =>
            runAction("repair-coordination-permissions", async () => {
              await tauriApi.repairCoordinationPermissions(preview.id);
              notify("success", t("toast-coordination-repaired"));
              await refreshStatus();
            }),
        });
      }),
    [notify, refreshStatus, requestConfirmation, runAction, t],
  );
  return { previewRepair };
}
