import { StrictMode, useState, type PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EnvironmentStatus } from "../../domain/types";
import { useWinBoatControl } from "./useWinBoatControl";

const api = vi.hoisted(() => ({
  startWinBoatWindows: vi.fn(),
  openWinBoat: vi.fn(),
}));
vi.mock("../../api/tauri", () => ({ tauriApi: api }));
const wrapper = ({ children }: PropsWithChildren) => (
  <StrictMode>{children}</StrictMode>
);
const t = (key: string) => key;
function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const stopped: EnvironmentStatus = {
  platform: {
    kind: "linux-winboat",
    architecture: "x86_64",
    requiresWinboat: true,
    supportsStudioManagement: true,
    supportsInstallation: true,
    supportsUninstallation: true,
    supportsProjects: true,
  },
  ready: false,
  winboatAvailable: true,
  winboatInitialized: true,
  setupPending: false,
  composeAvailable: true,
  runtimeAvailable: true,
  freerdpAvailable: true,
  sharedDirectoryAvailable: true,
  sharedMountMatches: true,
  containerStatus: "exited",
  guestOnline: false,
  diagnostics: [],
};
function setup() {
  const notify = vi.fn();
  const refreshStatus = vi.fn().mockResolvedValue(undefined);
  const onError = vi.fn();
  const runAction = async (_key: string, action: () => Promise<void>) => {
    try {
      await action();
    } catch (error) {
      onError(error);
    }
  };
  const hook = renderHook(
    ({ status }) => {
      const [startupPending, setStartupPending] = useState(false);
      return useWinBoatControl({
        t,
        notify,
        refreshStatus,
        runAction,
        isBusy: () => false,
        status,
        startupPending,
        setStartupPending,
        applyConfig: vi.fn(),
        updateConfigPair: vi.fn(),
      });
    },
    { wrapper, initialProps: { status: stopped } },
  );
  return { ...hook, notify, refreshStatus, onError };
}
describe("Windows startup lifecycle", () => {
  it("clears a local failure when a newer external readiness attempt succeeds", async () => {
    api.startWinBoatWindows.mockRejectedValueOnce({
      code: "guest_startup_timeout",
      message: "safe failure",
    });
    const { result, rerender } = setup();
    await act(() => result.current.startWindows());
    expect(result.current.lifecycle).toBe("startup-failed");
    rerender({
      status: {
        ...stopped,
        guestOnline: true,
        startup: {
          id: 2,
          startedAt: "2026-09-08T00:00:00Z",
          phase: "online",
          errorCode: null,
          containerStatus: "running",
        },
      },
    });
    expect(result.current.lifecycle).toBe("online");
  });
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("announces acceptance immediately, deduplicates clicks and waits for readiness before success", async () => {
    const request = deferred();
    api.startWinBoatWindows.mockReturnValue(request.promise);
    const { result, notify } = setup();
    let started!: Promise<void>;
    act(() => {
      started = result.current.startWindows();
      void result.current.startWindows();
    });
    expect(api.startWinBoatWindows).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith(
      "info",
      "toast-windows-start-requested",
    );
    expect(notify).not.toHaveBeenCalledWith("success", expect.anything());
    expect(result.current.lifecycle).toBe("starting-container");
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(notify).not.toHaveBeenCalledWith("success", expect.anything());
    await act(async () => {
      request.resolve();
      await started;
    });
    expect(notify).toHaveBeenCalledWith("success", "toast-windows-ready");
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(["container_exited_during_startup", "guest_startup_timeout"])(
    "retains %s across offline refresh and clears only on a successful retry",
    async (code) => {
      api.startWinBoatWindows.mockRejectedValueOnce({
        code,
        message: "safe startup reason",
      });
      const { result, rerender, notify } = setup();
      await act(() => result.current.startWindows());
      expect(result.current.lifecycle).toBe("startup-failed");
      expect(result.current.offlineGuidance.detail).toBe("safe startup reason");
      expect(result.current.winBoatControl.kind).toBe("open");
      rerender({ status: { ...stopped } });
      expect(result.current.offlineGuidance.title).toBe(
        "windows-startup-failed-title",
      );
      expect(notify).not.toHaveBeenCalledWith("success", expect.anything());
      api.startWinBoatWindows.mockResolvedValueOnce(undefined);
      await act(() => result.current.startWindows());
      expect(result.current.startupFailure).toBeNull();
      expect(notify).toHaveBeenCalledWith("success", "toast-windows-ready");
    },
  );

  it("does not publish notifications, refresh or timers after unmount", async () => {
    const request = deferred();
    api.startWinBoatWindows.mockReturnValue(request.promise);
    const { result, unmount, notify, refreshStatus } = setup();
    let started!: Promise<void>;
    act(() => {
      started = result.current.startWindows();
    });
    const refreshCount = refreshStatus.mock.calls.length;
    unmount();
    await act(async () => {
      request.resolve();
      await started;
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(notify).toHaveBeenCalledTimes(1);
    expect(refreshStatus).toHaveBeenCalledTimes(refreshCount);
    expect(vi.getTimerCount()).toBe(0);
  });
});
