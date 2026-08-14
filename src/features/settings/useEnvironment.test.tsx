import { StrictMode, type PropsWithChildren } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { Translate } from "../../i18n";
import { useEnvironment } from "./useEnvironment";

const api = vi.hoisted(() => ({
  getConfig: vi.fn(),
  getEnvironmentStatus: vi.fn(),
  startWinBoatWindows: vi.fn(),
  openWinBoat: vi.fn(),
  beginWinBoatSetup: vi.fn(),
  completeWinBoatSetup: vi.fn(),
  saveConfig: vi.fn(),
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
  freerdpBinary: "xfreerdp3",
  mendixInstallRoot: String.raw`C:\Program Files\Mendix`,
  mendixDataRoot: String.raw`C:\ProgramData\Mendix`,
  startupTimeoutSeconds: 180,
};

const status: EnvironmentStatus = {
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
};

const t: Translate = (key) => key;
const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);

describe("useEnvironment initialization", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getConfig.mockResolvedValue(config);
    api.getEnvironmentStatus.mockResolvedValue(status);
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
});
