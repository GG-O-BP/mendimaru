import type {
  DownloadableVersion,
  StudioVersionCatalog,
} from "../../domain/types";
import type { VersionSupportFilters } from "./types";

export function filterCatalog(
  versions: DownloadableVersion[],
  search: string,
  supportFilters: VersionSupportFilters,
) {
  const needle = search.trim().toLowerCase();
  const filteringBySupport = supportFilters.lts || supportFilters.mts;

  return versions.filter((version) => {
    const matchesSearch =
      !needle || version.version.toLowerCase().includes(needle);
    const matchesSupport =
      !filteringBySupport ||
      (supportFilters.lts && version.isLts) ||
      (supportFilters.mts && version.isMts);
    return matchesSearch && matchesSupport;
  });
}

export function nextCatalogPage(catalog: StudioVersionCatalog) {
  return Math.max(0, ...catalog.loadedPages) + 1;
}

export function catalogHasMore(catalog: StudioVersionCatalog) {
  if (catalog.totalCount != null) {
    return catalog.versions.length < catalog.totalCount;
  }
  return (
    catalog.loadedPages.length > 0 &&
    catalog.versions.length >= catalog.loadedPages.length * 10
  );
}
