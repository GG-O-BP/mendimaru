import { act, renderHook, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MendixProject, ProjectScanResult } from "../../domain/types";
import { useProjects } from "./useProjects";

const api = vi.hoisted(() => ({
  getProjects: vi.fn(),
  selectExternalProject: vi.fn(),
  setProjectFavorite: vi.fn(),
  openFolder: vi.fn(),
  onWorkspaceProjectsChanged: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({
  tauriApi: api,
}));

function deferredProjectScan(label: string) {
  let resolve!: (result: ProjectScanResult) => void;
  const promise = new Promise<ProjectScanResult>((next) => {
    resolve = next;
  });
  return { label, promise, resolve };
}

function project(name: string, overrides: Partial<MendixProject> = {}) {
  return {
    name,
    directory: `/workspace/${name}`,
    mprPath: `/workspace/${name}/${name}.mpr`,
    windowsPath: `\\\\host\\Data\\${name}\\${name}.mpr`,
    location: "configured-workspace",
    launchPending: false,
    favorite: false,
    lastModified: "2026-08-01T00:00:00Z",
    ...overrides,
  } satisfies MendixProject;
}

function scan(
  sourceKey: string,
  projects: MendixProject[],
  overrides: Partial<ProjectScanResult> = {},
) {
  return {
    sourceKey,
    projects,
    visitedEntries: projects.length,
    skippedEntries: 0,
    errorCount: 0,
    errors: [],
    settingsBytesRead: 0,
    truncated: false,
    durationMs: 1,
    watcherActive: false,
    ...overrides,
  } satisfies ProjectScanResult;
}

function dependencies(sharedDirectory = "/workspace/a") {
  return {
    t: (key: string) => key,
    sharedDirectory,
    onWarning: vi.fn(),
    runAction: vi.fn(async (_key: string, action: () => Promise<void>) =>
      action(),
    ),
  };
}

describe("useProjects", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
    api.getProjects.mockReset();
    api.setProjectFavorite.mockResolvedValue(undefined);
    api.onWorkspaceProjectsChanged.mockResolvedValue(vi.fn());
  });

  it("invalidates an old workspace immediately and discards its late response", async () => {
    const first = deferredProjectScan("a");
    const second = deferredProjectScan("b");
    api.getProjects
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const { rerender, result } = renderHook(
      ({ sharedDirectory }) => useProjects(dependencies(sharedDirectory)),
      { initialProps: { sharedDirectory: "/workspace/a" } },
    );

    await waitFor(() => expect(api.getProjects).toHaveBeenCalledTimes(1));
    rerender({ sharedDirectory: "/workspace/b" });
    expect(result.current.scanStatus).toBe("stale");
    await waitFor(() => expect(api.getProjects).toHaveBeenCalledTimes(2));

    await act(async () => {
      second.resolve(
        scan("/workspace/b", [project("New", { favorite: true })]),
      );
      await Promise.resolve();
    });
    expect(result.current.projects.map((item) => item.name)).toEqual(["New"]);

    await act(async () => {
      first.resolve(scan("/workspace/a", [project("Old")]));
      await Promise.resolve();
    });
    expect(result.current.projects.map((item) => item.name)).toEqual(["New"]);
  });

  it("coalesces watcher bursts into one delayed refresh", async () => {
    vi.useFakeTimers();
    let emit!: () => void;
    api.onWorkspaceProjectsChanged.mockImplementation(
      async (listener: () => void) => {
        emit = listener;
        return vi.fn();
      },
    );
    api.getProjects
      .mockResolvedValueOnce(scan("/workspace/a", [project("Orders")]))
      .mockResolvedValueOnce(scan("/workspace/a", [project("Changed")]));
    const { result } = renderHook(() => useProjects(dependencies()), {
      wrapper: StrictMode,
    });

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(api.getProjects).toHaveBeenCalledTimes(1);

    act(() => {
      emit();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
      emit();
      await vi.advanceTimersByTimeAsync(300);
      emit();
      await vi.advanceTimersByTimeAsync(499);
    });
    expect(api.getProjects).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(api.getProjects).toHaveBeenCalledTimes(2);
    expect(result.current.projects.map((item) => item.name)).toEqual([
      "Changed",
    ]);
  });

  it("combines search, favorites, sorting, and incremental rendering", async () => {
    const projects = [
      project("Zulu", {
        version: "10.24.9",
        lastModified: "2026-01-01T00:00:00Z",
      }),
      project("Alpha", {
        version: "11.12.2",
        favorite: true,
        lastLaunchedAt: "2026-08-01T00:00:00Z",
      }),
    ];
    api.getProjects.mockResolvedValue(
      scan("/workspace/a", [
        ...projects,
        ...Array.from({ length: 99 }, (_, index) => project(`Bulk ${index}`)),
      ]),
    );
    const { result } = renderHook(() => useProjects(dependencies()));

    await waitFor(() => expect(result.current.scanStatus).toBe("ready"));
    expect(result.current.filteredProjects).toHaveLength(100);
    expect(result.current.hasMoreProjects).toBe(true);

    act(() => {
      result.current.setSearch("al");
      result.current.setFavoriteOnly(true);
      result.current.setSortKey("recent");
      result.current.showMoreProjects();
    });
    expect(result.current.filteredProjects.map((item) => item.name)).toEqual([
      "Alpha",
    ]);
    expect(result.current.hasMoreProjects).toBe(false);
  });

  it("uses the periodic fallback when the backend reports no active watcher", async () => {
    vi.useFakeTimers();
    api.getProjects
      .mockResolvedValueOnce(scan("/workspace/a", [project("Orders")]))
      .mockResolvedValueOnce(scan("/workspace/a", [project("Fallback")]));
    const { result } = renderHook(() => useProjects(dependencies()));

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(api.getProjects).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(api.getProjects).toHaveBeenCalledTimes(2);
    expect(result.current.projects.map((item) => item.name)).toEqual([
      "Fallback",
    ]);
  });
});
