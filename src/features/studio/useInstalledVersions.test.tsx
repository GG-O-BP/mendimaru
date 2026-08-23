import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { StudioSessionStatus, StudioVersion } from "../../domain/types";
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

function session(): StudioSessionStatus {
  return {
    schemaVersion: "1.0.0",
    sessionId: "studio-4242-638908236000000000",
    version: "11.12.3",
    state: "running",
    processId: 4242,
    startedAt: "2026-08-15T03:00:00Z",
    connection: "connected",
    reconnectable: false,
    reconnectUnavailable: "already-connected",
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

  it("starts installed-version and session verification in parallel after loading the cache", async () => {
    const liveVersions = deferred<StudioVersion[]>();
    api.getInstalledVersionsCache.mockResolvedValue({
      versions: [version("11.12.3")],
    });
    api.getInstalledVersions.mockImplementation(() => liveVersions.promise);
    const dependencies = {
      t: (key: string) => key,
      installedVersionsSourceKey: "environment-a",
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    renderHook(() => useInstalledVersions(dependencies));

    await waitFor(() =>
      expect(api.getStudioSessions.mock.calls.length).toBeGreaterThan(0),
    );
    expect(api.getInstalledVersions).toHaveBeenCalledTimes(1);

    liveVersions.resolve([version("11.12.3")]);
    await act(async () => {
      await liveVersions.promise;
    });
  });

  it("waits for cold-start version detection before inspecting sessions", async () => {
    const liveVersions = deferred<StudioVersion[]>();
    api.getInstalledVersions.mockImplementation(() => liveVersions.promise);
    const dependencies = {
      t: (key: string) => key,
      installedVersionsSourceKey: "environment-a",
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    renderHook(() => useInstalledVersions(dependencies));

    await waitFor(() =>
      expect(api.getInstalledVersions).toHaveBeenCalledTimes(1),
    );
    expect(api.getStudioSessions).not.toHaveBeenCalled();

    liveVersions.resolve([version("11.12.3")]);
    await waitFor(() => expect(api.getStudioSessions).toHaveBeenCalledTimes(1));
  });

  it("rechecks sessions when live detection changes the cached Studio paths", async () => {
    const liveVersions = deferred<StudioVersion[]>();
    const firstSessions = deferred<StudioSessionStatus[]>();
    api.getInstalledVersionsCache.mockResolvedValue({
      versions: [version("11.6.9")],
    });
    api.getInstalledVersions.mockImplementation(() => liveVersions.promise);
    api.getStudioSessions
      .mockImplementationOnce(() => firstSessions.promise)
      .mockResolvedValueOnce([]);
    const dependencies = {
      t: (key: string) => key,
      installedVersionsSourceKey: "environment-a",
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    renderHook(() => useInstalledVersions(dependencies));

    await waitFor(() => expect(api.getStudioSessions).toHaveBeenCalledTimes(1));
    liveVersions.resolve([version("11.12.3")]);
    await act(async () => {
      await liveVersions.promise;
    });
    expect(api.getStudioSessions).toHaveBeenCalledTimes(1);

    firstSessions.resolve([]);
    await waitFor(() => expect(api.getStudioSessions).toHaveBeenCalledTimes(2));
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

  it("starts a session refresh for a newly configured source without waiting for the obsolete one", async () => {
    const obsoleteSessions = deferred<StudioSessionStatus[]>();
    api.getInstalledVersions.mockResolvedValue([version("11.12.3")]);
    api.getStudioSessions
      .mockImplementationOnce(() => obsoleteSessions.promise)
      .mockResolvedValueOnce([]);
    const dependencies = {
      t: (key: string) => key,
      notify: vi.fn(),
      requestConfirmation: vi.fn(),
      runAction: async (_key: string, action: () => Promise<void>) => action(),
      hasBusyPrefix: () => false,
      onWarning: vi.fn(),
    };
    const { rerender } = renderHook(
      ({ sourceKey }) =>
        useInstalledVersions({
          ...dependencies,
          installedVersionsSourceKey: sourceKey,
        }),
      { initialProps: { sourceKey: "environment-a" } },
    );

    await waitFor(() => expect(api.getStudioSessions).toHaveBeenCalledTimes(1));
    rerender({ sourceKey: "environment-b" });
    await waitFor(() => expect(api.getStudioSessions).toHaveBeenCalledTimes(2));

    obsoleteSessions.resolve([]);
    await act(async () => {
      await obsoleteSessions.promise;
    });
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

  it("automatically removes a session after Studio Pro exits", async () => {
    api.getInstalledVersions.mockResolvedValue([version("11.12.3")]);
    let sessions = [session()];
    api.getStudioSessions.mockImplementation(async () => [...sessions]);
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

    await waitFor(() => expect(result.current.sessions).toHaveLength(1));
    sessions = [];
    await waitFor(
      () => expect(api.getStudioSessions.mock.calls.length).toBeGreaterThan(1),
      {
        timeout: 2_500,
      },
    );
    await waitFor(() => expect(result.current.sessions).toEqual([]));
  });
});
