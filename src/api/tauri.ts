import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  BackendId,
  CapabilitySnapshot,
  DownloadableVersion,
  DownloadProgress,
  EnvironmentStatus,
  InstallQueueItem,
  InstalledVersionsCache,
  LocalizationBundle,
  MendixProject,
  OperationRecord,
  ProjectScanResult,
  SettingsSavePreview,
  SettingsSaveResult,
  SettingsConnectionTestResult,
  StudioSessionStatus,
  StudioVersion,
  StudioVersionCatalog,
} from "../domain/types";

const commands = {
  getConfig: "get_config",
  getLocalization: "get_localization",
  setLanguagePreference: "set_language_preference",
  formatLocalizedDates: "format_localized_dates",
  formatLocalizedNumbers: "format_localized_numbers",
  formatLocalizedBytes: "format_localized_bytes",
  detectSettings: "detect_settings",
  redetectConfig: "redetect_config",
  previewSettingsSave: "preview_settings_save",
  saveConfig: "save_config",
  testSettingsConnection: "test_settings_connection",
  getCapabilities: "get_capabilities",
  getEnvironmentStatus: "get_environment_status",
  getEnvironmentDiagnosticReport: "get_environment_diagnostic_report",
  exportEnvironmentDiagnosticReport: "export_environment_diagnostic_report",
  getInstalledVersions: "get_installed_versions",
  getInstalledVersionsCache: "get_installed_versions_cache",
  getStudioSessions: "get_studio_sessions",
  getDownloadableVersionsCache: "get_downloadable_versions_cache",
  fetchDownloadableVersions: "fetch_downloadable_versions",
  resolveDownloadableVersion: "resolve_downloadable_version",
  getProjects: "get_projects",
  selectExternalProject: "select_external_project",
  setProjectLaunchPreference: "set_project_launch_preference",
  setProjectFavorite: "set_project_favorite",
  startWinBoatWindows: "start_winboat_windows",
  openWinBoat: "open_winboat",
  beginWinBoatSetup: "begin_winboat_setup",
  completeWinBoatSetup: "complete_winboat_setup",
  launchStudioPro: "launch_studio_pro",
  reconnectStudioSession: "reconnect_studio_session",
  stopStudioSession: "stop_studio_session",
  uninstallStudioPro: "uninstall_studio_pro",
  installStudioPro: "install_studio_pro",
  cancelStudioDownload: "cancel_studio_download",
  enqueueInstallQueueItem: "enqueue_install_queue_item",
  getInstallQueue: "get_install_queue",
  cancelInstallQueueItem: "cancel_install_queue_item",
  retryInstallQueueItem: "retry_install_queue_item",
  moveInstallQueueItem: "move_install_queue_item",
  removeInstallQueueItem: "remove_install_queue_item",
  getOperations: "get_operations",
  retryOperation: "retry_operation",
  clearOperationHistory: "clear_operation_history",
  openOperationLogs: "open_operation_logs",
  openFolder: "open_folder",
} as const;

