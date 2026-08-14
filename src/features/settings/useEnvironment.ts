import { useCallback } from "react";
import { tauriApi } from "../../api/tauri";
import type {
  EnvironmentDiagnosticAction,
  EnvironmentDiagnosticId,
} from "../../domain/types";
import type { EnvironmentDependencies } from "./dependencies";
import { diagnosticTarget } from "./environmentDiagnostics";
import { useEnvironmentStatus } from "./useEnvironmentStatus";
import { useSettingsDraft } from "./useSettingsDraft";
import { useWinBoatControl } from "./useWinBoatControl";

export function useEnvironment(dependencies: EnvironmentDependencies) {
  const environmentStatus = useEnvironmentStatus(dependencies);
  const settings = useSettingsDraft({
    ...dependencies,
    environmentStatus: environmentStatus.status,
    refreshStatus: environmentStatus.refreshStatus,
  });
  const winBoat = useWinBoatControl({
    ...dependencies,
    status: environmentStatus.status,
    refreshStatus: environmentStatus.refreshStatus,
    applyConfig: settings.applyConfig,
    updateConfigPair: settings.updateConfigPair,
  });

  const runDiagnosticAction = useCallback(
    (
      action: EnvironmentDiagnosticAction,
      diagnosticId: EnvironmentDiagnosticId,
    ) => {
      if (action === "redetect") return settings.redetectSettings();
      if (action === "start-winboat") return winBoat.startWindows();
      if (action === "open-winboat") return winBoat.openWinBoat();

      const target = document.getElementById(diagnosticTarget(diagnosticId));
      target?.scrollIntoView?.({ behavior: "smooth", block: "center" });
      if (target instanceof HTMLElement) target.focus({ preventScroll: true });
      return Promise.resolve();
    },
    [settings, winBoat],
  );

  const copyDiagnosticReport = useCallback(
    () =>
      dependencies.runAction("copy-environment-report", async () => {
        const report = await tauriApi.getEnvironmentDiagnosticReport();
        if (!navigator.clipboard?.writeText) {
          throw new Error(dependencies.t("diagnostic-clipboard-unavailable"));
        }
        await navigator.clipboard.writeText(report);
        dependencies.notify(
          "success",
          dependencies.t("toast-diagnostic-report-copied"),
        );
      }),
    [dependencies],
  );

  const exportDiagnosticReport = useCallback(
    () =>
      dependencies.runAction("export-environment-report", async () => {
        const exported = await tauriApi.exportEnvironmentDiagnosticReport();
        if (exported) {
          dependencies.notify(
            "success",
            dependencies.t("toast-diagnostic-report-exported"),
          );
        }
      }),
    [dependencies],
  );

  return {
    config: settings.config,
    draftConfig: settings.draftConfig,
    setDraftConfig: settings.setDraftConfig,
    status: environmentStatus.status,
    online: winBoat.online,
    loading: settings.loading,
    applyMountNow: settings.applyMountNow,
    setApplyMountNow: settings.setApplyMountNow,
    settingsChanged: settings.settingsChanged,
    refreshStatus: environmentStatus.refreshStatus,
    choosePath: settings.choosePath,
    addStudioPath: settings.addStudioPath,
    removeStudioPath: settings.removeStudioPath,
    saveSettings: settings.saveSettings,
    redetectSettings: settings.redetectSettings,
    updateLanguagePreference: settings.updateLanguagePreference,
    winBoatControl: winBoat.winBoatControl,
    offlineGuidance: winBoat.offlineGuidance,
    setupCompletion: winBoat.setupCompletion,
    runDiagnosticAction,
    copyDiagnosticReport,
    exportDiagnosticReport,
  };
}

export type EnvironmentController = ReturnType<typeof useEnvironment>;
