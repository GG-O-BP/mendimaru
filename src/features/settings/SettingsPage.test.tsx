import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AppConfig } from "../../domain/types";
import type { Translate } from "../../i18n";
import { SettingsPage, type SettingsPageModel } from "./SettingsPage";

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

const t: Translate = (key, values) =>
  values ? `${key}:${Object.values(values).join(",")}` : key;

function model(overrides: Partial<SettingsPageModel> = {}): SettingsPageModel {
  return {
    config,
    nativeWindows: false,
    changed: false,
    mountMatches: false,
    applyNow: false,
    diagnostics: [
      {
        id: "container-runtime",
        status: "success",
        observed: "29.7.2",
      },
      {
        id: "guest-api",
        status: "failure",
        action: "open-winboat",
      },
      {
        id: "marketplace-browser",
        status: "warning",
        action: "redetect",
      },
    ],
    isBusy: () => false,
    onChange: vi.fn(),
    onChoose: vi.fn(),
    onAddStudioPath: vi.fn(),
    onRemoveStudioPath: vi.fn(),
    onApplyNow: vi.fn(),
    onSave: vi.fn(),
    onRedetect: vi.fn(),
    onDetectField: vi.fn(),
    onRestoreAdvancedDefaults: vi.fn(),
    onTestConnection: vi.fn(),
    connectionTest: null,
    onDiagnosticAction: vi.fn(),
    onCopyDiagnosticReport: vi.fn(),
    onExportDiagnosticReport: vi.fn(),
    ...overrides,
  };
}

describe("SettingsPage environment diagnostics", () => {
  it("keeps success, warning, and failure checks independent", () => {
    render(
      <SettingsPage
        t={t}
        model={model({ config: { ...config, freerdpBinary: "xfreerdp3" } })}
      />,
    );

    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByText("diagnostic-detected:29.7.2")).toBeVisible();
    expect(screen.getByText("diagnostic-status-success")).toBeVisible();
    expect(screen.getByText("diagnostic-status-warning")).toBeVisible();
    expect(screen.getByText("diagnostic-status-failure")).toBeVisible();
    expect(screen.getByText("diagnostic-guest-api-recovery")).toBeVisible();
  });

  it("dispatches only the selected safe recovery and report actions", () => {
    const onDiagnosticAction = vi.fn();
    const onCopyDiagnosticReport = vi.fn();
    const onExportDiagnosticReport = vi.fn();
    render(
      <SettingsPage
        t={t}
        model={model({
          onDiagnosticAction,
          onCopyDiagnosticReport,
          onExportDiagnosticReport,
        })}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "diagnostic-action-open-winboat" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "diagnostic-action-redetect" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "action-copy-diagnostic-report" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "action-export-diagnostic-report" }),
    );

    expect(onDiagnosticAction).toHaveBeenNthCalledWith(
      1,
      "open-winboat",
      "guest-api",
    );
    expect(onDiagnosticAction).toHaveBeenNthCalledWith(
      2,
      "redetect",
      "marketplace-browser",
    );
    expect(onCopyDiagnosticReport).toHaveBeenCalledOnce();
    expect(onExportDiagnosticReport).toHaveBeenCalledOnce();
  });
});

describe("SettingsPage advanced WinBoat settings", () => {
  it("keeps advanced controls collapsed until they are needed", () => {
    render(<SettingsPage t={t} model={model()} />);

    const advanced = screen.getByTestId("advanced-settings");
    expect(advanced).toHaveProperty("open", false);
    expect(screen.queryByTestId("advanced-apiUrl")).not.toBeVisible();
  });

  it("shows field errors and blocks connection tests until the draft is valid", () => {
    render(
      <SettingsPage
        t={t}
        model={model({ config: { ...config, freerdpBinary: "xfreerdp3" } })}
      />,
    );
    const advanced = screen.getByTestId("advanced-settings");
    fireEvent.click(advanced.querySelector("summary")!);

    expect(
      screen.getByText("error-settings-linux-absolute-path"),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "action-test-connection" }),
    ).toBeDisabled();
  });

  it("supports field-level detection, default restoration, and a non-mutating connection test", () => {
    const onDetectField = vi.fn();
    const onRestoreAdvancedDefaults = vi.fn();
    const onTestConnection = vi.fn();
    render(
      <SettingsPage
        t={t}
        model={model({
          onDetectField,
          onRestoreAdvancedDefaults,
          onTestConnection,
          connectionTest: { online: true, endpoint: "http://127.0.0.1:47280" },
        })}
      />,
    );
    fireEvent.click(
      screen.getByTestId("advanced-settings").querySelector("summary")!,
    );

    fireEvent.click(
      screen
        .getByTestId("advanced-containerName")
        .querySelector('button[type="button"]')!,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "action-restore-defaults" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "action-test-connection" }),
    );

    expect(onDetectField).toHaveBeenCalledWith("containerName");
    expect(onRestoreAdvancedDefaults).toHaveBeenCalledOnce();
    expect(onTestConnection).toHaveBeenCalledOnce();
    expect(screen.getByTestId("connection-test-result")).toHaveTextContent(
      "settings-connection-online",
    );
  });
});