export const tauriApi = {
  getConfig: () => invoke<AppConfig>(commands.getConfig),
  getLocalization: () => invoke<LocalizationBundle>(commands.getLocalization),
  setLanguagePreference: (language: string) =>
    invoke<LocalizationBundle>(commands.setLanguagePreference, { language }),
  formatDates: (values: string[]) =>
    invoke<string[]>(commands.formatLocalizedDates, { values }),
  formatNumbers: (values: number[]) =>
    invoke<string[]>(commands.formatLocalizedNumbers, { values }),
  formatBytes: (values: number[]) =>
    invoke<string[]>(commands.formatLocalizedBytes, { values }),
  detectSettings: () => invoke<AppConfig>(commands.detectSettings),
  redetectConfig: () => invoke<AppConfig>(commands.redetectConfig),
  previewSettingsSave: (config: AppConfig, applyMount: boolean) =>
    invoke<SettingsSavePreview | null>(commands.previewSettingsSave, {
      config,
      applyMount,
    }),
  saveConfig: (
    config: AppConfig,
    applyMount: boolean,
    composeRevision?: string,
  ) =>
    invoke<SettingsSaveResult>(commands.saveConfig, {
      config,
      applyMount,
      composeRevision: composeRevision ?? null,
    }),
  testSettingsConnection: (config: AppConfig) =>
    invoke<SettingsConnectionTestResult>(commands.testSettingsConnection, {
      config,
    }),
  getCapabilities: (backend?: BackendId) =>
    invoke<CapabilitySnapshot>(commands.getCapabilities, {
      backend: backend ?? null,
    }),
  getEnvironmentStatus: () =>
    invoke<EnvironmentStatus>(commands.getEnvironmentStatus),
  getEnvironmentDiagnosticReport: () =>
    invoke<string>(commands.getEnvironmentDiagnosticReport),
  exportEnvironmentDiagnosticReport: () =>
    invoke<boolean>(commands.exportEnvironmentDiagnosticReport),
  getInstalledVersions: () =>
    invoke<StudioVersion[]>(commands.getInstalledVersions),
  getInstalledVersionsCache: () =>
    invoke<InstalledVersionsCache>(commands.getInstalledVersionsCache),
  getStudioSessions: () =>
    invoke<StudioSessionStatus[]>(commands.getStudioSessions),
  getDownloadableVersionsCache: () =>
    invoke<StudioVersionCatalog>(commands.getDownloadableVersionsCache),
  fetchDownloadableVersions: (page: number, reset: boolean) =>
    invoke<StudioVersionCatalog>(commands.fetchDownloadableVersions, {
      page,
      reset,
    }),
  resolveDownloadableVersion: (version: string) =>
    invoke<DownloadableVersion>(commands.resolveDownloadableVersion, {
      version,
    }),
  getProjects: () => invoke<ProjectScanResult>(commands.getProjects),
  selectExternalProject: () =>
    invoke<MendixProject | null>(commands.selectExternalProject),
  setProjectLaunchPreference: (
    projectMprPath: string,
    selectedVersion: string | undefined,
    pending: boolean,
    completedLaunch = false,
  ) =>
    invoke<void>(commands.setProjectLaunchPreference, {
      projectMprPath,
      selectedVersion: selectedVersion ?? null,
      pending,
      completedLaunch,
    }),
  setProjectFavorite: (projectMprPath: string, favorite: boolean) =>
    invoke<void>(commands.setProjectFavorite, {
      projectMprPath,
      favorite,
    }),
  startWinBoatWindows: () => invoke<void>(commands.startWinBoatWindows),
  openWinBoat: () => invoke<void>(commands.openWinBoat),
  beginWinBoatSetup: () => invoke<void>(commands.beginWinBoatSetup),
  completeWinBoatSetup: () =>
    invoke<SettingsSaveResult>(commands.completeWinBoatSetup),
  launchStudioPro: (version: string, projectMprPath?: string) =>
    invoke<void>(commands.launchStudioPro, {
      version,
      projectMprPath: projectMprPath ?? null,
    }),
  reconnectStudioSession: (sessionId: string) =>
    invoke<void>(commands.reconnectStudioSession, { sessionId }),
  stopStudioSession: (sessionId: string) =>
    invoke<void>(commands.stopStudioSession, { sessionId }),
  uninstallStudioPro: (version: string) =>
    invoke<void>(commands.uninstallStudioPro, { version }),
  installStudioPro: (version: string, forceRedownload = false) =>
    invoke<void>(commands.installStudioPro, { version, forceRedownload }),
  cancelStudioDownload: () => invoke<boolean>(commands.cancelStudioDownload),
  enqueueInstallStudioPro: (version: string, forceRedownload = false) =>
    invoke<InstallQueueItem>(commands.enqueueInstallQueueItem, {
      version,
      forceRedownload,
    }),
  getInstallQueue: () => invoke<InstallQueueItem[]>(commands.getInstallQueue),
  cancelInstallQueueItem: (itemId: string, keepPartial: boolean) =>
    invoke<boolean>(commands.cancelInstallQueueItem, { itemId, keepPartial }),
  retryInstallQueueItem: (itemId: string) =>
    invoke<InstallQueueItem>(commands.retryInstallQueueItem, { itemId }),
  moveInstallQueueItem: (itemId: string, up: boolean) =>
    invoke<void>(commands.moveInstallQueueItem, { itemId, up }),
  removeInstallQueueItem: (itemId: string) =>
    invoke<void>(commands.removeInstallQueueItem, { itemId }),
  getOperations: () => invoke<OperationRecord[]>(commands.getOperations),
  retryOperation: (id: string) => invoke<void>(commands.retryOperation, { id }),
  clearOperationHistory: () => invoke<number>(commands.clearOperationHistory),
  openOperationLogs: () => invoke<void>(commands.openOperationLogs),
  openFolder: (path: string) => invoke<void>(commands.openFolder, { path }),
  onStudioDownloadProgress: (
    handler: (progress: DownloadProgress) => void,
  ): Promise<UnlistenFn> =>
    listen<DownloadProgress>("studio-download-progress", (event) => {
      handler(event.payload);
    }),
  onInstallQueueChanged: (
    handler: (items: InstallQueueItem[]) => void,
  ): Promise<UnlistenFn> =>
    listen<InstallQueueItem[]>("install-queue-changed", (event) => {
      handler(event.payload);
    }),
  onWorkspaceProjectsChanged: (
    handler: (sourceKey: string) => void,
  ): Promise<UnlistenFn> =>
    listen<string>("workspace-projects-changed", (event) => {
      handler(event.payload);
    }),
};
