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
  DownloadableVersion,
  DownloadProgress,
  EnvironmentStatus,
  LocalizationBundle,
  MendixProject,
  OperationRecord,
  StudioSessionStatus,
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
  getStudioSessions: vi.fn(),
  getDownloadableVersionsCache: vi.fn(),
  fetchDownloadableVersions: vi.fn(),
  resolveDownloadableVersion: vi.fn(),
  getProjects: vi.fn(),
  setProjectLaunchPreference: vi.fn(),
  startWinBoatWindows: vi.fn(),
  openWinBoat: vi.fn(),
  beginWinBoatSetup: vi.fn(),
  completeWinBoatSetup: vi.fn(),
  launchStudioPro: vi.fn(),
  reconnectStudioSession: vi.fn(),
  stopStudioSession: vi.fn(),
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
    getStudioSessions: mocks.getStudioSessions,
    getDownloadableVersionsCache: mocks.getDownloadableVersionsCache,
    fetchDownloadableVersions: mocks.fetchDownloadableVersions,
    resolveDownloadableVersion: mocks.resolveDownloadableVersion,
    getProjects: mocks.getProjects,
    setProjectLaunchPreference: mocks.setProjectLaunchPreference,
    startWinBoatWindows: mocks.startWinBoatWindows,
    openWinBoat: mocks.openWinBoat,
    beginWinBoatSetup: mocks.beginWinBoatSetup,
    completeWinBoatSetup: mocks.completeWinBoatSetup,
    launchStudioPro: mocks.launchStudioPro,
    reconnectStudioSession: mocks.reconnectStudioSession,
    stopStudioSession: mocks.stopStudioSession,
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
  launchPending: false,
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
let sessions: StudioSessionStatus[];

