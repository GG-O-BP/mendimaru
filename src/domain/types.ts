import enumValues from "../shared/contracts/enumValues.json";

export type ViewKey = "studio" | "projects" | "settings";

export type HostPlatform = keyof typeof enumValues.hostPlatform;

export type BackendId = keyof typeof enumValues.backendId;

export type PlatformId = keyof typeof enumValues.platformId;

export type RuntimeMode = keyof typeof enumValues.runtimeMode;

export type CapabilityId = keyof typeof enumValues.capabilityId;

export type CapabilityStatus = keyof typeof enumValues.capabilityStatus;

export type BackendErrorCode = keyof typeof enumValues.backendErrorCode;

export interface CapabilityLimitation {
  code: BackendErrorCode;
  message: string;
  requiredPermission?: string;
  requiredVersion?: string;
}

export interface Capability {
  id: CapabilityId;
  status: CapabilityStatus;
  requiredPermissions: string[];
  fallbackAllowed: boolean;
  limitation?: CapabilityLimitation;
}

export interface CapabilityManifest {
  schemaVersion: string;
  backend: BackendId;
  hostPlatform: PlatformId;
  studioPlatform: PlatformId;
  runtimePlatform?: PlatformId;
  runtimeMode?: RuntimeMode;
  architecture: string;
  capabilities: Capability[];
}

export interface CapabilitySnapshot {
  schemaVersion: string;
  snapshotId: string;
  capturedAt: string;
  manifest: CapabilityManifest;
}

export interface BackendError {
  schemaVersion: string;
  code: BackendErrorCode;
  message: string;
  backend?: BackendId;
  capability?: CapabilityId;
  reason?: CapabilityLimitation;
  retryable: boolean;
  diagnosticRef?: string;
}

export type SessionState = keyof typeof enumValues.sessionState;

export interface SessionDescriptor {
  schemaVersion: string;
  sessionId: string;
  createdAt: string;
  state: SessionState;
  capabilitySnapshot: CapabilitySnapshot;
}

export type ArtifactKind = keyof typeof enumValues.artifactKind;

export interface ArtifactDescriptor {
  schemaVersion: string;
  artifactId: string;
  sessionId: string;
  backend: BackendId;
  kind: ArtifactKind;
  createdAt: string;
  mediaType?: string;
  location?: string;
  sha256?: string;
  sizeBytes?: number;
  backendDiagnosticRef?: string;
}

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
  windowsStudioPaths: string[];
  startupTimeoutSeconds: number;
}

export interface PlatformCapabilities {
  kind: HostPlatform;
  architecture: string;
  requiresWinboat: boolean;
  supportsStudioManagement: boolean;
  supportsInstallation: boolean;
  supportsUninstallation: boolean;
  supportsProjects: boolean;
}

export interface EnvironmentStatus {
  platform: PlatformCapabilities;
  ready: boolean;
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
  removable: boolean;
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
  details?: BackendError;
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
