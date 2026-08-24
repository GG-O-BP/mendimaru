import type {
  DownloadableVersion,
  StudioVersionCatalog,
} from "../../domain/types";
import type { VersionSupportFilters } from "./types";

export const CATALOG_CACHE_MAX_AGE_MS = 6 * 60 * 60 * 1_000;

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

export function catalogCacheIsFresh(
  catalog: StudioVersionCatalog,
  now = Date.now(),
) {
  if (catalog.versions.length === 0 || !catalog.fetchedAt) return false;
  const fetchedAt = Date.parse(catalog.fetchedAt);
  if (!Number.isFinite(fetchedAt)) return false;
  const age = now - fetchedAt;
  return age >= 0 && age <= CATALOG_CACHE_MAX_AGE_MS;
}
