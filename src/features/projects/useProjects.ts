import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type {
  MendixProject,
  ProjectScanResult,
  ProjectSortKey,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import { useTauriSubscription } from "../../shared/hooks/useTauriSubscription";

const PROJECT_PAGE_SIZE = 100;
const WATCHER_DEBOUNCE_MS = 500;
const WATCHER_FALLBACK_MS = 30_000;
const WATCHER_SAFETY_INTERVAL_MS = 5 * 60_000;

type ProjectsStatus = "loading" | "stale" | "ready" | "error";

interface ProjectsState {
  sourceKey: string;
  status: ProjectsStatus;
  projects: MendixProject[];
  scan?: ProjectScanResult;
  error?: string;
}

interface SourceRequest {
  sourceKey: string;
  promise: Promise<MendixProject[] | undefined>;
}

interface UseProjectsOptions {
  t: Translate;
  sharedDirectory: string;
  onWarning: (message: string | null) => void;
  runAction: (key: string, action: () => Promise<void>) => Promise<void>;
}

export function useProjects({
  t,
  sharedDirectory,
  onWarning,
  runAction,
}: UseProjectsOptions) {
  const sourceKey = normalizeProjectSourceKey(sharedDirectory);
  const [state, setState] = useState<ProjectsState>(() => ({
    sourceKey,
    status: "loading",
    projects: [],
  }));
  const [search, setSearch] = useState("");
  const [favoriteOnly, setFavoriteOnly] = useState(false);
  const [sortKey, setSortKey] = useState<ProjectSortKey>("modified");
  const [visibleCount, setVisibleCount] = useState(PROJECT_PAGE_SIZE);
  const [externalSelectionBusy, setExternalSelectionBusy] = useState(false);
  const externalSelectionLock = useRef(false);
  const sourceRef = useRef(sourceKey);
  const sourceEffectRef = useRef<string | undefined>(undefined);
  const requestRef = useRef(0);
  const inFlightRef = useRef<SourceRequest | undefined>(undefined);
  const queuedWatcherRefreshRef = useRef(false);
  const watcherTimerRef = useRef<number | undefined>(undefined);
  const refreshRef = useRef<() => Promise<MendixProject[] | undefined>>(
    async () => undefined,
  );
  const warningRef = useRef(onWarning);
  const translateRef = useRef(t);

  useEffect(() => {
    warningRef.current = onWarning;
    translateRef.current = t;
  }, [onWarning, t]);

  const refresh = useCallback(async (): Promise<
    MendixProject[] | undefined
  > => {
    const activeSourceKey = sourceRef.current;
    const current = inFlightRef.current;
    if (current?.sourceKey === activeSourceKey) {
      return current.promise;
    }
    queuedWatcherRefreshRef.current = false;

    const request = ++requestRef.current;
    setState((current) =>
      current.sourceKey === activeSourceKey &&
      (current.status === "ready" || current.status === "stale")
        ? current
        : {
            ...current,
            sourceKey: activeSourceKey,
            status: "loading",
          },
    );
    const promise = (async () => {
      try {
        const result = await tauriApi.getProjects();
        if (
          request !== requestRef.current ||
          activeSourceKey !== sourceRef.current
        ) {
          return undefined;
        }
        setState({
          sourceKey: activeSourceKey,
          status: "ready",
          projects: result.projects,
          scan: result,
        });
        setVisibleCount(PROJECT_PAGE_SIZE);
        return result.projects;
      } catch (error: unknown) {
        if (
          request !== requestRef.current ||
          activeSourceKey !== sourceRef.current
        ) {
          return undefined;
        }
        const message = errorText(error, translateRef.current);
        setState((current) => ({
          sourceKey: activeSourceKey,
          status: "error",
          projects:
            current.sourceKey === activeSourceKey ? current.projects : [],
          scan:
            current.sourceKey === activeSourceKey ? current.scan : undefined,
          error: message,
        }));
        warningRef.current(message);
        return undefined;
      } finally {
        if (inFlightRef.current?.sourceKey === activeSourceKey) {
          inFlightRef.current = undefined;
        }
        if (queuedWatcherRefreshRef.current) {
          queuedWatcherRefreshRef.current = false;
          void refreshRef.current();
        }
      }
    })();
    inFlightRef.current = { sourceKey: activeSourceKey, promise };
    return promise;
  }, []);

  useEffect(() => {
    refreshRef.current = refresh;
  }, [refresh]);

  useEffect(() => {
    if (sourceEffectRef.current === sourceKey) return;
    sourceEffectRef.current = sourceKey;
    sourceRef.current = sourceKey;
    requestRef.current += 1;
    inFlightRef.current = undefined;
    setState((current) => ({
      sourceKey,
      status: "stale",
      projects: current.projects,
      scan: current.scan,
    }));
    void refresh();
  }, [refresh, sourceKey]);

  const scheduleWatcherRefresh = useCallback(() => {
    window.clearTimeout(watcherTimerRef.current);
    watcherTimerRef.current = window.setTimeout(() => {
      watcherTimerRef.current = undefined;
      if (inFlightRef.current?.sourceKey === sourceRef.current) {
        queuedWatcherRefreshRef.current = true;
        return;
      }
      void refreshRef.current();
    }, WATCHER_DEBOUNCE_MS);
  }, []);

  const subscribeToWorkspaceEvents = useCallback(
    () => tauriApi.onWorkspaceProjectsChanged(() => scheduleWatcherRefresh()),
    [scheduleWatcherRefresh],
  );

  useTauriSubscription(subscribeToWorkspaceEvents);

  useEffect(
    () => () => {
      window.clearTimeout(watcherTimerRef.current);
    },
    [],
  );

  const fallbackInterval = state.scan?.watcherActive
    ? WATCHER_SAFETY_INTERVAL_MS
    : WATCHER_FALLBACK_MS;
  useEffect(() => {
    if (state.status === "loading" || state.status === "error")
      return undefined;
    const timer = window.setInterval(() => {
      queuedWatcherRefreshRef.current = true;
      void refreshRef.current();
    }, fallbackInterval);
    return () => window.clearInterval(timer);
  }, [fallbackInterval, state.status]);

  const filteredProjects = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return projectsSort(
      projectsFilter(projectsSearch(state.projects, needle), favoriteOnly),
      sortKey,
    );
  }, [favoriteOnly, state.projects, search, sortKey]);

  const changeSearch = useCallback((value: string) => {
    setSearch(value);
    setVisibleCount(PROJECT_PAGE_SIZE);
  }, []);

  const changeFavoriteOnly = useCallback((value: boolean) => {
    setFavoriteOnly(value);
    setVisibleCount(PROJECT_PAGE_SIZE);
  }, []);

  const changeSortKey = useCallback((value: ProjectSortKey) => {
    setSortKey(value);
    setVisibleCount(PROJECT_PAGE_SIZE);
  }, []);

  const openFolder = useCallback(
    (path: string) =>
      runAction(`folder-${path}`, () => tauriApi.openFolder(path)),
    [runAction],
  );

  const selectExternalProject = useCallback(async () => {
    if (externalSelectionLock.current) return null;
    externalSelectionLock.current = true;
    setExternalSelectionBusy(true);
    try {
      return await tauriApi.selectExternalProject();
    } catch (error: unknown) {
      warningRef.current(errorText(error, translateRef.current));
      return null;
    } finally {
      externalSelectionLock.current = false;
      setExternalSelectionBusy(false);
    }
  }, []);

  const toggleFavorite = useCallback(async (project: MendixProject) => {
    if (project.location === "explicit-host-selection") return;
    const favorite = !project.favorite;
    setState((current) => ({
      ...current,
      projects: current.projects.map((candidate) =>
        candidate.mprPath === project.mprPath
          ? { ...candidate, favorite }
          : candidate,
      ),
    }));
    try {
      await tauriApi.setProjectFavorite(project.mprPath, favorite);
    } catch (error: unknown) {
      setState((current) => ({
        ...current,
        projects: current.projects.map((candidate) =>
          candidate.mprPath === project.mprPath
            ? { ...candidate, favorite: !favorite }
            : candidate,
        ),
      }));
      warningRef.current(errorText(error, translateRef.current));
    }
  }, []);

  const markProjectLaunched = useCallback((project: MendixProject) => {
    const launchedAt = new Date().toISOString();
    setState((current) => ({
      ...current,
      projects: current.projects.map((candidate) =>
        candidate.mprPath === project.mprPath
          ? {
              ...candidate,
              launchPending: false,
              lastLaunchedAt: launchedAt,
            }
          : candidate,
      ),
    }));
  }, []);

  return {
    projects: state.projects,
    filteredProjects: filteredProjects.slice(0, visibleCount),
    totalVisibleProjects: filteredProjects.length,
    hasMoreProjects: filteredProjects.length > visibleCount,
    scanStatus: state.status,
    scanError: state.error,
    scan: state.scan,
    search,
    setSearch: changeSearch,
    favoriteOnly,
    setFavoriteOnly: changeFavoriteOnly,
    sortKey,
    setSortKey: changeSortKey,
    showMoreProjects: () =>
      setVisibleCount((count) => count + PROJECT_PAGE_SIZE),
    refresh,
    scheduleWatcherRefresh,
    toggleFavorite,
    markProjectLaunched,
    openFolder,
    selectExternalProject,
    externalSelectionBusy,
  };
}

