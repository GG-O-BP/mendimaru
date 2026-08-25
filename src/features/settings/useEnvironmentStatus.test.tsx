import { StrictMode, type PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EnvironmentStatus } from "../../domain/types";
import { useEnvironmentStatus } from "./useEnvironmentStatus";

const api = vi.hoisted(() => ({
  getEnvironmentStatus: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({ tauriApi: api }));

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

const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);
const t = (key: string) => key;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function runInitialTimer() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

describe("useEnvironmentStatus polling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    api.getEnvironmentStatus.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps one request in flight and schedules one refresh after completion", async () => {
    const first = deferred<EnvironmentStatus>();
    api.getEnvironmentStatus.mockReturnValueOnce(first.promise);
    const onWarning = vi.fn();
    const { result } = renderHook(
      () => useEnvironmentStatus({ t, onWarning }),
      { wrapper },
    );

    await runInitialTimer();
    expect(api.getEnvironmentStatus).toHaveBeenCalledOnce();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(api.getEnvironmentStatus).toHaveBeenCalledOnce();

    api.getEnvironmentStatus.mockResolvedValueOnce(status);
    await act(async () => {
      first.resolve(status);
      await Promise.resolve();
    });
    expect(result.current.status).toEqual(status);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(14_999);
    });
    expect(api.getEnvironmentStatus).toHaveBeenCalledOnce();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(api.getEnvironmentStatus).toHaveBeenCalledTimes(2);
  });

  it("discards an old source response and serializes its replacement", async () => {
    const oldRequest = deferred<EnvironmentStatus>();
    const oldStatus = { ...status, ready: false };
    const newStatus = { ...status, guestOnline: false };
    api.getEnvironmentStatus
      .mockReturnValueOnce(oldRequest.promise)
      .mockResolvedValueOnce(newStatus);
    const { result } = renderHook(
      () => useEnvironmentStatus({ t, onWarning: vi.fn() }),
      { wrapper },
    );
    await runInitialTimer();

    let refreshed!: Promise<void>;
    act(() => {
      refreshed = result.current.refreshStatus({ sourceChanged: true });
    });
    expect(api.getEnvironmentStatus).toHaveBeenCalledOnce();

    await act(async () => {
      oldRequest.resolve(oldStatus);
      await refreshed;
    });

    expect(api.getEnvironmentStatus).toHaveBeenCalledTimes(2);
    expect(result.current.status).toEqual(newStatus);
    expect(result.current.status).not.toEqual(oldStatus);
  });

  it("deduplicates timeout warnings and clears one after manual recovery", async () => {
    const onWarning = vi.fn();
    api.getEnvironmentStatus.mockRejectedValueOnce(
      new Error("probe timed out"),
    );
    const { result } = renderHook(
      () => useEnvironmentStatus({ t, onWarning }),
      { wrapper },
    );
    await runInitialTimer();
    expect(onWarning).toHaveBeenCalledWith("probe timed out");

    api.getEnvironmentStatus.mockRejectedValueOnce(
      new Error("probe timed out"),
    );
    await act(() => result.current.refreshStatus());
    expect(onWarning).toHaveBeenCalledTimes(1);

    api.getEnvironmentStatus.mockResolvedValueOnce(status);
    await act(() => result.current.refreshStatus());
    expect(result.current.status).toEqual(status);
    expect(onWarning).toHaveBeenLastCalledWith(null);
    expect(onWarning).toHaveBeenCalledTimes(2);
  });
});
