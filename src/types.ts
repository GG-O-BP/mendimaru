export type ViewKey = "studio" | "projects" | "settings";

export interface AppConfig {
  languagePreference: string;
  winboatExecutable: string;
  composeFile: string;
  containerRuntime: "docker" | "podman" | string;
  containerName: string;
  apiUrl: string;
  rdpHost: string;
  rdpPort: number;
  sharedDirectory: string;
  windowsSharedDirectory: string;
  freerdpBinary: string;
  mendixInstallRoot: string;
  mendixDataRoot: string;
  startupTimeoutSeconds: number;
}

export interface EnvironmentStatus {
  winboatAvailable: boolean;
  composeAvailable: boolean;
  runtimeAvailable: boolean;
  freerdpAvailable: boolean;
  sharedDirectoryAvailable: boolean;
  sharedMountMatches: boolean;
  containerStatus: string;
  guestOnline: boolean;
  notices: string[];
}

export interface StudioVersion {
  version: string;
  displayName: string;
  executablePath: string;
  installRoot: string;
  source: string;
}

export interface DownloadableVersion {
  version: string;
  releaseDate?: string;
  releaseNotesUrl?: string;
  isLts: boolean;
  isBeta: boolean;
  isMts: boolean;
  isLatest: boolean;
  formattedReleaseDate?: string;
}

export interface StudioVersionCatalog {
  versions: DownloadableVersion[];
  loadedPages: number[];
  totalCount?: number;
  fetchedAt?: string;
}

export interface MendixProject {
  name: string;
  directory: string;
  mprPath: string;
  windowsPath: string;
  version?: string;
  lastModified?: string;
  formattedLastModified?: string;
}

export interface DownloadProgress {
  version: string;
  state: string;
  downloadedBytes: number;
  totalBytes?: number;
  percentage?: number;
  message: string;
  downloadedBytesLabel: string;
  totalBytesLabel?: string;
}

export interface LocaleOption {
  id: string;
  nativeName: string;
}

export interface LocalizationBundle {
  locale: string;
  preference: string;
  direction: "ltr" | "rtl";
  availableLocales: LocaleOption[];
  messages: Record<string, string>;
  numbers: string[];
}

export interface CommandError {
  code: string;
  message: string;
}

export interface SettingsSaveResult {
  config: AppConfig;
  mountChanged: boolean;
  containerRecreated: boolean;
  backupPath?: string;
}

export type ToastKind = "success" | "error" | "info";

export interface ToastMessage {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
}

export interface ConfirmationState {
  title: string;
  description: string;
  confirmLabel: string;
  danger?: boolean;
  action: () => Promise<void>;
}
