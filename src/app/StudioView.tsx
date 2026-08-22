import type { LocalizationBundle } from "../domain/types";
import type { EnvironmentController } from "../features/settings/useEnvironment";
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
        loading: studio.installedLoading,
        loaded: studio.installedLoaded,
        stale: studio.installedStale,
        error: studio.installedError,
        sessionsLoading: studio.sessionsLoading,
        isLaunching: studio.isLaunching,
        isBusy,
        onRefresh: () => void studio.refreshInstalled(),
        onLaunch: (version) => void studio.launchVersion(version),
        onUninstall: studio.askUninstall,
        onReconnect: (session) => void studio.reconnectSession(session),
        onStop: studio.askStopSession,
      }}
      catalog={{
        versions: studio.filteredCatalog,
        totalCount: studio.catalog.totalCount,
        loadedCount: studio.catalog.versions.length,
        search: studio.search,
        supportFilters: studio.supportFilters,
        loading: studio.catalogLoading,
        error: studio.catalogError,
        hasMore: studio.hasMore,
        installedSet: studio.installedSet,
        installedVersionsLoaded: studio.installedLoaded,
        isInstalling: studio.isInstalling,
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
    />
  );
}
