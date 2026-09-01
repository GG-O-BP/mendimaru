import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  LocalizationBundle,
  MendixProject,
  ProjectScanResult,
} from "../../domain/types";
import { ProjectsPage, type ProjectsPageModel } from "./ProjectsPage";

const api = vi.hoisted(() => ({
  formatDates: vi.fn(),
  formatNumbers: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({
  tauriApi: api,
}));

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [],
  messages: {},
  numbers: [],
};

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

function model(overrides: Partial<ProjectsPageModel> = {}) {
  const projects = [project("Orders", { favorite: true })];
  return {
    projects,
    totalProjects: projects.length,
    totalVisibleProjects: projects.length,
    hasMoreProjects: false,
    search: "",
    favoriteOnly: false,
    sortKey: "modified",
    scanStatus: "ready",
    sharedDirectory: "/workspace",
    installedSet: new Set(["11.12.2"]),
    installedVersionsLoaded: true,
    studioLaunchReady: true,
    studioSessionsLoading: false,
    supportsExternalSelection: false,
    externalSelectionBusy: false,
    isLaunching: false,
    isBusy: () => false,
    launchKeyFor: () => "launch-test",
    preferredVersionFor: () => undefined,
    launchPendingFor: () => false,
    onSearch: vi.fn(),
    onFavoriteOnly: vi.fn(),
    onSortKey: vi.fn(),
    onShowMoreProjects: vi.fn(),
    onToggleFavorite: vi.fn(),
    onRefresh: vi.fn(),
    onOpenWorkspace: vi.fn(),
    onOpenFolder: vi.fn(),
    onSelectExternal: vi.fn(),
    onLaunch: vi.fn(),
    ...overrides,
  } satisfies ProjectsPageModel;
}

function renderPage(nextModel = model()) {
  render(
    <ProjectsPage
      t={(key) => key}
      localization={localization}
      model={nextModel}
    />,
  );
}

describe("ProjectsPage", () => {
  beforeEach(() => {
    api.formatDates.mockImplementation(async (values: string[]) => values);
    api.formatNumbers.mockImplementation(async (values: number[]) =>
      values.map(String),
    );
  });

  it("renders favorites, sorting, last-open metadata, and incremental results", () => {
    const scan = {
      sourceKey: "/workspace",
      projects: [],
      visitedEntries: 101,
      skippedEntries: 2,
      errorCount: 1,
      errors: ["permission denied"],
      settingsBytesRead: 10,
      truncated: true,
      durationMs: 12,
      watcherActive: true,
    } satisfies ProjectScanResult;
    const onShowMoreProjects = vi.fn();
    renderPage(
      model({
        projects: Array.from({ length: 101 }, (_, index) =>
          project(`Project ${index}`),
        ),
        totalProjects: 101,
        totalVisibleProjects: 101,
        hasMoreProjects: true,
        scan,
        onShowMoreProjects,
      }),
    );

    expect(screen.getByText("project-column-recent")).toBeVisible();
    expect(screen.getByText("projects-favorite-filter")).toBeVisible();
    expect(screen.getByLabelText("projects-sort-label")).toBeVisible();
    expect(screen.getByText("projects-scan-partial")).toBeVisible();
    expect(screen.getByText("projects-scan-partial-detail")).toBeVisible();
    expect(screen.getByText("permission denied")).toBeVisible();
    fireEvent.click(screen.getByText("projects-show-more"));
    expect(onShowMoreProjects).toHaveBeenCalledTimes(1);
  });

  it("shows stale and complete failure states", () => {
    renderPage(model({ scanStatus: "stale" }));
    expect(screen.getByText("projects-scan-stale")).toBeVisible();

    renderPage(
      model({ scanStatus: "error", scanError: "workspace unavailable" }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("projects-scan-error");
    expect(screen.getByText("workspace unavailable")).toBeVisible();
  });

  it("toggles a discovered favorite and disables session-only favorites", () => {
    const external = project("External", {
      location: "explicit-host-selection",
    });
    const onToggleFavorite = vi.fn();
    renderPage(
      model({
        projects: [project("Orders", { favorite: true }), external],
        onToggleFavorite,
      }),
    );

    const rows = screen.getAllByRole("row");
    fireEvent.click(within(rows[1]).getByTitle("project-favorite-remove"));
    expect(onToggleFavorite).toHaveBeenCalledTimes(1);
    expect(
      within(rows[2]).getByTitle("project-favorite-external-disabled"),
    ).toBeDisabled();
  });
});
