import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { tauriApi } from "../../api/tauri";
import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { WinBoatControlDependencies } from "./dependencies";
import { deriveEnvironmentPresentation } from "./environmentState";
import type { EnvironmentRefreshOptions } from "./useEnvironmentStatus";

export interface SetupCompletion {
  sequence: number;
  containerRecreated: boolean;
}

interface UseWinBoatControlOptions extends WinBoatControlDependencies {
  status: EnvironmentStatus | null;
  refreshStatus: (options?: EnvironmentRefreshOptions) => Promise<void>;
  applyConfig: (config: AppConfig) => void;
  updateConfigPair: (update: (config: AppConfig) => AppConfig) => void;
}

export function useWinBoatControl({
  t,
  notify,
  runAction,
  isBusy,
  status,
  refreshStatus,
  applyConfig,
  updateConfigPair,
}: UseWinBoatControlOptions) {
  const setupCompletionScheduled = useRef(false);
  const [setupCompletion, setSetupCompletion] =
    useState<SetupCompletion | null>(null);
  const setupPending = Boolean(status?.setupPending);
  const guestOnline = Boolean(status?.guestOnline);

  const startWindows = useCallback(
    () =>
      runAction("start-windows", async () => {
        await tauriApi.startWinBoatWindows();
        notify(
          "info",
          t("toast-windows-started"),
          t("toast-windows-started-detail"),
        );
        window.setTimeout(() => void refreshStatus(), 3_500);
      }),
    [notify, refreshStatus, runAction, t],
  );

  const openWinBoat = useCallback(
    () => runAction("open-winboat", () => tauriApi.openWinBoat()),
    [runAction],
  );

  const beginWinBoatSetup = useCallback(
    () =>
      runAction("setup-winboat", async () => {
        await tauriApi.beginWinBoatSetup();
        updateConfigPair((config) => ({
          ...config,
          winboatSetupPending: true,
        }));
        notify(
          "info",
          t("toast-winboat-setup-opened"),
          t("toast-winboat-setup-opened-detail"),
        );
        await refreshStatus({ sourceChanged: true });
      }),
    [notify, refreshStatus, runAction, t, updateConfigPair],
  );

  useEffect(() => {
    if (!setupPending || !guestOnline) {
      if (!setupPending) setupCompletionScheduled.current = false;
      return undefined;
    }
    if (setupCompletionScheduled.current) return undefined;
    setupCompletionScheduled.current = true;

    const timeout = window.setTimeout(() => {
      void runAction("complete-winboat-setup", async () => {
        const result = await tauriApi.completeWinBoatSetup();
        applyConfig(result.config);
        setSetupCompletion((current) => ({
          sequence: (current?.sequence ?? 0) + 1,
          containerRecreated: result.containerRecreated,
        }));
        notify(
          "success",
          t("toast-winboat-setup-complete"),
          result.containerRecreated
            ? t("toast-winboat-setup-complete-reconnected")
            : undefined,
        );
        await refreshStatus({ sourceChanged: true });
      }).finally(() => {
        setupCompletionScheduled.current = false;
      });
    }, 5_000);

    return () => {
      window.clearTimeout(timeout);
      setupCompletionScheduled.current = false;
    };
  }, [
    applyConfig,
    guestOnline,
    notify,
    refreshStatus,
    runAction,
    setupPending,
    t,
  ]);

  const { actionKey, actionLabel, controlKind, offlineGuidance, online } =
    useMemo(() => deriveEnvironmentPresentation(status, t), [status, t]);

  const runPrimaryAction = useCallback(() => {
    if (controlKind === "setup") void beginWinBoatSetup();
    else if (controlKind === "open") void openWinBoat();
    else if (controlKind === "start") void startWindows();
  }, [beginWinBoatSetup, controlKind, openWinBoat, startWindows]);

  return {
    online,
    offlineGuidance,
    setupCompletion,
    startWindows,
    openWinBoat,
    winBoatControl: {
      kind: controlKind,
      key: actionKey,
      label: actionLabel,
      busy: isBusy(actionKey),
      onAction: runPrimaryAction,
    },
  };
}