function normalizeProjectSourceKey(sharedDirectory: string) {
  return sharedDirectory.trim().replace(/[\\/]+$/, "");
}

function projectsSearch(projects: MendixProject[], needle: string) {
  if (!needle) return projects;
  return projects.filter((project) =>
    `${project.name} ${project.directory} ${project.version ?? ""}`
      .toLowerCase()
      .includes(needle),
  );
}

function projectsFilter(projects: MendixProject[], favoriteOnly: boolean) {
  return favoriteOnly
    ? projects.filter((project) => project.favorite)
    : projects;
}

function projectsSort(projects: MendixProject[], sortKey: ProjectSortKey) {
  return [...projects].sort((left, right) => {
    if (sortKey === "name") {
      return left.name.localeCompare(right.name, undefined, {
        sensitivity: "base",
      });
    }
    if (sortKey === "version") {
      return compareOptionalVersions(left.version, right.version);
    }
    if (sortKey === "recent") {
      return timestamp(right.lastLaunchedAt) - timestamp(left.lastLaunchedAt);
    }
    return timestamp(right.lastModified) - timestamp(left.lastModified);
  });
}

function compareOptionalVersions(
  left: string | undefined,
  right: string | undefined,
) {
  if (!left || !right) return Number(Boolean(left)) - Number(Boolean(right));
  return left.localeCompare(right, undefined, { numeric: true });
}

function timestamp(value: string | undefined) {
  if (!value) return 0;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : 0;
}
