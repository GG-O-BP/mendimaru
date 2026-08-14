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
  OperationRecord,
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
  getEnvironmentDiagnosticReport: vi.fn(),
  exportEnvironmentDiagnosticReport: vi.fn(),
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
  getOperations: vi.fn(),
  retryOperation: vi.fn(),
  clearOperationHistory: vi.fn(),
  openOperationLogs: vi.fn(),
  openFolder: vi.fn(),
  onStudioDownloadProgress: vi.fn(),
  openDialog: vi.fn(),
  setWindowTitle: vi.fn(),
  writeClipboard: vi.fn(),
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
    getEnvironmentDiagnosticReport: mocks.getEnvironmentDiagnosticReport,
    exportEnvironmentDiagnosticReport: mocks.exportEnvironmentDiagnosticReport,
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
    getOperations: mocks.getOperations,
    retryOperation: mocks.retryOperation,
    clearOperationHistory: mocks.clearOperationHistory,
    openOperationLogs: mocks.openOperationLogs,
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
  diagnostics: [
    {
      id: "shared-directory",
      status: "success",
    },
    {
      id: "marketplace-browser",
      status: "warning",
      action: "redetect",
    },
  ],
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
  vi.clearAllMocks();
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
  mocks.getEnvironmentDiagnosticReport.mockResolvedValue(
    '{"schemaVersion":"1.0.0","checks":[]}',
  );
  mocks.exportEnvironmentDiagnosticReport.mockResolvedValue(true);
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
  mocks.getOperations.mockResolvedValue([]);
  mocks.retryOperation.mockResolvedValue(undefined);
  mocks.clearOperationHistory.mockResolvedValue(0);
  mocks.openOperationLogs.mockResolvedValue(undefined);
  mocks.openFolder.mockResolvedValue(undefined);
  mocks.onStudioDownloadProgress.mockResolvedValue(vi.fn());
  mocks.openDialog.mockResolvedValue(undefined);
  mocks.setWindowTitle.mockResolvedValue(undefined);
  mocks.writeClipboard.mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: mocks.writeClipboard },
  });
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

