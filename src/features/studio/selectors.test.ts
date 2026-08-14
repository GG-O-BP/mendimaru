import { describe, expect, it } from "vitest";
import type {
  DownloadableVersion,
  StudioVersionCatalog,
} from "../../domain/types";
import { catalogHasMore, filterCatalog, nextCatalogPage } from "./selectors";

function version(
  value: string,
  support: Partial<Pick<DownloadableVersion, "isLts" | "isMts">> = {},
): DownloadableVersion {
  return {
    version: value,
    isLts: support.isLts ?? false,
    isMts: support.isMts ?? false,
    isBeta: false,
    isLatest: false,
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
});
