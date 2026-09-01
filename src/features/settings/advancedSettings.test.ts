import { describe, expect, it } from "vitest";
import type { AppConfig } from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  advancedSettingsAreValid,
  validateAdvancedSettings,
} from "./advancedSettings";

const config: AppConfig = {
  languagePreference: "system",
  winboatSetupPending: false,
  winboatExecutable: "/opt/winboat/winboat",
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

const t: Translate = (key) => key;

describe("validateAdvancedSettings", () => {
  it("accepts the complete loopback-only operating configuration", () => {
    expect(advancedSettingsAreValid(config, t)).toBe(true);
  });

  it("rejects public endpoints, unsafe ports, and relative paths near their fields", () => {
    const errors = validateAdvancedSettings(
      {
        ...config,
        containerName: "Bad Name",
        apiUrl: "http://user:secret@192.168.1.10:47280",
        rdpHost: "192.168.1.10",
        rdpPort: 65536,
        windowsSharedDirectory: "Data",
        freerdpBinary: "xfreerdp3",
        mendixInstallRoot: "C:Mendix",
        startupTimeoutSeconds: 901,
      },
      t,
    );

    expect(errors).toEqual({
      containerName: "error-settings-container-name",
      apiUrl: "error-settings-loopback-url",
      rdpHost: "error-settings-loopback-host",
      rdpPort: "error-settings-port-range",
      windowsSharedDirectory: "error-settings-windows-absolute-path",
      freerdpBinary: "error-settings-linux-absolute-path",
      mendixInstallRoot: "error-settings-windows-absolute-path",
      startupTimeoutSeconds: "error-settings-timeout-range",
    });
  });
});
