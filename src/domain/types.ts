import enumValues from "../shared/contracts/enumValues.json";

export type ViewKey = "studio" | "projects" | "settings";

export type ContainerRuntime = keyof typeof enumValues.containerRuntime;

export type ContainerStatus = keyof typeof enumValues.containerStatus;

export type DownloadState = keyof typeof enumValues.downloadState;
export type TextDirection = keyof typeof enumValues.textDirection;

export interface AppConfig {
  languagePreference: string;
  winboatSetupPending: boolean;
  winboatExecutable: string;
  composeFile: string;
  containerRuntime: ContainerRuntime;
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
  winboatInitialized: boolean;
  setupPending: boolean;
  composeAvailable: boolean;
  runtimeAvailable: boolean;
  freerdpAvailable: boolean;
  sharedDirectoryAvailable: boolean;
  sharedMountMatches: boolean;
  containerStatus: ContainerStatus;
  guestOnline: boolean;
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
}

export interface DownloadProgress {
  version: string;
  state: DownloadState;
  downloadedBytes: number;
  totalBytes?: number;
  percentage?: number;
  estimated: boolean;
  message: string;
}

export interface LocaleOption {
  id: string;
  nativeName: string;
}

export interface LocalizationBundle {
  locale: string;
  preference: string;
  direction: TextDirection;
  availableLocales: LocaleOption[];
  messages: Record<string, string>;
  numbers: string[];
}

export type CommandErrorCode = keyof typeof enumValues.commandErrorCode;

export interface CommandError {
  code: CommandErrorCode;
  message: string;
}

export interface SettingsSaveResult {
  config: AppConfig;
  mountChanged: boolean;
  containerRecreated: boolean;
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
