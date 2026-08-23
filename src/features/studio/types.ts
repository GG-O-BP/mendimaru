import type {
  DownloadableVersion,
  DownloadProgress,
  StudioSessionStatus,
  StudioVersion,
} from "../../domain/types";

export type VersionSupportFilter = "lts" | "mts";
export type VersionSupportFilters = Record<VersionSupportFilter, boolean>;
export type EnvironmentControlKind =
  "settings" | "setup" | "open" | "start" | "native";

export const EMPTY_VERSION_SUPPORT_FILTERS: VersionSupportFilters = {
  lts: false,
  mts: false,
};

export interface InstalledVersionsModel {
  versions: StudioVersion[];
  sessions: StudioSessionStatus[];
  connectedRemoteAppVersion?: string;
  loading: boolean;
  loaded: boolean;
  launchReady: boolean;
  stale: boolean;
  error: string | null;
  sessionsLoading: boolean;
  isLaunching: boolean;
  isBusy: (key: string) => boolean;
  onRefresh: () => void;
  onLaunch: (version: StudioVersion) => void;
  onUninstall: (version: StudioVersion) => void;
  onReconnect: (session: StudioSessionStatus) => void;
  onStop: (session: StudioSessionStatus) => void;
}

export interface CatalogModel {
  versions: DownloadableVersion[];
  totalCount?: number;
  loadedCount: number;
  search: string;
  supportFilters: VersionSupportFilters;
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  installedSet: Set<string>;
  installedVersionsLoaded: boolean;
  studioSessionsLoading: boolean;
  connectedRemoteAppVersion?: string;
  isInstalling: boolean;
  isBusy: (key: string) => boolean;
  onSearch: (value: string) => void;
  onToggleSupportFilter: (value: VersionSupportFilter) => void;
  onRefresh: () => void;
  onLoadMore: () => void;
  onInstall: (version: DownloadableVersion, forceRedownload?: boolean) => void;
}

export interface InstallationModel {
  progress: DownloadProgress | null;
  isInstalling: boolean;
  onCancel: () => void;
}
