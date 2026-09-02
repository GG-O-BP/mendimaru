import type {
  DownloadableVersion,
  StudioVersionCatalog,
  StudioVersion,
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

export function compareStudioVersions(left: string, right: string) {
  const parse = (value: string) =>
    value
      .split(/[-+]/)[0]
      ?.split(".")
      .map((part) => Number.parseInt(part, 10))
      .map((part) => (Number.isFinite(part) ? part : 0)) ?? [];
  const leftParts = parse(left);
  const rightParts = parse(right);
  const length = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

export function selectUpdateCandidateVersions(
  catalog: StudioVersionCatalog,
  installedVersions: StudioVersion[],
) {
  const installed = new Set(
    installedVersions.map((version) => version.version),
  );
  const candidates = new Set<string>();
  const channels = [
    (version: DownloadableVersion) => version.isLatest,
    (version: DownloadableVersion) => version.isLts,
    (version: DownloadableVersion) => version.isMts,
  ] as const;

  for (const channel of channels) {
    const channelVersions = catalog.versions.filter(
      (version) => !version.isBeta && channel(version),
    );
    const installedChannelVersions = channelVersions
      .filter((version) => installed.has(version.version))
      .sort((left, right) =>
        compareStudioVersions(left.version, right.version),
      );
    const installedChannelVersion =
      installedChannelVersions[installedChannelVersions.length - 1];
    const updateVersions = channelVersions
      .filter(
        (version) =>
          !installedChannelVersion ||
          compareStudioVersions(
            version.version,
            installedChannelVersion.version,
          ) > 0,
      )
      .sort((left, right) =>
        compareStudioVersions(left.version, right.version),
      );
    const candidate = updateVersions[updateVersions.length - 1];
    if (candidate && !installed.has(candidate.version))
      candidates.add(candidate.version);
  }
  return candidates;
}

export function safeReleaseNotesUrl(value?: string) {
  if (!value) return undefined;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password) {
      return undefined;
    }
    return url.toString();
  } catch {
    return undefined;
  }
}
