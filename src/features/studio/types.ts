import type {
  DownloadableVersion,
  DownloadProgress,
  StudioVersion,
} from "../../domain/types";

export type VersionSupportFilter = "lts" | "mts";
export type VersionSupportFilters = Record<VersionSupportFilter, boolean>;
export type WinBoatControlKind = "settings" | "setup" | "open" | "start";

export const EMPTY_VERSION_SUPPORT_FILTERS: VersionSupportFilters = {
  lts: false,
  mts: false,
};

export interface InstalledVersionsModel {
  versions: StudioVersion[];
  isLaunching: boolean;
  isBusy: (key: string) => boolean;
  onRefresh: () => void;
  onLaunch: (version: StudioVersion) => void;
  onUninstall: (version: StudioVersion) => void;
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
  isInstalling: boolean;
  isBusy: (key: string) => boolean;
  onSearch: (value: string) => void;
  onToggleSupportFilter: (value: VersionSupportFilter) => void;
  onRefresh: () => void;
  onLoadMore: () => void;
  onInstall: (version: DownloadableVersion) => void;
}

export interface InstallationModel {
  progress: DownloadProgress | null;
  isInstalling: boolean;
  onCancel: () => void;
}
