import { describe, expect, it } from "vitest";
import type {
  DownloadableVersion,
  StudioVersion,
  StudioVersionCatalog,
} from "../../domain/types";
import {
  CATALOG_CACHE_MAX_AGE_MS,
  catalogCacheIsFresh,
  catalogHasMore,
  compareStudioVersions,
  filterCatalog,
  nextCatalogPage,
  safeReleaseNotesUrl,
  selectUpdateCandidateVersions,
} from "./selectors";

function version(
  value: string,
  support: Partial<
    Pick<DownloadableVersion, "isBeta" | "isLatest" | "isLts" | "isMts">
  > = {},
): DownloadableVersion {
  return {
    version: value,
    isLts: support.isLts ?? false,
    isMts: support.isMts ?? false,
    isBeta: support.isBeta ?? false,
    isLatest: support.isLatest ?? false,
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

describe("Studio catalog selectors", () => {
  const versions = [
    version("11.13.0"),
    version("11.12.2", { isLts: true }),
    version("10.24.22", { isMts: true }),
  ];

  it("combines text and support filters", () => {
    expect(
      filterCatalog(versions, "11", { lts: true, mts: false }).map(
        (item) => item.version,
      ),
    ).toEqual(["11.12.2"]);
    expect(
      filterCatalog(versions, "", { lts: true, mts: true }).map(
        (item) => item.version,
      ),
    ).toEqual(["11.12.2", "10.24.22"]);
  });

  it("derives pagination from loaded pages and server totals", () => {
    const catalog: StudioVersionCatalog = {
      versions,
      loadedPages: [1, 3, 2],
      totalCount: 4,
    };
    expect(nextCatalogPage(catalog)).toBe(4);
    expect(catalogHasMore(catalog)).toBe(true);
    expect(catalogHasMore({ ...catalog, totalCount: 3 })).toBe(false);
  });

  it("uses the page-size heuristic when no total is available", () => {
    const catalog: StudioVersionCatalog = {
      versions: Array.from({ length: 10 }, (_, index) =>
        version(`11.0.${index}`),
      ),
      loadedPages: [1],
    };
    expect(catalogHasMore(catalog)).toBe(true);
    expect(
      catalogHasMore({ ...catalog, versions: catalog.versions.slice(0, 9) }),
    ).toBe(false);
  });

  it("reuses only a non-empty catalog fetched within six hours", () => {
    const now = Date.parse("2026-08-24T06:00:00Z");
    const catalog: StudioVersionCatalog = {
      versions,
      loadedPages: [1],
      fetchedAt: new Date(now - CATALOG_CACHE_MAX_AGE_MS).toISOString(),
    };

    expect(catalogCacheIsFresh(catalog, now)).toBe(true);
    expect(
      catalogCacheIsFresh(
        {
          ...catalog,
          fetchedAt: new Date(now - CATALOG_CACHE_MAX_AGE_MS - 1).toISOString(),
        },
        now,
      ),
    ).toBe(false);
    expect(catalogCacheIsFresh({ ...catalog, versions: [] }, now)).toBe(false);
    expect(
      catalogCacheIsFresh({ ...catalog, fetchedAt: "not-a-date" }, now),
    ).toBe(false);
    expect(
      catalogCacheIsFresh(
        { ...catalog, fetchedAt: new Date(now + 1).toISOString() },
        now,
      ),
    ).toBe(false);
  });

  it("compares dotted Studio versions numerically", () => {
    expect(compareStudioVersions("11.9.0", "11.10.0")).toBeLessThan(0);
    expect(compareStudioVersions("11.10.0", "11.10.0")).toBe(0);
    expect(compareStudioVersions("11.10.1", "11.10")).toBeGreaterThan(0);
  });

  it("selects stable channel update candidates without recommending beta", () => {
    const updateCatalog: StudioVersionCatalog = {
      versions: [
        version("11.14.0", { isLatest: true, isBeta: true }),
        version("11.13.0", { isLatest: true }),
        version("10.24.24", { isLts: true }),
        version("10.24.20", { isLts: true }),
      ],
      loadedPages: [1],
    };

    expect(
      selectUpdateCandidateVersions(updateCatalog, [
        installed("11.12.2"),
        installed("10.24.20"),
      ]),
    ).toEqual(new Set(["11.13.0", "10.24.24"]));
    expect(
      selectUpdateCandidateVersions(updateCatalog, [
        installed("11.13.0"),
        installed("10.24.24"),
      ]),
    ).toEqual(new Set());
  });

  it("accepts only credentialed-free HTTPS release note links", () => {
    expect(safeReleaseNotesUrl("https://docs.example.com/release")).toBe(
      "https://docs.example.com/release",
    );
    expect(
      safeReleaseNotesUrl("http://docs.example.com/release"),
    ).toBeUndefined();
    expect(safeReleaseNotesUrl("javascript:alert(1)")).toBeUndefined();
    expect(
      safeReleaseNotesUrl("https://user:password@docs.example.com/release"),
    ).toBeUndefined();
    expect(safeReleaseNotesUrl(undefined)).toBeUndefined();
  });
});
