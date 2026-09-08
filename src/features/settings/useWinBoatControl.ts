import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { tauriApi } from "../../api/tauri";
import { errorText } from "../../api/errors";
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
  setStartupPending: (pending: boolean) => void;
  startupPending: boolean;
  observedStartupFailure?: EnvironmentStatus["startup"];
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
  setStartupPending,
  startupPending,
  observedStartupFailure,
}: UseWinBoatControlOptions) {
  const mounted = useRef(false);
  const starting = useRef(false);
  const [startupFailure, setStartupFailure] = useState<string | null>(null);
  const [failureAfterId, setFailureAfterId] = useState(0);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const setupCompletionScheduled = useRef(false);
  const [setupCompletion, setSetupCompletion] =
    useState<SetupCompletion | null>(null);
  const setupPending = Boolean(status?.setupPending);
  const guestOnline = Boolean(status?.guestOnline);

  const startWindows = useCallback(async () => {
    if (starting.current || !mounted.current) return;
    starting.current = true;
    setStartupPending(true);
    try {
      await runAction("start-windows", async () => {
        notify("info", t("toast-windows-start-requested"));
        void refreshStatus({ sourceChanged: true });
        try {
          await tauriApi.startWinBoatWindows();
          if (!mounted.current) return;
          setStartupFailure(null);
          notify("success", t("toast-windows-ready"));
        } catch (error) {
          if (!mounted.current) return;
          setStartupFailure(errorText(error, t));
          setFailureAfterId(status?.startup?.id ?? 0);
          throw error;
        } finally {
          if (mounted.current) {
            setStartupPending(false);
            await refreshStatus({ sourceChanged: true });
          }
        }
      });
    } finally {
      starting.current = false;
      if (mounted.current) setStartupPending(false);
    }
  }, [
    notify,
    refreshStatus,
    runAction,
    setStartupPending,
    status?.startup?.id,
    t,
  ]);

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

  const {
    actionKey,
    actionLabel,
    controlKind,
    offlineGuidance,
    online,
    connectionLabel,
    lifecycle,
  } = useMemo(
    () =>
      deriveEnvironmentPresentation(
        status,
        t,
        (status?.startup?.phase === "online" &&
        status.startup.id > failureAfterId
          ? null
          : startupFailure) ??
          (observedStartupFailure
            ? t(
                observedStartupFailure.errorCode === "qemu_boot_timeout"
                  ? "diagnostic-qemu-boot-timeout"
                  : observedStartupFailure.errorCode === "guest_startup_timeout"
                    ? "windows-startup-timeout-detail"
                    : "windows-startup-failed-detail",
              )
            : null),
        startupPending,
      ),
    [
      status,
      startupFailure,
      failureAfterId,
      startupPending,
      observedStartupFailure,
      t,
    ],
  );

  const runPrimaryAction = useCallback(() => {
    if (controlKind === "setup") void beginWinBoatSetup();
    else if (controlKind === "open") void openWinBoat();
    else if (controlKind === "start") void startWindows();
  }, [beginWinBoatSetup, controlKind, openWinBoat, startWindows]);

  return {
    online,
    connectionLabel,
    lifecycle,
    startupFailure,
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
