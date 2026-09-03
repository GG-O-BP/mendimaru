import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type {
  DownloadableVersion,
  LocalizationBundle,
  StudioVersion,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import { VersionCatalogTable } from "./VersionCatalogTable";

const localization: LocalizationBundle = {
  locale: "en-US",
  preference: "system",
  direction: "ltr",
  availableLocales: [{ id: "en-US", nativeName: "English" }],
  messages: {},
  numbers: [],
};
const t: Translate = (key, values) =>
  values ? `${key}:${values.version ?? ""}` : key;

function downloadable(
  version: string,
  overrides: Partial<DownloadableVersion> = {},
): DownloadableVersion {
  return {
    version,
    isLts: false,
    isBeta: false,
    isMts: false,
    isLatest: false,
    ...overrides,
  };
}

function installed(value: string): StudioVersion {
  return {
    version: value,
    displayName: `Studio Pro ${value}`,
    executablePath: `C:\\Program Files\\Mendix\\${value}\\modeler\\studiopro.exe`,
    installRoot: `C:\\Program Files\\Mendix\\${value}`,
    source: "fixture",
    removable: true,
  };
}

describe("VersionCatalogTable update guidance", () => {
  it("marks a stable update and exposes only safe HTTPS release notes", () => {
    render(
      <VersionCatalogTable
        t={t}
        localization={localization}
        online
        catalog={{
          versions: [
            downloadable("11.14.0", {
              isLatest: true,
              isBeta: true,
              releaseNotesUrl: "http://insecure.example.com/release",
            }),
            downloadable("11.13.0", {
              isLatest: true,
              releaseNotesUrl: "https://docs.example.com/release/11.13.0",
            }),
          ],
          totalCount: 2,
          fetchedAt: "2026-09-01T00:00:00Z",
          cacheFresh: true,
          loadedCount: 2,
          search: "",
          supportFilters: { lts: false, mts: false },
          loading: false,
          error: null,
          hasMore: false,
          installedSet: new Set(["11.12.2"]),
          installedVersions: [installed("11.12.2")],
          updateCandidates: new Set(["11.13.0"]),
          installedVersionsLoaded: true,
          studioSessionsLoading: false,
          isInstalling: false,
          queuedVersions: new Set<string>(),
          isBusy: () => false,
          onSearch: () => undefined,
          onToggleSupportFilter: () => undefined,
          onRefresh: () => undefined,
          onLoadMore: () => undefined,
          onInstall: () => undefined,
        }}
      />,
    );

    expect(screen.getByText("badge-update-available")).toBeInTheDocument();
    const link = screen.getByRole("link", {
      name: "release-notes-for:11.13.0",
    });
    expect(link).toHaveAttribute(
      "href",
      "https://docs.example.com/release/11.13.0",
    );
    expect(link).toHaveAttribute("target", "_blank");
    expect(
      screen.queryByRole("link", { name: "release-notes-for:11.14.0" }),
    ).not.toBeInTheDocument();
  });
});