beforeEach(() => {
  vi.clearAllMocks();
  installed = [removableStudio, portableStudio];
  sessions = [];
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
  mocks.getStudioSessions.mockImplementation(async () => [...sessions]);
  mocks.getDownloadableVersionsCache.mockResolvedValue(catalog);
  mocks.fetchDownloadableVersions.mockResolvedValue(catalog);
  mocks.resolveDownloadableVersion.mockImplementation(
    async (version: string) => ({
      version,
      isLts: false,
      isBeta: false,
      isMts: false,
      isLatest: false,
    }),
  );
  mocks.getProjects.mockResolvedValue([project]);
  mocks.setProjectLaunchPreference.mockResolvedValue(undefined);
  mocks.launchStudioPro.mockResolvedValue(undefined);
  mocks.reconnectStudioSession.mockImplementation(async (sessionId: string) => {
    sessions = sessions.map((session) =>
      session.sessionId === sessionId
        ? {
            ...session,
            connection: "connected",
            reconnectable: false,
            reconnectUnavailable: "already-connected",
          }
        : session,
    );
  });
  mocks.stopStudioSession.mockImplementation(async (sessionId: string) => {
    sessions = sessions.filter((session) => session.sessionId !== sessionId);
  });
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

  it("manages exact running sessions across versions with reconnect and confirmed safe close", async () => {
    sessions = [
      {
        schemaVersion: "1.0.0",
        sessionId: "studio-4242-638908236000000000",
        version: "11.12.2",
        state: "running",
        processId: 4242,
        startedAt: "2026-08-15T03:00:00Z",
        projectName: "Orders",
        connection: "disconnected",
        reconnectable: true,
      },
      {
        schemaVersion: "1.0.0",
        sessionId: "studio-5151-638908200000000000",
        version: "10.24.9",
        state: "running",
        processId: 5151,
        startedAt: "2026-08-15T02:00:00Z",
        connection: "connected",
        reconnectable: false,
        reconnectUnavailable: "already-connected",
      },
    ];

    await renderReadyApp();
    expect(await screen.findByText("Orders")).toBeVisible();
    expect(screen.getAllByText("studio-session-count")).toHaveLength(2);
    expect(screen.getByText("studio-session-disconnected")).toBeVisible();
    expect(screen.getByText("studio-session-connected")).toBeVisible();

    const ordersRow = screen
      .getByText("Orders")
      .closest<HTMLElement>(".studio-session");
    expect(ordersRow).not.toBeNull();
    const ordersCard = ordersRow!.closest("article");
    expect(ordersCard).not.toBeNull();
    expect(
      within(ordersCard!).getByTitle("running-version-title"),
    ).toBeDisabled();

    fireEvent.click(
      within(ordersRow!).getByRole("button", {
        name: "action-reconnect-session",
      }),
    );
    await waitFor(() =>
      expect(mocks.reconnectStudioSession).toHaveBeenCalledWith(
        "studio-4242-638908236000000000",
      ),
    );
    await waitFor(() =>
      expect(
        within(ordersRow!).getByTitle("session-reconnect-already-connected"),
      ).toBeDisabled(),
    );

    fireEvent.click(within(ordersRow!).getByTitle("action-stop-session"));
    const closeDialog = await screen.findByRole("dialog");
    expect(mocks.stopStudioSession).not.toHaveBeenCalled();
    expect(
      within(closeDialog).getByText("confirm-stop-session-description"),
    ).toBeVisible();
    fireEvent.click(
      within(closeDialog).getByRole("button", {
        name: "action-stop-session",
      }),
    );
    await waitFor(() =>
      expect(mocks.stopStudioSession).toHaveBeenCalledWith(
        "studio-4242-638908236000000000",
      ),
    );
    await waitFor(() => expect(screen.queryByText("Orders")).toBeNull());
    expect(
      within(ordersCard!).getByTitle("remove-version-title"),
    ).toBeEnabled();

    const connectedRow = screen
      .getByText("studio-session-connected")
      .closest<HTMLElement>(".studio-session");
    expect(connectedRow).not.toBeNull();
    expect(
      within(connectedRow!).getByTitle("session-reconnect-already-connected"),
    ).toBeDisabled();
  });

  it("refreshes away an already-ended session after a targeted close fails", async () => {
    const endedId = "studio-6161-638908164000000000";
    sessions = [
      {
        schemaVersion: "1.0.0",
        sessionId: endedId,
        version: "11.12.2",
        state: "running",
        processId: 6161,
        startedAt: "2026-08-15T01:00:00Z",
        projectName: "EndedOrders",
        connection: "native",
        reconnectable: true,
      },
    ];
    mocks.stopStudioSession.mockImplementationOnce(async () => {
      sessions = [];
      throw {
        code: "operation_failed",
        message: "The selected Studio Pro session has already ended.",
      };
    });

    await renderReadyApp();
    const row = (await screen.findByText("EndedOrders")).closest<HTMLElement>(
      ".studio-session",
    );
    expect(row).not.toBeNull();
    fireEvent.click(within(row!).getByTitle("action-stop-session"));
    fireEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "action-stop-session",
      }),
    );

    await waitFor(() =>
      expect(mocks.stopStudioSession).toHaveBeenCalledWith(endedId),
    );
    expect(
      await screen.findByText(
        "The selected Studio Pro session has already ended.",
      ),
    ).toBeVisible();
    await waitFor(() => expect(screen.queryByText("EndedOrders")).toBeNull());
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

  it("keeps an explicit version choice when an older exact lookup completes later", async () => {
    installed = [portableStudio];
    let finishLookup: (version: DownloadableVersion) => void = () => {
      throw new Error("lookup resolver was not initialized");
    };
    mocks.resolveDownloadableVersion.mockImplementationOnce(
      () =>
        new Promise<DownloadableVersion>((resolve) => {
          finishLookup = resolve;
        }),
    );
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(mocks.resolveDownloadableVersion).toHaveBeenCalledWith("11.12.2"),
    );

    const versionSelect = within(assistant).getByLabelText(
      "project-launch-select-version",
    );
    fireEvent.change(versionSelect, { target: { value: "10.24.9" } });
    finishLookup({
      version: "11.12.2",
      isLts: true,
      isBeta: false,
      isMts: false,
      isLatest: false,
    });

    await waitFor(() => expect(versionSelect).toHaveValue("10.24.9"));
    fireEvent.click(
      within(assistant).getByLabelText("project-launch-mismatch-acknowledge"),
    );
    fireEvent.click(
      within(assistant).getByRole("button", { name: "project-launch-open" }),
    );
    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "10.24.9",
        project.mprPath,
      ),
    );
    expect(mocks.launchStudioPro).not.toHaveBeenCalledWith(
      "11.12.2",
      project.mprPath,
    );
  });

  it("resumes the persisted project choice returned after an app restart", async () => {
    const resumedProject: MendixProject = {
      ...project,
      preferredVersion: "10.24.9",
      launchPending: true,
    };
    mocks.getProjects.mockResolvedValue([resumedProject]);
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");

    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-resume" }),
    );
    const assistant = await screen.findByRole("dialog");
    expect(
      within(assistant).getByLabelText("project-launch-select-version"),
    ).toHaveValue("10.24.9");
    fireEvent.click(
      within(assistant).getByLabelText("project-launch-mismatch-acknowledge"),
    );
    fireEvent.click(
      within(assistant).getByRole("button", { name: "project-launch-open" }),
    );

    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "10.24.9",
        resumedProject.mprPath,
      ),
    );
    expect(mocks.resolveDownloadableVersion).not.toHaveBeenCalled();
    expect(mocks.setProjectLaunchPreference).toHaveBeenLastCalledWith(
      resumedProject.mprPath,
      "10.24.9",
      false,
    );
  });

  it("resolves an unloaded exact version, installs it, verifies detection, and opens the original project", async () => {
    installed = [portableStudio];
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");

    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    await waitFor(() =>
      expect(mocks.resolveDownloadableVersion).toHaveBeenCalledWith("11.12.2"),
    );
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();
    expect(
      within(assistant).queryByText("project-launch-mismatch-title"),
    ).toBeNull();

    fireEvent.click(
      await within(assistant).findByRole("button", {
        name: "project-launch-install-open",
      }),
    );

    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.12.2", false),
    );
    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "11.12.2",
        project.mprPath,
      ),
    );
    expect(mocks.launchStudioPro).not.toHaveBeenCalledWith(
      "10.24.9",
      project.mprPath,
    );
    await waitFor(() =>
      expect(mocks.setProjectLaunchPreference).toHaveBeenLastCalledWith(
        project.mprPath,
        "11.12.2",
        false,
      ),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("does not open a project when installation returns without detecting the exact version", async () => {
    installed = [portableStudio];
    mocks.installStudioPro.mockResolvedValueOnce(undefined);
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    fireEvent.click(
      await within(assistant).findByRole("button", {
        name: "project-launch-install-open",
      }),
    );

    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.12.2", false),
    );
    expect(
      await within(assistant).findByText("project-launch-install-not-detected"),
    ).toBeVisible();
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();
    expect(mocks.setProjectLaunchPreference).not.toHaveBeenLastCalledWith(
      project.mprPath,
      "11.12.2",
      false,
    );
  });

  it("does not auto-open when download cancellation races with successful installation", async () => {
    installed = [portableStudio];
    let sendProgress: (progress: DownloadProgress) => void = () => {
      throw new Error("progress listener was not initialized");
    };
    mocks.onStudioDownloadProgress.mockImplementationOnce(async (listener) => {
      sendProgress = listener;
      return vi.fn();
    });
    let finishInstall: () => void = () => {
      throw new Error("installer resolver was not initialized");
    };
    mocks.installStudioPro.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          finishInstall = resolve;
        }),
    );
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    fireEvent.click(
      await within(assistant).findByRole("button", {
        name: "project-launch-install-open",
      }),
    );
    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledWith("11.12.2", false),
    );

    sendProgress({
      version: "11.12.2",
      state: "downloading",
      downloadedBytes: 512,
      totalBytes: 1024,
      percentage: 50,
      estimated: false,
      message: "downloading",
    });
    await within(assistant).findByText(/progress-downloading/);
    const cancel = within(assistant).getAllByRole("button", {
      name: "action-cancel",
    });
    fireEvent.click(cancel.find((button) => !button.hasAttribute("disabled"))!);
    await waitFor(() => expect(mocks.cancelStudioDownload).toHaveBeenCalled());

    installed = [...installed, removableStudio];
    finishInstall();
    await within(assistant).findByRole("button", {
      name: "project-launch-open",
    });
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeVisible();
  });

  it("requires an explicit remembered choice and safety acknowledgement for an unknown project version", async () => {
    const unknownProject: MendixProject = {
      ...project,
      version: undefined,
      preferredVersion: undefined,
      launchPending: false,
    };
    mocks.getProjects.mockResolvedValue([unknownProject]);
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");

    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    let assistant = await screen.findByRole("dialog");
    const versionSelect = within(assistant).getByLabelText(
      "project-launch-select-version",
    );
    expect(versionSelect).toHaveValue("");
    fireEvent.change(versionSelect, { target: { value: "10.24.9" } });
    expect(
      within(assistant).getByRole("button", { name: "project-launch-open" }),
    ).toBeDisabled();
    fireEvent.click(
      within(assistant).getByRole("button", { name: "action-cancel" }),
    );

    expect(
      await screen.findByRole("button", { name: "project-launch-resume" }),
    ).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-resume" }),
    );
    assistant = await screen.findByRole("dialog");
    expect(
      within(assistant).getByLabelText("project-launch-select-version"),
    ).toHaveValue("10.24.9");
    fireEvent.click(
      within(assistant).getByLabelText("project-launch-mismatch-acknowledge"),
    );
    fireEvent.click(
      within(assistant).getByRole("button", { name: "project-launch-open" }),
    );

    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "10.24.9",
        unknownProject.mprPath,
      ),
    );
    expect(mocks.resolveDownloadableVersion).not.toHaveBeenCalled();
  });

  it("preserves an exact launch intent through cancellation and failure, then resumes on retry", async () => {
    installed = [portableStudio];
    mocks.installStudioPro
      .mockRejectedValueOnce({
        code: "download_cancelled",
        message: "download cancelled",
      })
      .mockRejectedValueOnce({
        code: "install_failed",
        message: "installer failed",
      });
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    const continueButton = await within(assistant).findByRole("button", {
      name: "project-launch-install-open",
    });

    fireEvent.click(continueButton);
    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledTimes(1),
    );
    await waitFor(() => expect(continueButton).toBeEnabled());
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();
    expect(within(assistant).getByText("progress-cancelled")).toBeVisible();

    fireEvent.click(continueButton);
    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledTimes(2),
    );
    expect(within(assistant).getByText("installer failed")).toBeVisible();
    await waitFor(() => expect(continueButton).toBeEnabled());
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();

    fireEvent.click(continueButton);
    await waitFor(() =>
      expect(mocks.installStudioPro).toHaveBeenCalledTimes(3),
    );
    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "11.12.2",
        project.mprPath,
      ),
    );
    expect(mocks.setProjectLaunchPreference).toHaveBeenCalledWith(
      project.mprPath,
      "11.12.2",
      true,
    );
    expect(mocks.setProjectLaunchPreference).toHaveBeenLastCalledWith(
      project.mprPath,
      "11.12.2",
      false,
    );
  });

  it("does not fall back after exact lookup failure and gates an explicit mismatch choice", async () => {
    installed = [portableStudio];
    mocks.resolveDownloadableVersion.mockRejectedValueOnce({
      code: "operation_failed",
      message: "exact version unavailable",
    });
    await renderReadyApp();
    fireEvent.click(screen.getByRole("button", { name: /nav-projects/ }));
    await screen.findByText("Orders");
    fireEvent.click(
      screen.getByRole("button", { name: "project-launch-assist" }),
    );
    const assistant = await screen.findByRole("dialog");
    expect(
      await within(assistant).findByText("exact version unavailable"),
    ).toBeVisible();
    expect(mocks.launchStudioPro).not.toHaveBeenCalled();

    fireEvent.change(
      within(assistant).getByLabelText("project-launch-select-version"),
      { target: { value: "10.24.9" } },
    );
    const open = within(assistant).getByRole("button", {
      name: "project-launch-open",
    });
    expect(open).toBeDisabled();
    fireEvent.click(
      within(assistant).getByLabelText("project-launch-mismatch-acknowledge"),
    );
    expect(open).toBeEnabled();
    fireEvent.click(open);
    await waitFor(() =>
      expect(mocks.launchStudioPro).toHaveBeenCalledWith(
        "10.24.9",
        project.mprPath,
      ),
    );
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
    mocks.getStudioSessions.mockRejectedValue({
      code: "precondition_failed",
      message: "WinBoat Guest Server is offline.",
    });
    mocks.startWinBoatWindows.mockResolvedValue(undefined);

    render(<App />);
    await screen.findByText("route-linux");
    expect(screen.getByText("connection-offline")).toBeVisible();
    await waitFor(() => expect(mocks.getStudioSessions).toHaveBeenCalled());
    expect(screen.queryByText("studio-session-count")).toBeNull();
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