describe("native Windows application E2E", () => {
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
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.13.0", false),
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

  it("forces a fresh installer download only after explicit confirmation", async () => {
    await renderReadyApp();

    fireEvent.click(await screen.findByTitle("action-force-redownload"));
    const redownloadDialog = await screen.findByRole("dialog");
    expect(
      within(redownloadDialog).getByText(
        "confirm-force-redownload-description",
      ),
    ).toBeInTheDocument();
    fireEvent.click(
      within(redownloadDialog).getByRole("button", {
        name: "action-force-redownload-install",
      }),
    );

    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.13.0", true),
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

  it("renders independent diagnostics and copies, exports, and repairs safely", async () => {
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-settings/ }));
    await screen.findByText("diagnostics-title");

    expect(screen.getByText("diagnostic-shared-directory-title")).toBeVisible();
    expect(screen.getByText("diagnostic-status-success")).toBeVisible();
    expect(screen.getByText("diagnostic-browser-title")).toBeVisible();
    expect(screen.getByText("diagnostic-status-warning")).toBeVisible();
    expect(screen.getByText("diagnostic-browser-recovery")).toBeVisible();

    fireEvent.click(
      screen.getByRole("button", { name: "action-copy-diagnostic-report" }),
    );
    await waitFor(() => {
      expect(mocks.getEnvironmentDiagnosticReport).toHaveBeenCalledOnce();
      expect(mocks.writeClipboard).toHaveBeenCalledWith(
        '{"schemaVersion":"1.0.0","checks":[]}',
      );
    });

    fireEvent.click(
      screen.getByRole("button", { name: "action-export-diagnostic-report" }),
    );
    await waitFor(() =>
      expect(mocks.exportEnvironmentDiagnosticReport).toHaveBeenCalledOnce(),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "diagnostic-action-redetect" }),
    );
    await waitFor(() => expect(mocks.redetectConfig).toHaveBeenCalledOnce());
    expect(mocks.startWinBoatWindows).not.toHaveBeenCalled();
  });

  it("restores persistent operations, exposes safe failure context, retries, and clears only terminal history", async () => {
    let operationRecords: OperationRecord[] = [
      {
        schemaVersion: "1.0.0",
        id: "install-11.13.0-0123456789abcdef0123456789abcdef",
        kind: "install",
        targetVersion: "11.13.0",
        protectedProject: false,
        state: "running",
        stage: "downloading",
        percentage: 47,
        estimated: false,
        startedAt: "2026-08-15T02:00:00Z",
        updatedAt: "2026-08-15T02:01:00Z",
        retryable: false,
        logAvailable: true,
      },
      {
        schemaVersion: "1.0.0",
        id: "uninstall-11.12.2-fedcba9876543210fedcba9876543210",
        kind: "uninstall",
        targetVersion: "11.12.2",
        protectedProject: false,
        state: "failed",
        stage: "uninstalling",
        estimated: false,
        startedAt: "2026-08-15T01:00:00Z",
        updatedAt: "2026-08-15T01:01:00Z",
        finishedAt: "2026-08-15T01:01:00Z",
        error: {
          code: "operation_failed",
          reason: "operation_failed",
          exitCode: 1603,
        },
        retryable: true,
        logAvailable: true,
      },
      {
        schemaVersion: "1.0.0",
        id: "launch-11.12.2-aabbccddeeff00112233445566778899",
        kind: "launch",
        targetVersion: "11.12.2",
        protectedProject: true,
        state: "interrupted",
        stage: "interrupted",
        estimated: false,
        startedAt: "2026-08-14T23:00:00Z",
        updatedAt: "2026-08-14T23:01:00Z",
        finishedAt: "2026-08-14T23:01:00Z",
        error: {
          code: "operation_interrupted",
          reason: "operation_interrupted",
        },
        retryable: false,
        logAvailable: true,
      },
      {
        schemaVersion: "1.0.0",
        id: "install-11.11.0-11112222333344445555666677778888",
        kind: "install",
        targetVersion: "11.11.0",
        protectedProject: false,
        state: "succeeded",
        stage: "completed",
        percentage: 100,
        estimated: false,
        startedAt: "2026-08-14T22:00:00Z",
        updatedAt: "2026-08-14T22:05:00Z",
        finishedAt: "2026-08-14T22:05:00Z",
        retryable: false,
        logAvailable: true,
      },
      {
        schemaVersion: "1.0.0",
        id: "install-11.10.0-99990000aaaabbbbccccddddeeeeffff",
        kind: "install",
        targetVersion: "11.10.0",
        protectedProject: false,
        state: "cancelled",
        stage: "downloading",
        percentage: 21,
        estimated: false,
        startedAt: "2026-08-14T21:00:00Z",
        updatedAt: "2026-08-14T21:02:00Z",
        finishedAt: "2026-08-14T21:02:00Z",
        error: {
          code: "download_cancelled",
          reason: "download_cancelled",
        },
        retryable: true,
        logAvailable: true,
      },
    ];
    mocks.getOperations.mockImplementation(async () => [...operationRecords]);
    mocks.getLocalization.mockResolvedValue({
      ...localization,
      messages: {
        ...localization.messages,
        "operation-exit-code": "Exit code %code%",
      },
    });
    mocks.clearOperationHistory.mockImplementation(async () => {
      const removed = operationRecords.filter(
        (operation) => operation.state !== "running",
      ).length;
      operationRecords = operationRecords.filter(
        (operation) => operation.state === "running",
      );
      return removed;
    });

    const app = render(<App />);
    await screen.findByText("route-native-windows");
    fireEvent.click(screen.getByRole("button", { name: /nav-operations/ }));

    expect(await screen.findByText("operation-state-running")).toBeVisible();
    expect(screen.getByText("operation-state-succeeded")).toBeVisible();
    expect(screen.getByText("operation-state-cancelled")).toBeVisible();
    expect(screen.getByText("47%")).toBeVisible();
    expect(screen.getByText(/Exit code 1603/)).toBeVisible();
    expect(screen.getByText("operation-project-protected")).toBeVisible();
    expect(screen.getByText(/operation-reason-operation-failed/)).toBeVisible();

    const failedItem = screen
      .getByText("operation-state-failed")
      .closest("article");
    expect(failedItem).not.toBeNull();
    fireEvent.click(
      within(failedItem!).getByRole("button", {
        name: "action-retry-operation",
      }),
    );
    await waitFor(() =>
      expect(mocks.retryOperation).toHaveBeenCalledWith(
        "uninstall-11.12.2-fedcba9876543210fedcba9876543210",
      ),
    );

    const protectedItem = screen
      .getByText("operation-project-protected")
      .closest("article");
    expect(
      within(protectedItem!).getByRole("button", {
        name: "action-retry-operation",
      }),
    ).toBeDisabled();

    fireEvent.click(
      screen.getByRole("button", { name: "action-open-operation-logs" }),
    );
    await waitFor(() => expect(mocks.openOperationLogs).toHaveBeenCalledOnce());

    fireEvent.click(
      screen.getByRole("button", { name: "action-clear-operation-history" }),
    );
    const clearDialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(clearDialog).getByRole("button", {
        name: "action-clear-operation-history",
      }),
    );
    await waitFor(() =>
      expect(mocks.clearOperationHistory).toHaveBeenCalledOnce(),
    );
    await waitFor(() =>
      expect(screen.queryByText("operation-state-failed")).toBeNull(),
    );
    expect(screen.getByText("operation-state-running")).toBeVisible();

    app.unmount();
    render(<App />);
    await screen.findByText("route-native-windows");
    fireEvent.click(screen.getByRole("button", { name: /nav-operations/ }));
    expect(await screen.findByText("operation-state-running")).toBeVisible();
  });
});

