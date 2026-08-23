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

  it("hydrates the cache immediately and defers the initial live refresh", async () => {
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
