import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type {
  AppConfig,
  EnvironmentStatus,
  LocalizationBundle,
  MendixProject,
  StudioVersion,
  StudioVersionCatalog,
} from "./domain/types";
import uiMessages from "./shared/contracts/uiMessages.json";

const mocks = vi.hoisted(() => ({
  getConfig: vi.fn(),
  getLocalization: vi.fn(),
  setLanguagePreference: vi.fn(),
  formatDates: vi.fn(),
  formatNumbers: vi.fn(),
  formatBytes: vi.fn(),
  redetectConfig: vi.fn(),
  saveConfig: vi.fn(),
  getEnvironmentStatus: vi.fn(),
  getInstalledVersions: vi.fn(),
  getDownloadableVersionsCache: vi.fn(),
  fetchDownloadableVersions: vi.fn(),
  getProjects: vi.fn(),
  startWinBoatWindows: vi.fn(),
  openWinBoat: vi.fn(),
  beginWinBoatSetup: vi.fn(),
  completeWinBoatSetup: vi.fn(),
  launchStudioPro: vi.fn(),
  uninstallStudioPro: vi.fn(),
  installStudioPro: vi.fn(),
  cancelStudioDownload: vi.fn(),
  openFolder: vi.fn(),
  onStudioDownloadProgress: vi.fn(),
  openDialog: vi.fn(),
  setWindowTitle: vi.fn(),
}));

vi.mock("./api/tauri", () => ({
  tauriApi: {
    getConfig: mocks.getConfig,
    getLocalization: mocks.getLocalization,
    setLanguagePreference: mocks.setLanguagePreference,
    formatDates: mocks.formatDates,
    formatNumbers: mocks.formatNumbers,
    formatBytes: mocks.formatBytes,
    redetectConfig: mocks.redetectConfig,
    saveConfig: mocks.saveConfig,
    getEnvironmentStatus: mocks.getEnvironmentStatus,
    getInstalledVersions: mocks.getInstalledVersions,
    getDownloadableVersionsCache: mocks.getDownloadableVersionsCache,
    fetchDownloadableVersions: mocks.fetchDownloadableVersions,
    getProjects: mocks.getProjects,
    startWinBoatWindows: mocks.startWinBoatWindows,
    openWinBoat: mocks.openWinBoat,
    beginWinBoatSetup: mocks.beginWinBoatSetup,
    completeWinBoatSetup: mocks.completeWinBoatSetup,
    launchStudioPro: mocks.launchStudioPro,
    uninstallStudioPro: mocks.uninstallStudioPro,
    installStudioPro: mocks.installStudioPro,
    cancelStudioDownload: mocks.cancelStudioDownload,
    openFolder: mocks.openFolder,
    onStudioDownloadProgress: mocks.onStudioDownloadProgress,
  },
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openDialog }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTitle: mocks.setWindowTitle }),
}));

const config: AppConfig = {
  languagePreference: "system",
  winboatSetupPending: false,
  winboatExecutable: "",
  composeFile: "",
  containerRuntime: "docker",
  containerName: "",
  apiUrl: "",
  rdpHost: "",
  rdpPort: 0,
  sharedDirectory: String.raw`C:\Users\dev\Mendix`,
  windowsSharedDirectory: "",
  freerdpBinary: "",
  mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
  mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
  windowsStudioPaths: [],
  startupTimeoutSeconds: 180,
};

const status: EnvironmentStatus = {
  platform: {
    kind: "windows-native",
    architecture: "x86_64",
    requiresWinboat: false,
    supportsStudioManagement: true,
    supportsInstallation: true,
    supportsUninstallation: true,
    supportsProjects: true,
  },
  ready: true,
  winboatAvailable: false,
  winboatInitialized: false,
  setupPending: false,
  composeAvailable: false,
  runtimeAvailable: true,
  freerdpAvailable: false,
  sharedDirectoryAvailable: true,
  sharedMountMatches: true,
  containerStatus: "not-found",
  guestOnline: true,
};

const removableStudio: StudioVersion = {
  version: "11.12.2",
  displayName: "Mendix 11.12.2",
  executablePath: String.raw`C:\Program Files\Mendix\11.12.2\modeler\StudioPro.exe`,
  installRoot: String.raw`C:\Program Files\Mendix\11.12.2`,
  source: "Windows Registry",
  removable: true,
};

const portableStudio: StudioVersion = {
  version: "10.24.9",
  displayName: "Mendix 10.24.9",
  executablePath: String.raw`D:\Portable\Mendix\10.24.9\modeler\StudioPro.exe`,
  installRoot: String.raw`D:\Portable\Mendix\10.24.9`,
  source: "Custom path",
  removable: false,
};

const project: MendixProject = {
  name: "Orders",
  directory: String.raw`C:\Users\dev\Mendix\Orders`,
  mprPath: String.raw`C:\Users\dev\Mendix\Orders\Orders.mpr`,
  windowsPath: String.raw`C:\Users\dev\Mendix\Orders\Orders.mpr`,
  version: "11.12.2",
  lastModified: "2026-08-14T03:00:00Z",
};

const catalog: StudioVersionCatalog = {
  versions: [
    {
      version: "11.13.0",
      releaseDate: "2026-07-28",
      isLts: false,
      isBeta: false,
      isMts: true,
      isLatest: true,
    },
  ],
  loadedPages: [1],
  totalCount: 1,
  fetchedAt: "2026-08-14T03:00:00Z",
};

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [{ id: "en-US", nativeName: "English" }],
  messages: Object.fromEntries(
    Object.keys(uiMessages).map((key) => [key, key]),
  ),
  numbers: [],
};

