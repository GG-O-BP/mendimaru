import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { StudioVersion } from "../../domain/types";
import { useInstalledVersions } from "./useInstalledVersions";

const api = vi.hoisted(() => ({
  getInstalledVersionsCache: vi.fn(),
  getInstalledVersions: vi.fn(),
  getStudioSessions: vi.fn(),
  launchStudioPro: vi.fn(),
  reconnectStudioSession: vi.fn(),
  stopStudioSession: vi.fn(),
  uninstallStudioPro: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({ tauriApi: api }));

function version(value: string): StudioVersion {
  return {
    version: value,
    displayName: `Studio Pro ${value}`,
    executablePath: `C:\\Program Files\\Mendix\\${value}\\modeler\\StudioPro.exe`,
    installRoot: `C:\\Program Files\\Mendix\\${value}`,
    source: "fixture",
    removable: true,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("useInstalledVersions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getInstalledVersionsCache.mockResolvedValue({ versions: [] });
    api.getStudioSessions.mockResolvedValue([]);
  });

  it("ignores a slower obsolete detection response", async () => {
    const initial = deferred<StudioVersion[]>();
    const latest = deferred<StudioVersion[]>();
    api.getInstalledVersions
      .mockImplementationOnce(() => initial.promise)
      .mockImplementationOnce(() => latest.promise);
    const dependencies = {
      t: (key: string) => key,
      installedVersionsSourceKey: "environment-a",
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    const { result } = renderHook(() => useInstalledVersions(dependencies));

    await waitFor(() =>
      expect(api.getInstalledVersions).toHaveBeenCalledTimes(1),
    );
    let latestRefresh!: Promise<StudioVersion[] | undefined>;
    act(() => {
      latestRefresh = result.current.refreshInstalled();
    });
    await waitFor(() =>
      expect(api.getInstalledVersions).toHaveBeenCalledTimes(2),
    );

    latest.resolve([version("11.12.3")]);
    await act(async () => {
      await latestRefresh;
    });
    expect(
      result.current.installedVersions.map(({ version }) => version),
    ).toEqual(["11.12.3"]);
    expect(result.current.installedLoaded).toBe(true);

    initial.resolve([version("10.24.24")]);
    await act(async () => {
      await initial.promise;
    });
    expect(
      result.current.installedVersions.map(({ version }) => version),
    ).toEqual(["11.12.3"]);
  });

  it("keeps cached versions visible but untrusted until live detection succeeds", async () => {
    const live = deferred<StudioVersion[]>();
    api.getInstalledVersionsCache.mockResolvedValue({
      versions: [version("11.6.9")],
      capturedAt: "2026-08-22T00:00:00Z",
    });
    api.getInstalledVersions.mockImplementation(() => live.promise);
    const { result } = renderHook(() =>
      useInstalledVersions({
        t: (key: string) => key,
        installedVersionsSourceKey: "environment-a",
        notify: vi.fn(),
        requestConfirmation: vi.fn(),
        runAction: async (_key: string, action: () => Promise<void>) =>
          action(),
        hasBusyPrefix: () => false,
        onWarning: vi.fn(),
      }),
    );

    await waitFor(() =>
      expect(result.current.installedVersions[0]?.version).toBe("11.6.9"),
    );
    expect(result.current.installedLoaded).toBe(false);
    expect(result.current.installedStale).toBe(true);

    live.resolve([version("11.6.9")]);
    await waitFor(() => expect(result.current.installedLoaded).toBe(true));
    expect(result.current.installedStale).toBe(false);
  });

  it("invalidates trusted versions immediately when the configured source changes", async () => {
    api.getInstalledVersionsCache
      .mockResolvedValueOnce({ versions: [version("11.6.9")] })
      .mockResolvedValueOnce({ versions: [] });
    api.getInstalledVersions
      .mockResolvedValueOnce([version("11.6.9")])
      .mockImplementationOnce(() => new Promise<StudioVersion[]>(() => {}));
    const dependencies = {
      t: (key: string) => key,
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    const { result, rerender } = renderHook(
      ({ sourceKey }) =>
        useInstalledVersions({
          ...dependencies,
          installedVersionsSourceKey: sourceKey,
        }),
      { initialProps: { sourceKey: "environment-a" } },
    );

    await waitFor(() => expect(result.current.installedLoaded).toBe(true));
    expect(result.current.installedVersions[0]?.version).toBe("11.6.9");

    rerender({ sourceKey: "environment-b" });

    await waitFor(() => expect(result.current.installedLoaded).toBe(false));
    expect(result.current.installedVersions).toEqual([]);
    expect(result.current.installedLoading).toBe(true);
  });

  it("stops loading after a live detection failure while keeping cached data untrusted", async () => {
    api.getInstalledVersionsCache.mockResolvedValue({
      versions: [version("11.6.9")],
    });
    api.getInstalledVersions.mockRejectedValue(new Error("guest unavailable"));
    const { result } = renderHook(() =>
      useInstalledVersions({
        t: (key: string) => key,
        installedVersionsSourceKey: "environment-a",
        notify: vi.fn(),
        requestConfirmation: vi.fn(),
        runAction: async (_key: string, action: () => Promise<void>) =>
          action(),
        hasBusyPrefix: () => false,
        onWarning: vi.fn(),
      }),
    );

    await waitFor(() =>
      expect(result.current.installedError).toBe("guest unavailable"),
    );
    expect(result.current.installedVersions[0]?.version).toBe("11.6.9");
    expect(result.current.installedLoading).toBe(false);
    expect(result.current.installedLoaded).toBe(false);
  });
});
