import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { InstallQueueItem } from "../../domain/types";
import { useInstallQueue } from "./useInstallQueue";

const api = vi.hoisted(() => ({
  getInstallQueue: vi.fn(),
  onInstallQueueChanged: vi.fn(),
  removeInstallQueueItem: vi.fn(),
}));
vi.mock("../../api/tauri", () => ({ tauriApi: api }));

const item: InstallQueueItem = {
  id: "install-queue-test",
  version: "11.12.6",
  forceRedownload: false,
  state: "succeeded",
  createdAt: "2026-10-14T00:00:00Z",
  updatedAt: "2026-10-14T00:00:00Z",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function dependencies() {
  return {
    t: (key: string) => key,
    notify: vi.fn(),
    requestConfirmation: vi.fn(),
    runAction: async (_key: string, action: () => Promise<void>) => action(),
    hasBusyPrefix: () => false,
    onWarning: vi.fn(),
  };
}

describe("useInstallQueue", () => {
  let changed: (items: InstallQueueItem[]) => void;
  let stop: ReturnType<typeof vi.fn<() => void>>;
  beforeEach(() => {
    vi.clearAllMocks();
    stop = vi.fn();
    api.onInstallQueueChanged.mockImplementation(async (handler) => {
      changed = handler;
      return stop;
    });
    api.getInstallQueue.mockResolvedValue([item]);
  });

  it("subscribes before reading the initial queue", async () => {
    const ready = deferred<() => void>();
    api.onInstallQueueChanged.mockReturnValue(ready.promise);
    const deps = dependencies();
    const { result, unmount } = renderHook(() => useInstallQueue(deps));
    expect(api.getInstallQueue).not.toHaveBeenCalled();
    await act(async () => ready.resolve(stop));
    await waitFor(() => expect(result.current.items).toEqual([item]));
    unmount();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("does not restore a removed row from an older initial response", async () => {
    const initial = deferred<InstallQueueItem[]>();
    api.getInstallQueue.mockReturnValue(initial.promise);
    const deps = dependencies();
    const { result } = renderHook(() => useInstallQueue(deps));
    await waitFor(() => expect(api.getInstallQueue).toHaveBeenCalledOnce());
    act(() => changed([]));
    await act(async () => initial.resolve([item]));
    expect(result.current.items).toEqual([]);
  });

  it("removes the row when the backend publishes a completed removal", async () => {
    const deps = dependencies();
    const { result } = renderHook(() => useInstallQueue(deps));
    await waitFor(() => expect(result.current.items).toEqual([item]));
    api.removeInstallQueueItem.mockImplementation(async () => changed([]));
    await act(async () => result.current.removeItem(item.id));
    expect(result.current.items).toEqual([]);
    expect(api.removeInstallQueueItem).toHaveBeenCalledWith(item.id);
  });

  it("releases a listener that finishes subscribing after unmount", async () => {
    const ready = deferred<() => void>();
    api.onInstallQueueChanged.mockReturnValue(ready.promise);
    const deps = dependencies();
    const { unmount } = renderHook(() => useInstallQueue(deps));
    unmount();
    await act(async () => ready.resolve(stop));
    expect(stop).toHaveBeenCalledOnce();
    expect(api.getInstallQueue).not.toHaveBeenCalled();
  });
});