describe("Linux environment diagnostic E2E", () => {
  it("keeps a partial failure actionable without treating the guest as ready", async () => {
    const linuxConfig: AppConfig = {
      ...config,
      winboatExecutable: "/opt/winboat/winboat",
      composeFile: "/home/dev/.winboat/docker-compose.yml",
      containerName: "WinBoat",
      apiUrl: "http://127.0.0.1:47280",
      rdpHost: "127.0.0.1",
      rdpPort: 47300,
      sharedDirectory: "/home/dev/Mendix",
      windowsSharedDirectory: String.raw`\\host.lan\Data`,
      freerdpBinary: "xfreerdp3",
    };
    const partialStatus: EnvironmentStatus = {
      ...status,
      platform: {
        ...status.platform,
        kind: "linux-winboat",
        requiresWinboat: true,
      },
      ready: false,
      winboatAvailable: true,
      winboatInitialized: true,
      composeAvailable: true,
      runtimeAvailable: true,
      freerdpAvailable: true,
      sharedDirectoryAvailable: true,
      sharedMountMatches: false,
      containerStatus: "exited",
      guestOnline: false,
      diagnostics: [
        { id: "container-runtime", status: "success", observed: "29.7.2" },
        {
          id: "shared-mount",
          status: "failure",
          action: "open-settings",
        },
        {
          id: "container",
          status: "warning",
          observed: "exited",
          action: "start-winboat",
        },
        {
          id: "guest-api",
          status: "warning",
          action: "start-winboat",
        },
      ],
    };
    mocks.getConfig.mockResolvedValue(linuxConfig);
    mocks.getEnvironmentStatus.mockResolvedValue(partialStatus);
    mocks.startWinBoatWindows.mockResolvedValue(undefined);

    render(<App />);
    await screen.findByText("route-linux");
    expect(screen.getByText("connection-offline")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /nav-settings/ }));
    await screen.findByText("diagnostics-title");

    expect(screen.getByText("diagnostic-status-failure")).toBeVisible();
    expect(screen.getAllByText("diagnostic-status-warning")).toHaveLength(2);
    fireEvent.click(
      screen.getAllByRole("button", { name: "diagnostic-action-start" })[0],
    );
    await waitFor(() =>
      expect(mocks.startWinBoatWindows).toHaveBeenCalledOnce(),
    );
    expect(mocks.saveConfig).not.toHaveBeenCalled();
  });
});
