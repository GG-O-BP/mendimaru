import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import type { FrontendHealth } from "../../domain/types";
import { tauriApi } from "../../api/tauri";
import { FrontendHealthPanel } from "./FrontendHealthPanel";
vi.mock("../../api/tauri", () => ({
  tauriApi: { diagnoseFrontendHealth: vi.fn() },
}));
const t = (key: string) => key;
const report: FrontendHealth = {
  schemaVersion: "5.0.0",
  frontendState: "unhealthy",
  studioState: "running",
  httpReady: true,
  runtimeSessionId: `runtime_${"1".repeat(32)}`,
  startedAt: "2026-09-16T00:00:00Z",
  finishedAt: "2026-09-16T00:00:03Z",
  navigationComplete: true,
  documentStatus: 200,
  observationMilliseconds: 3000,
  assetBypass: false,
  counts: {
    pageErrors: 1,
    consoleErrors: 0,
    failedRequests: 1,
    httpErrors: 0,
    errorDialogs: 1,
  },
  truncated: false,
  diagnostics: [
    {
      code: "shared_unc_asset_unreachable",
      action: "Check generated imports",
      occurrences: 1,
      endpoint: {
        scheme: "http",
        hostKind: "shared-unc",
        port: 80,
        pathKind: "shared-deployment",
      },
      failure: "dns_failure",
    },
  ],
};
beforeEach(() => vi.resetAllMocks());
it("runs only on request and distinguishes Studio, HTTP and frontend states", async () => {
  vi.mocked(tauriApi.diagnoseFrontendHealth).mockResolvedValue(report);
  render(<FrontendHealthPanel t={t} />);
  expect(tauriApi.diagnoseFrontendHealth).not.toHaveBeenCalled();
  expect(screen.getAllByText("frontend-not-checked")).toHaveLength(3);
  fireEvent.change(screen.getByRole("textbox"), {
    target: { value: report.runtimeSessionId },
  });
  fireEvent.click(screen.getByText("frontend-run"));
  await screen.findByText("frontend-unhealthy");
  expect(screen.getByText("frontend-studio-running")).toBeInTheDocument();
  expect(screen.getByText("frontend-ready")).toBeInTheDocument();
  expect(
    screen.getByText("frontend-cause-shared-unc-asset-unreachable"),
  ).toBeInTheDocument();
  expect(screen.getByText(/shared-unc.*dns_failure/)).toBeInTheDocument();
  expect(tauriApi.diagnoseFrontendHealth).toHaveBeenCalledExactlyOnceWith(
    report.runtimeSessionId,
  );
  fireEvent.change(screen.getByRole("textbox"), {
    target: { value: "http://localhost:8081" },
  });
  expect(screen.queryByText("frontend-unhealthy")).not.toBeInTheDocument();
});
it("prevents concurrent runs and rejects results from an unmounted panel", async () => {
  let resolve!: (report: FrontendHealth) => void;
  vi.mocked(tauriApi.diagnoseFrontendHealth).mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = render(<FrontendHealthPanel t={t} />);
  fireEvent.change(screen.getByRole("textbox"), {
    target: { value: "http://localhost:8080" },
  });
  fireEvent.click(screen.getByText("frontend-run"));
  expect(screen.getByRole("textbox")).toBeDisabled();
  fireEvent.click(screen.getByText("frontend-checking"));
  expect(tauriApi.diagnoseFrontendHealth).toHaveBeenCalledTimes(1);
  view.unmount();
  render(<FrontendHealthPanel t={t} />);
  await act(async () => resolve(report));
  expect(screen.queryByText("frontend-unhealthy")).not.toBeInTheDocument();
});
it("reports prerequisite failures without treating an unobserved page as healthy", async () => {
  vi.mocked(tauriApi.diagnoseFrontendHealth).mockRejectedValue(
    new Error("private path"),
  );
  render(<FrontendHealthPanel t={t} />);
  fireEvent.change(screen.getByRole("textbox"), {
    target: { value: "http://localhost:8080" },
  });
  fireEvent.click(screen.getByText("frontend-run"));
  await waitFor(() =>
    expect(screen.getByText("frontend-failed")).toBeInTheDocument(),
  );
  expect(screen.getAllByText("frontend-not-checked")).toHaveLength(3);
  expect(screen.queryByText("private path")).not.toBeInTheDocument();
});
