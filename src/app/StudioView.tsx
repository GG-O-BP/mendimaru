import { useMemo } from "react";
import type { LocalizationBundle } from "../domain/types";
import type { EnvironmentController } from "../features/settings/useEnvironment";
import {
  catalogCacheIsFresh,
  selectUpdateCandidateVersions,
} from "../features/studio/selectors";
import { StudioPage } from "../features/studio/StudioPage";
import type { useStudio } from "../features/studio/useStudio";
import type { Translate } from "../i18n";

export function StudioView({
  t,
  localization,
  environment,
  studio,
  isBusy,
}: {
  t: Translate;
  localization: LocalizationBundle;
  environment: EnvironmentController;
  studio: ReturnType<typeof useStudio>;
  isBusy: (key: string) => boolean;
}) {
  const updateCandidates = useMemo(
    () =>
      selectUpdateCandidateVersions(studio.catalog, studio.installedVersions),
    [studio.catalog, studio.installedVersions],
  );
  const catalogFresh = useMemo(
    () => catalogCacheIsFresh(studio.catalog),
    [studio.catalog],
  );
  const connectedRemoteAppVersion = environment.status?.platform.requiresWinboat
    ? studio.sessions.find((session) => session.connection === "connected")
        ?.version
    : undefined;

  return (
    <StudioPage
      t={t}
      localization={localization}
      online={environment.online}
      offlineGuidance={environment.offlineGuidance}
      winBoatControl={environment.winBoatControl}
      installed={{
        versions: studio.installedVersions,
        sessions: studio.sessions,
        connectedRemoteAppVersion,
        loading: studio.installedLoading,
        loaded: studio.installedLoaded,
        launchReady: studio.launchReady,
        stale: studio.installedStale,
        error: studio.installedError,
        sessionsLoading: studio.sessionsLoading,
        isLaunching: studio.isLaunching,
        isBusy,
        onRefresh: () => void studio.refreshOverview(),
        onLaunch: (version) => void studio.launchVersion(version),
        onUninstall: studio.askUninstall,
        onReconnect: (session) => void studio.reconnectSession(session),
        onStop: studio.askStopSession,
      }}
      catalog={{
        versions: studio.filteredCatalog,
        totalCount: studio.catalog.totalCount,
        fetchedAt: studio.catalog.fetchedAt,
        cacheFresh: catalogFresh,
        loadedCount: studio.catalog.versions.length,
        search: studio.search,
        supportFilters: studio.supportFilters,
        loading: studio.catalogLoading,
        error: studio.catalogError,
        hasMore: studio.hasMore,
        installedSet: studio.installedSet,
        installedVersions: studio.installedVersions,
        updateCandidates,
        installedVersionsLoaded: studio.installedLoaded,
        studioSessionsLoading: studio.sessionsLoading,
        connectedRemoteAppVersion,
        isInstalling: studio.isInstalling,
        queuedVersions: studio.installQueue.activeVersions,
        isBusy,
        onSearch: studio.setSearch,
        onToggleSupportFilter: studio.toggleSupportFilter,
        onRefresh: () => void studio.refreshCatalog(),
        onLoadMore: () => void studio.loadMore(),
        onInstall: studio.askInstall,
      }}
      installation={{
        progress: studio.downloadProgress,
        isInstalling: studio.isInstalling,
        onCancel: () => void studio.cancelDownload(),
      }}
      queue={{
        items: studio.installQueue.items,
        activeVersions: studio.installQueue.activeVersions,
        onCancel: (itemId) => void studio.installQueue.cancelItem(itemId, true),
        onDiscard: (itemId) =>
          void studio.installQueue.cancelItem(itemId, false),
        onRetry: (itemId) => void studio.installQueue.retryItem(itemId),
        onMove: (itemId, up) => void studio.installQueue.moveItem(itemId, up),
        onRemove: (itemId) => void studio.installQueue.removeItem(itemId),
      }}
    />
  );
}
