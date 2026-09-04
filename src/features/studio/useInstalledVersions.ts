import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type {
  InstalledVersionsCache,
  StudioSessionStatus,
  StudioVersion,
} from "../../domain/types";
import type { InstalledVersionsDependencies } from "./dependencies";
import { traceStudioOverview } from "./overviewTrace";

const ACTIVE_SESSION_REFRESH_INTERVAL_MS = 1_000;

interface SourceRequest<T> {
  sourceKey: string;
  promise: Promise<T>;
}

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
  const cacheRequestInFlight = useRef<
    SourceRequest<InstalledVersionsCache> | undefined
  >(undefined);
  const installedRefreshInFlight = useRef<
    SourceRequest<StudioVersion[] | undefined> | undefined
  >(undefined);
  const sessionRefreshInFlight = useRef<
    SourceRequest<StudioSessionStatus[] | undefined> | undefined
  >(undefined);
  const overviewRefreshInFlight = useRef<SourceRequest<void> | undefined>(
    undefined,
  );
  const installedVersionsRef = useRef(installedVersions);
  const displayedInstalledSourceRef = useRef(displayedInstalledSource);
  const launchLock = useRef(false);

  useEffect(() => {
    installedVersionsRef.current = installedVersions;
    displayedInstalledSourceRef.current = displayedInstalledSource;
  }, [displayedInstalledSource, installedVersions]);

  const loadInstalledCache = useCallback(() => {
    const current = cacheRequestInFlight.current;
    if (current?.sourceKey === installedVersionsSourceKey) {
      return current.promise;
    }
    traceStudioOverview("cache-request-start");
    const startedAt = performance.now();
    const request: SourceRequest<InstalledVersionsCache> = {
      sourceKey: installedVersionsSourceKey,
      promise: tauriApi.getInstalledVersionsCache().finally(() => {
        traceStudioOverview("cache-request-end", {
          durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
        });
      }),
    };
    cacheRequestInFlight.current = request;
    return request.promise;
  }, [installedVersionsSourceKey]);

  const refreshSessions = useCallback(
    (silent = false) => {
      const current = sessionRefreshInFlight.current;
      if (current?.sourceKey === installedVersionsSourceKey) {
        return current.promise;
      }
      const request = ++sessionRequest.current;
      setSessionsLoading(true);
      traceStudioOverview("session-request-start");
      const startedAt = performance.now();
      const promise = (async () => {
        try {
          const next = await tauriApi.getStudioSessions();
          if (request === sessionRequest.current) {
            setDisplayedSessionsSource(installedVersionsSourceKey);
            setSessions(next);
          }
          traceStudioOverview("session-request-end", {
            durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
            sessionCount: next.length,
          });
          return request === sessionRequest.current ? next : undefined;
        } catch (error) {
          if (request === sessionRequest.current) {
            setDisplayedSessionsSource(installedVersionsSourceKey);
            setSessions([]);
          }
          traceStudioOverview("session-request-error", {
            durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
          });
          if (!silent) onWarning(errorText(error, t));
          return undefined;
        } finally {
          if (request === sessionRequest.current) setSessionsLoading(false);
        }
      })();
      const inFlight = {
        sourceKey: installedVersionsSourceKey,
        promise,
      };
      sessionRefreshInFlight.current = inFlight;
      void promise.finally(() => {
        if (sessionRefreshInFlight.current === inFlight) {
          sessionRefreshInFlight.current = undefined;
        }
      });
      return promise;
    },
    [installedVersionsSourceKey, onWarning, t],
  );

  const refreshInstalled = useCallback(
    (silent = false) => {
      const current = installedRefreshInFlight.current;
      if (current?.sourceKey === installedVersionsSourceKey) {
        return current.promise;
      }
      const request = ++installedRequest.current;
      setInstalledLoading(true);
      setInstalledLoaded(false);
      setInstalledStale(true);
      setInstalledError(null);
      traceStudioOverview("installed-request-start");
      const startedAt = performance.now();
      const promise = (async () => {
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
          traceStudioOverview("installed-request-end", {
            durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
            versionCount: next.length,
          });
        } catch (error) {
          if (request === installedRequest.current) {
            const message = errorText(error, t);
            setInstalledErrorSource(installedVersionsSourceKey);
            setInstalledError(message);
            if (!silent) onWarning(message);
          }
          traceStudioOverview("installed-request-error", {
            durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
          });
        } finally {
          if (request === installedRequest.current) setInstalledLoading(false);
        }
        return request === installedRequest.current ? next : undefined;
      })();
      const inFlight = {
        sourceKey: installedVersionsSourceKey,
        promise,
      };
      installedRefreshInFlight.current = inFlight;
      void promise.finally(() => {
        if (installedRefreshInFlight.current === inFlight) {
          installedRefreshInFlight.current = undefined;
        }
      });
      return promise;
    },
    [installedVersionsSourceKey, onWarning, t],
  );

  const refreshOverview = useCallback(
    (silent = false, baseline?: StudioVersion[]) => {
      const current = overviewRefreshInFlight.current;
      if (current?.sourceKey === installedVersionsSourceKey) {
        return current.promise;
      }
      const knownVersions =
        baseline ??
        (displayedInstalledSourceRef.current === installedVersionsSourceKey
          ? installedVersionsRef.current
          : []);
      const promise = (async () => {
        const installed = refreshInstalled(silent);
        const initialSessions =
          knownVersions.length > 0 ? refreshSessions(silent) : undefined;
        const next = await installed;
        if (next === undefined) {
          await initialSessions;
          return;
        }
        if (!initialSessions) {
          await refreshSessions(silent);
        } else {
          await initialSessions;
          if (!sameStudioPaths(knownVersions, next)) {
            await refreshSessions(silent);
          }
        }
        traceStudioOverview("overview-ready", {
          versionCount: next.length,
        });
      })();
      const inFlight = {
        sourceKey: installedVersionsSourceKey,
        promise,
      };
      overviewRefreshInFlight.current = inFlight;
      void promise.finally(() => {
        if (overviewRefreshInFlight.current === inFlight) {
          overviewRefreshInFlight.current = undefined;
        }
      });
      return promise;
    },
    [installedVersionsSourceKey, refreshInstalled, refreshSessions],
  );
  const refreshOverviewRef = useRef(refreshOverview);
  useEffect(() => {
    refreshOverviewRef.current = refreshOverview;
  }, [refreshOverview]);

  useEffect(() => {
    let active = true;
    const initialize = async () => {
      let cachedVersions: StudioVersion[] = [];
      try {
        const cached = await loadInstalledCache();
        if (!active) return;
        cachedVersions = cached.versions;
        if (cachedVersions.length > 0) {
          setDisplayedInstalledSource(installedVersionsSourceKey);
          setInstalledVersions(cachedVersions);
          setInstalledStale(true);
          traceStudioOverview("cache-render", {
            versionCount: cachedVersions.length,
          });
        }
      } catch {
        // A missing or invalid cache is equivalent to a cold start.
      }
      if (!active) return;
      await refreshOverviewRef.current(true, cachedVersions);
    };
    void initialize();
    return () => {
      active = false;
    };
  }, [installedVersionsSourceKey, loadInstalledCache]);

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
      const refreshSessionsAfterLaunch =
        !window.__mendimaruSkipPostLaunchSessionRefresh__;
      return runAction(`launch-${version.version}`, async () => {
        try {
          await tauriApi.launchStudioPro(version.version, projectMprPath);
          await afterLaunch?.();
        } finally {
          if (refreshSessionsAfterLaunch) {
            await refreshSessions(true);
          }
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
  const currentInstalledLoading =
    installedErrorSource !== installedVersionsSourceKey &&
    (!installedSourceVerified || installedLoading);

  return {
    installedVersions: installedSourceMatches ? installedVersions : [],
    installedSet: installedSourceMatches ? installedSet : new Set<string>(),
    installedLoading: currentInstalledLoading,
    installedLoaded: installedSourceVerified && installedLoaded,
    launchReady: installedSourceMatches && installedVersions.length > 0,
    installedStale: installedSourceMatches && installedStale,
    installedError:
      installedErrorSource === installedVersionsSourceKey
        ? installedError
        : null,
    sessions: sessionsSourceMatches ? sessions : [],
    sessionsLoading: installedSourceVerified
      ? !sessionsSourceMatches || sessionsLoading
      : currentInstalledLoading || sessionsLoading,
    isLaunching: hasBusyPrefix("launch-"),
    refreshInstalled,
    refreshOverview,
    refreshSessions,
    launchVersion,
    reconnectSession,
    askStopSession,
    askUninstall,
  };
}
