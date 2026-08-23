import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { StudioSessionStatus, StudioVersion } from "../../domain/types";
import type { InstalledVersionsDependencies } from "./dependencies";

const ACTIVE_SESSION_REFRESH_INTERVAL_MS = 1_000;

function sameStudioPaths(left: StudioVersion[], right: StudioVersion[]) {
  if (left.length !== right.length) return false;
  const rightByVersion = new Map(
    right.map((version) => [
      version.version,
      version.executablePath.toLowerCase(),
    ]),
  );
  return left.every(
    (version) =>
      rightByVersion.get(version.version) ===
      version.executablePath.toLowerCase(),
  );
}

export function useInstalledVersions({
  t,
  installedVersionsSourceKey,
  notify,
  requestConfirmation,
  runAction,
  hasBusyPrefix,
  onWarning,
}: InstalledVersionsDependencies) {
  const [installedVersions, setInstalledVersions] = useState<StudioVersion[]>(
    [],
  );
  const [installedLoading, setInstalledLoading] = useState(true);
  const [installedLoaded, setInstalledLoaded] = useState(false);
  const [installedStale, setInstalledStale] = useState(false);
  const [installedError, setInstalledError] = useState<string | null>(null);
  const [displayedInstalledSource, setDisplayedInstalledSource] = useState(
    installedVersionsSourceKey,
  );
  const [verifiedInstalledSource, setVerifiedInstalledSource] = useState<
    string | null
  >(null);
  const [installedErrorSource, setInstalledErrorSource] = useState<
    string | null
  >(null);
  const [sessions, setSessions] = useState<StudioSessionStatus[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [displayedSessionsSource, setDisplayedSessionsSource] = useState<
    string | null
  >(null);
  const installedRequest = useRef(0);
  const sessionRequest = useRef(0);
  const sessionRefreshSourceInFlight = useRef<string | null>(null);
  const launchLock = useRef(false);

  const refreshSessions = useCallback(
    async (silent = false) => {
      if (sessionRefreshSourceInFlight.current === installedVersionsSourceKey) {
        return;
      }
      sessionRefreshSourceInFlight.current = installedVersionsSourceKey;
      const request = ++sessionRequest.current;
      setSessionsLoading(true);
      try {
        const next = await tauriApi.getStudioSessions();
        if (request === sessionRequest.current) {
          setDisplayedSessionsSource(installedVersionsSourceKey);
          setSessions(next);
        }
      } catch (error) {
        if (request === sessionRequest.current) {
          setDisplayedSessionsSource(installedVersionsSourceKey);
          setSessions([]);
        }
        if (!silent) onWarning(errorText(error, t));
      } finally {
        if (request === sessionRequest.current) setSessionsLoading(false);
        if (
          sessionRefreshSourceInFlight.current === installedVersionsSourceKey
        ) {
          sessionRefreshSourceInFlight.current = null;
        }
      }
    },
    [installedVersionsSourceKey, onWarning, t],
  );

  const refreshInstalled = useCallback(
    async (silent = false) => {
      const request = ++installedRequest.current;
      setInstalledLoading(true);
      setInstalledLoaded(false);
      setInstalledStale(true);
      setInstalledError(null);
      let next: StudioVersion[] | undefined;
      try {
        next = await tauriApi.getInstalledVersions();
        if (request === installedRequest.current) {
          setDisplayedInstalledSource(installedVersionsSourceKey);
          setVerifiedInstalledSource(installedVersionsSourceKey);
          setInstalledErrorSource(null);
          setInstalledVersions(next);
          setInstalledLoaded(true);
          setInstalledStale(false);
        }
      } catch (error) {
        if (request === installedRequest.current) {
          const message = errorText(error, t);
          setInstalledErrorSource(installedVersionsSourceKey);
          setInstalledError(message);
          if (!silent) onWarning(message);
        }
      } finally {
        if (request === installedRequest.current) setInstalledLoading(false);
      }
      return request === installedRequest.current ? next : undefined;
    },
    [installedVersionsSourceKey, onWarning, t],
  );

  useEffect(() => {
    let active = true;
    const initialize = async () => {
      let cachedVersions: StudioVersion[] = [];
      try {
        const cached = await tauriApi.getInstalledVersionsCache();
        if (!active) return;
        cachedVersions = cached.versions;
        if (cachedVersions.length > 0) {
          setDisplayedInstalledSource(installedVersionsSourceKey);
          setInstalledVersions(cachedVersions);
          setInstalledStale(true);
        }
      } catch {
        // A missing or invalid cache is equivalent to a cold start.
      }
      if (!active) return;

      const installed = refreshInstalled(true);
      if (cachedVersions.length === 0) {
        const next = await installed;
        if (active && next !== undefined) await refreshSessions(true);
        return;
      }

      const sessions = refreshSessions(true);
      const next = await installed;
      if (
        !active ||
        next === undefined ||
        sameStudioPaths(cachedVersions, next)
      ) {
        return;
      }
      await sessions;
      if (active) await refreshSessions(true);
    };
    void initialize();
    return () => {
      active = false;
      installedRequest.current += 1;
    };
  }, [installedVersionsSourceKey, refreshInstalled, refreshSessions]);

  useEffect(() => {
    if (
      !sessions.some((session) => session.connection === "connected") ||
      displayedSessionsSource !== installedVersionsSourceKey
    ) {
      return;
    }
    const interval = window.setInterval(() => {
      void refreshSessions(true);
    }, ACTIVE_SESSION_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [
    displayedSessionsSource,
    installedVersionsSourceKey,
    refreshSessions,
    sessions,
  ]);

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
  const installedSourceMatches =
    displayedInstalledSource === installedVersionsSourceKey;
  const installedSourceVerified =
    verifiedInstalledSource === installedVersionsSourceKey;
  const sessionsSourceMatches =
    displayedSessionsSource === installedVersionsSourceKey;

  return {
    installedVersions: installedSourceMatches ? installedVersions : [],
    installedSet: installedSourceMatches ? installedSet : new Set<string>(),
    installedLoading:
      installedErrorSource !== installedVersionsSourceKey &&
      (!installedSourceVerified || installedLoading),
    installedLoaded: installedSourceVerified && installedLoaded,
    launchReady: installedSourceMatches && installedVersions.length > 0,
    installedStale: installedSourceMatches && installedStale,
    installedError:
      installedErrorSource === installedVersionsSourceKey
        ? installedError
        : null,
    sessions: sessionsSourceMatches ? sessions : [],
    sessionsLoading: !sessionsSourceMatches || sessionsLoading,
    isLaunching: hasBusyPrefix("launch-"),
    refreshInstalled,
    refreshSessions,
    launchVersion,
    reconnectSession,
    askStopSession,
    askUninstall,
  };
}
