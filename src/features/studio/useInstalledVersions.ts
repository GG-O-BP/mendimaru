import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { StudioSessionStatus, StudioVersion } from "../../domain/types";
import type { InstalledVersionsDependencies } from "./dependencies";

export function useInstalledVersions({
  t,
  notify,
  requestConfirmation,
  runAction,
  hasBusyPrefix,
  onWarning,
}: InstalledVersionsDependencies) {
  const [installedVersions, setInstalledVersions] = useState<StudioVersion[]>(
    [],
  );
  const [sessions, setSessions] = useState<StudioSessionStatus[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const sessionRequest = useRef(0);
  const launchLock = useRef(false);

  const refreshSessions = useCallback(
    async (silent = false) => {
      const request = ++sessionRequest.current;
      setSessionsLoading(true);
      try {
        const next = await tauriApi.getStudioSessions();
        if (request === sessionRequest.current) setSessions(next);
      } catch (error) {
        if (!silent) onWarning(errorText(error, t));
      } finally {
        if (request === sessionRequest.current) setSessionsLoading(false);
      }
    },
    [onWarning, t],
  );

  const refreshInstalled = useCallback(
    async (silent = false) => {
      let next: StudioVersion[] | undefined;
      try {
        next = await tauriApi.getInstalledVersions();
        setInstalledVersions(next);
      } catch (error) {
        if (!silent) onWarning(errorText(error, t));
      }
      await refreshSessions(silent);
      return next;
    },
    [onWarning, refreshSessions, t],
  );

  useEffect(() => {
    const initialRefresh = window.setTimeout(
      () => void refreshInstalled(true),
      0,
    );
    return () => window.clearTimeout(initialRefresh);
  }, [refreshInstalled]);

  const launchVersion = useCallback(
    (
      version: StudioVersion,
      projectMprPath?: string,
      projectName?: string,
      afterLaunch?: () => Promise<void>,
    ) => {
      if (launchLock.current) return Promise.resolve();
      launchLock.current = true;
      return runAction(`launch-${version.version}`, async () => {
        try {
          await tauriApi.launchStudioPro(version.version, projectMprPath);
          await afterLaunch?.();
        } finally {
          await refreshSessions(true);
        }
        notify(
          "success",
          t("toast-studio-opened", { version: version.version }),
          projectName
            ? t("toast-project-opened", { project: projectName })
            : undefined,
        );
      }).finally(() => {
        launchLock.current = false;
      });
    },
    [notify, refreshSessions, runAction, t],
  );

  const reconnectSession = useCallback(
    (session: StudioSessionStatus) =>
      runAction(`reconnect-${session.sessionId}`, async () => {
        try {
          await tauriApi.reconnectStudioSession(session.sessionId);
        } finally {
          await refreshSessions(true);
        }
        notify(
          "success",
          t("toast-session-reconnected", { version: session.version }),
        );
      }),
    [notify, refreshSessions, runAction, t],
  );

  const askStopSession = useCallback(
    (session: StudioSessionStatus) => {
      requestConfirmation({
        title: t("confirm-stop-session-title", { version: session.version }),
        description: t("confirm-stop-session-description"),
        confirmLabel: t("action-stop-session"),
        danger: true,
        action: () =>
          runAction(`stop-${session.sessionId}`, async () => {
            try {
              await tauriApi.stopStudioSession(session.sessionId);
            } finally {
              await refreshSessions(true);
            }
            notify(
              "success",
              t("toast-session-stopped", { version: session.version }),
            );
          }),
      });
    },
    [notify, refreshSessions, requestConfirmation, runAction, t],
  );

  const askUninstall = useCallback(
    (version: StudioVersion) => {
      requestConfirmation({
        title: t("confirm-uninstall-title", { version: version.version }),
        description: t("confirm-uninstall-description"),
        confirmLabel: t("action-uninstall"),
        danger: true,
        action: () =>
          runAction(`uninstall-${version.version}`, async () => {
            await tauriApi.uninstallStudioPro(version.version);
            setInstalledVersions((current) =>
              current.filter(
                (installed) => installed.version !== version.version,
              ),
            );
            await refreshInstalled();
            notify(
              "success",
              t("toast-uninstall-complete", { version: version.version }),
            );
          }),
      });
    },
    [notify, refreshInstalled, requestConfirmation, runAction, t],
  );

  const installedSet = useMemo(
    () => new Set(installedVersions.map((version) => version.version)),
    [installedVersions],
  );

  return {
    installedVersions,
    installedSet,
    sessions,
    sessionsLoading,
    isLaunching: hasBusyPrefix("launch-"),
    refreshInstalled,
    refreshSessions,
    launchVersion,
    reconnectSession,
    askStopSession,
    askUninstall,
  };
}