let installed: StudioVersion[];

beforeEach(() => {
  installed = [removableStudio, portableStudio];
  mocks.getConfig.mockResolvedValue({ ...config });
  mocks.getLocalization.mockResolvedValue(localization);
  mocks.setLanguagePreference.mockResolvedValue(localization);
  mocks.formatDates.mockImplementation(async (values: string[]) => values);
  mocks.formatNumbers.mockImplementation(async (values: number[]) =>
    values.map(String),
  );
  mocks.formatBytes.mockImplementation(async (values: number[]) =>
    values.map(String),
  );
  mocks.getEnvironmentStatus.mockResolvedValue(status);
  mocks.getInstalledVersions.mockImplementation(async () => [...installed]);
  mocks.getDownloadableVersionsCache.mockResolvedValue(catalog);
  mocks.fetchDownloadableVersions.mockResolvedValue(catalog);
  mocks.getProjects.mockResolvedValue([project]);
  mocks.launchStudioPro.mockResolvedValue(undefined);
  mocks.uninstallStudioPro.mockImplementation(async (version: string) => {
    installed = installed.filter((item) => item.version !== version);
  });
  mocks.installStudioPro.mockImplementation(async (version: string) => {
    installed = [
      ...installed,
      { ...removableStudio, version, displayName: `Mendix ${version}` },
    ];
  });
  mocks.cancelStudioDownload.mockResolvedValue(true);
  mocks.openFolder.mockResolvedValue(undefined);
  mocks.onStudioDownloadProgress.mockResolvedValue(vi.fn());
  mocks.openDialog.mockResolvedValue(undefined);
  mocks.setWindowTitle.mockResolvedValue(undefined);
  mocks.redetectConfig.mockResolvedValue({ ...config });
  mocks.saveConfig.mockImplementation(async (nextConfig: AppConfig) => ({
    config: nextConfig,
    mountChanged: false,
    containerRecreated: false,
  }));
});

async function renderReadyApp() {
  render(<App />);
  await screen.findByText("route-native-windows");
}

describe("native Windows application integration", () => {
  it("renders native capabilities without any WinBoat route or control", async () => {
    await renderReadyApp();

    expect(screen.getByText("connection-native")).toBeInTheDocument();
    expect(screen.queryByText("route-linux")).not.toBeInTheDocument();
    expect(screen.queryByText("route-windows")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /WinBoat/i })).toBeNull();
    expect(mocks.startWinBoatWindows).not.toHaveBeenCalled();
    expect(mocks.openWinBoat).not.toHaveBeenCalled();
  });

  it("launches, installs, and safely uninstalls Studio Pro", async () => {
    await renderReadyApp();
    await screen.findByText("11.12.2");

    fireEvent.click(
      screen.getAllByRole("button", { name: "action-launch" })[0],
    );
    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith("11.12.2", undefined),
    );

    expect(screen.getByTitle("removal-unavailable-title")).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "action-install" }));
    const installDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(installDialog).getByRole("button", {
        name: "action-download-install",
      }),
    );
    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.13.0"),
    );

    const installedCard = screen.getByText("11.12.2").closest("article");
    expect(installedCard).not.toBeNull();
    fireEvent.click(within(installedCard!).getByTitle("remove-version-title"));
    const uninstallDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(uninstallDialog).getByRole("button", { name: "action-uninstall" }),
    );
    await waitFor(() =>
      expect(mocks.uninstallStudioPro).toHaveBeenCalledWith("11.12.2"),
    );
  });

  it("opens native folders and launches a project with its exact version", async () => {
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");

    fireEvent.click(screen.getByRole("button", { name: "action-open-folder" }));
    fireEvent.click(screen.getByTitle("open-linux-folder"));
    fireEvent.click(screen.getByRole("button", { name: "action-open" }));

    await waitFor(() => {
      expect(mocks.openFolder).toHaveBeenCalledWith(config.sharedDirectory);
      expect(mocks.openFolder).toHaveBeenCalledWith(project.directory);
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "11.12.2",
        project.mprPath,
      );
    });
  });

  it("adds a portable Studio path and saves without applying a WinBoat mount", async () => {
    mocks.openDialog.mockResolvedValue(
      String.raw`D:\Portable\Mendix\11.8.0\modeler\StudioPro.exe`,
    );
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-settings/ }));
    await screen.findByText("settings-native-title");

    expect(screen.queryByText("WinBoat")).not.toBeInTheDocument();
    expect(
      screen.queryByText("settings-apply-now-title"),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "action-add-studio-path" }),
    );
    const portablePath = String.raw`D:\Portable\Mendix\11.8.0\modeler\StudioPro.exe`;
    await screen.findByDisplayValue(portablePath);
    fireEvent.click(
      screen.getByRole("button", { name: "action-save-settings" }),
    );

    await waitFor(() => expect(mocks.saveConfig).toHaveBeenCalled());
    const [savedConfig, applyMount] =
      mocks.saveConfig.mock.calls[mocks.saveConfig.mock.calls.length - 1] ?? [];
    expect(savedConfig.windowsStudioPaths).toEqual([portablePath]);
    expect(applyMount).toBe(false);
  });
});
