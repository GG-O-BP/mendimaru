import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  BackendId,
  CapabilitySnapshot,
  DownloadProgress,
  EnvironmentStatus,
  LocalizationBundle,
  MendixProject,
  SettingsSaveResult,
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
  redetectConfig: "redetect_config",
  saveConfig: "save_config",
  getCapabilities: "get_capabilities",
  getEnvironmentStatus: "get_environment_status",
  getInstalledVersions: "get_installed_versions",
  getDownloadableVersionsCache: "get_downloadable_versions_cache",
  fetchDownloadableVersions: "fetch_downloadable_versions",
  getProjects: "get_projects",
  startWinBoatWindows: "start_winboat_windows",
  openWinBoat: "open_winboat",
  beginWinBoatSetup: "begin_winboat_setup",
  completeWinBoatSetup: "complete_winboat_setup",
  launchStudioPro: "launch_studio_pro",
  uninstallStudioPro: "uninstall_studio_pro",
  installStudioPro: "install_studio_pro",
  cancelStudioDownload: "cancel_studio_download",
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
  redetectConfig: () => invoke<AppConfig>(commands.redetectConfig),
  saveConfig: (config: AppConfig, applyMount: boolean) =>
    invoke<SettingsSaveResult>(commands.saveConfig, { config, applyMount }),
  getCapabilities: (backend?: BackendId) =>
    invoke<CapabilitySnapshot>(commands.getCapabilities, {
      backend: backend ?? null,
    }),
  getEnvironmentStatus: () =>
    invoke<EnvironmentStatus>(commands.getEnvironmentStatus),
  getInstalledVersions: () =>
    invoke<StudioVersion[]>(commands.getInstalledVersions),
  getDownloadableVersionsCache: () =>
    invoke<StudioVersionCatalog>(commands.getDownloadableVersionsCache),
  fetchDownloadableVersions: (page: number, reset: boolean) =>
    invoke<StudioVersionCatalog>(commands.fetchDownloadableVersions, {
      page,
      reset,
    }),
  getProjects: () => invoke<MendixProject[]>(commands.getProjects),
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
  uninstallStudioPro: (version: string) =>
    invoke<void>(commands.uninstallStudioPro, { version }),
  installStudioPro: (version: string, forceRedownload = false) =>
    invoke<void>(commands.installStudioPro, { version, forceRedownload }),
  cancelStudioDownload: () => invoke<boolean>(commands.cancelStudioDownload),
  openFolder: (path: string) => invoke<void>(commands.openFolder, { path }),
  onStudioDownloadProgress: (
    handler: (progress: DownloadProgress) => void,
  ): Promise<UnlistenFn> =>
    listen<DownloadProgress>("studio-download-progress", (event) => {
      handler(event.payload);
    }),
};
