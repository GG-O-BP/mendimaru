import { StrictMode, type PropsWithChildren } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { Translate } from "../../i18n";
import { useEnvironment } from "./useEnvironment";

const api = vi.hoisted(() => ({
  getConfig: vi.fn(),
  getEnvironmentStatus: vi.fn(),
  getEnvironmentDiagnosticReport: vi.fn(),
  exportEnvironmentDiagnosticReport: vi.fn(),
  startWinBoatWindows: vi.fn(),
  openWinBoat: vi.fn(),
  beginWinBoatSetup: vi.fn(),
  completeWinBoatSetup: vi.fn(),
  previewSettingsSave: vi.fn(),
  saveConfig: vi.fn(),
  detectSettings: vi.fn(),
  testSettingsConnection: vi.fn(),
  redetectConfig: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({ tauriApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const config: AppConfig = {
  languagePreference: "system",
  winboatSetupPending: false,
  winboatExecutable: "/usr/bin/winboat",
  composeFile: "/home/dev/.winboat/docker-compose.yml",
  containerRuntime: "docker",
  containerName: "WinBoat",
  apiUrl: "http://127.0.0.1:47280",
  rdpHost: "127.0.0.1",
  rdpPort: 47300,
  sharedDirectory: "/home/dev/Mendix",
  windowsSharedDirectory: String.raw`\\host.lan\Data`,
  freerdpBinary: "/usr/bin/xfreerdp3",
  mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
  mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
  windowsStudioPaths: [],
  startupTimeoutSeconds: 180,
};

const status: EnvironmentStatus = {
  platform: {
    kind: "linux-winboat",
    architecture: "x86_64",
    requiresWinboat: true,
    supportsStudioManagement: true,
    supportsInstallation: true,
    supportsUninstallation: true,
    supportsProjects: true,
  },
  ready: true,
  winboatAvailable: true,
  winboatInitialized: true,
  setupPending: false,
  composeAvailable: true,
  runtimeAvailable: true,
  freerdpAvailable: true,
  sharedDirectoryAvailable: true,
  sharedMountMatches: true,
  containerStatus: "running",
  guestOnline: true,
  diagnostics: [],
};

const t: Translate = (key, values) =>
  values ? `${key}:${Object.values(values).join(",")}` : key;
const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);

describe("useEnvironment initialization", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getConfig.mockResolvedValue(config);
    api.getEnvironmentStatus.mockResolvedValue(status);
    api.startWinBoatWindows.mockResolvedValue(undefined);
    api.openWinBoat.mockResolvedValue(undefined);
    api.getEnvironmentDiagnosticReport.mockResolvedValue(
      '{"schemaVersion":"1.0.0"}',
    );
    api.exportEnvironmentDiagnosticReport.mockResolvedValue(true);
    api.detectSettings.mockResolvedValue(config);
    api.testSettingsConnection.mockResolvedValue({
      online: true,
      endpoint: config.apiUrl,
    });
  });

  it("finishes loading when React StrictMode replays mount effects", async () => {
    const { result } = renderHook(
      () =>
        useEnvironment({
          t,
          notify: vi.fn(),
          requestConfirmation: vi.fn(),
          runAction: async (_key, action) => action(),
          isBusy: () => false,
          onWarning: vi.fn(),
        }),
      { wrapper },
    );

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.config).toEqual(config);
    expect(result.current.online).toBe(true);
    expect(api.getConfig).toHaveBeenCalled();
  });

  it("routes diagnostic recovery and redacted report actions through the shared action lock", async () => {
    const notify = vi.fn();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const { result } = renderHook(
      () =>
        useEnvironment({
          t,
          notify,
          requestConfirmation: vi.fn(),
          runAction: async (_key, action) => action(),
          isBusy: () => false,
          onWarning: vi.fn(),
        }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(() =>
      result.current.runDiagnosticAction("start-winboat", "container"),
    );
    await act(() =>
      result.current.runDiagnosticAction("open-winboat", "guest-api"),
    );
    await act(() => result.current.copyDiagnosticReport());
    await act(() => result.current.exportDiagnosticReport());

    expect(api.startWinBoatWindows).toHaveBeenCalledOnce();
    expect(api.openWinBoat).toHaveBeenCalledOnce();
    expect(writeText).toHaveBeenCalledWith('{"schemaVersion":"1.0.0"}');
    expect(api.exportEnvironmentDiagnosticReport).toHaveBeenCalledOnce();
    expect(notify).toHaveBeenCalledWith(
      "success",
      "toast-diagnostic-report-copied",
    );
    expect(notify).toHaveBeenCalledWith(
      "success",
      "toast-diagnostic-report-exported",
    );
  });

  it("updates one advanced draft field and tests the draft without saving it", async () => {
    const detected = { ...config, apiUrl: "http://127.0.0.1:47282" };
    api.detectSettings.mockResolvedValue(detected);
    api.testSettingsConnection.mockResolvedValue({
      online: true,
      endpoint: detected.apiUrl,
    });
    const { result } = renderHook(
      () =>
        useEnvironment({
          t,
          notify: vi.fn(),
          requestConfirmation: vi.fn(),
          runAction: async (_key, action) => action(),
          isBusy: () => false,
          onWarning: vi.fn(),
        }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(() => result.current.detectAdvancedSetting("apiUrl"));
    await act(() => result.current.testConnection());

    expect(result.current.draftConfig?.apiUrl).toBe(detected.apiUrl);
    expect(result.current.config?.apiUrl).toBe(config.apiUrl);
    expect(api.testSettingsConnection).toHaveBeenCalledWith(
      expect.objectContaining({ apiUrl: detected.apiUrl }),
    );
    expect(api.saveConfig).not.toHaveBeenCalled();
    expect(result.current.connectionTest).toEqual({
      online: true,
      endpoint: detected.apiUrl,
    });
  });

  it("previews the exact Compose service, mount diff, and recreate scope before saving", async () => {
    const requestConfirmation = vi.fn();
    api.previewSettingsSave.mockResolvedValue({
      serviceName: "windows-vm",
      currentSharedDirectory: "${HOME}/Mendix",
      nextSharedDirectory: "/home/dev/NewMendix",
      mountChanged: true,
      containerWillRecreate: true,
      composeRevision: "sha256-preview",
    });
    api.saveConfig.mockImplementation(async (nextConfig: AppConfig) => ({
      config: nextConfig,
      mountChanged: true,
      containerRecreated: true,
    }));
    const { result } = renderHook(
      () =>
        useEnvironment({
          t,
          notify: vi.fn(),
          requestConfirmation,
          runAction: async (_key, action) => action(),
          isBusy: () => false,
          onWarning: vi.fn(),
        }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      result.current.setDraftConfig({
        ...config,
        sharedDirectory: "/home/dev/NewMendix",
      });
    });
    await waitFor(() =>
      expect(result.current.draftConfig?.sharedDirectory).toBe(
        "/home/dev/NewMendix",
      ),
    );
    act(() => result.current.saveSettings());

    await waitFor(() => expect(requestConfirmation).toHaveBeenCalledOnce());
    const confirmation = requestConfirmation.mock.calls[0]?.[0];
    expect(confirmation.title).toBe("confirm-mount-change-title");
    expect(confirmation.description).toContain("windows-vm");
    expect(confirmation.description).toContain("${HOME}/Mendix");
    expect(confirmation.description).toContain("/home/dev/NewMendix");
    expect(confirmation.description).toContain("mount-preview-service-scope");
    expect(api.saveConfig).not.toHaveBeenCalled();

    await act(() => confirmation.action());
    expect(api.saveConfig).toHaveBeenCalledWith(
      expect.objectContaining({ sharedDirectory: "/home/dev/NewMendix" }),
      true,
      "sha256-preview",
    );
  });
});
