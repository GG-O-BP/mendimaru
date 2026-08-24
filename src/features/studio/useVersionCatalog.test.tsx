import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StudioVersionCatalog } from "../../domain/types";
import { useVersionCatalog } from "./useVersionCatalog";

const api = vi.hoisted(() => ({
  getDownloadableVersionsCache: vi.fn(),
  fetchDownloadableVersions: vi.fn(),
  resolveDownloadableVersion: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({ tauriApi: api }));

const cachedCatalog: StudioVersionCatalog = {
  versions: [
    {
      version: "11.12.3",
      isLts: false,
      isBeta: false,
      isMts: true,
      isLatest: true,
    },
  ],
  loadedPages: [1],
};

describe("useVersionCatalog", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    api.getDownloadableVersionsCache.mockResolvedValue(cachedCatalog);
    api.fetchDownloadableVersions.mockResolvedValue(cachedCatalog);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("hydrates a stale cache immediately and defers the live refresh", async () => {
    const { result } = renderHook(() =>
      useVersionCatalog({ t: (key: string) => key }),
    );

    await act(async () => {
      await Promise.resolve();
    });
    expect(result.current.catalog).toEqual(cachedCatalog);
    expect(api.fetchDownloadableVersions).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_999);
    });
    expect(api.fetchDownloadableVersions).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(api.fetchDownloadableVersions).toHaveBeenCalledWith(1, false);
  });

  it("does not launch a background refresh for a fresh cache", async () => {
    api.getDownloadableVersionsCache.mockResolvedValue({
      ...cachedCatalog,
      fetchedAt: new Date().toISOString(),
    });

    const { result } = renderHook(() =>
      useVersionCatalog({ t: (key: string) => key }),
    );

    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(result.current.catalog.versions).toEqual(cachedCatalog.versions);
    expect(api.fetchDownloadableVersions).not.toHaveBeenCalled();
  });

  it("cancels the deferred refresh when the consumer unmounts", async () => {
    const { unmount } = renderHook(() =>
      useVersionCatalog({ t: (key: string) => key }),
    );

    await act(async () => {
      await Promise.resolve();
    });
    unmount();
    await vi.advanceTimersByTimeAsync(5_000);

    expect(api.fetchDownloadableVersions).not.toHaveBeenCalled();
  });
});
