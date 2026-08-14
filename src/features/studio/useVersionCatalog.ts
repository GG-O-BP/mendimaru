import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { StudioVersionCatalog } from "../../domain/types";
import type { VersionCatalogDependencies } from "./dependencies";
import { catalogHasMore, filterCatalog, nextCatalogPage } from "./selectors";
import {
  EMPTY_VERSION_SUPPORT_FILTERS,
  type VersionSupportFilter,
  type VersionSupportFilters,
} from "./types";

const EMPTY_CATALOG: StudioVersionCatalog = {
  versions: [],
  loadedPages: [],
};

type CatalogRequestState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "error"; message: string };

export function useVersionCatalog({ t }: VersionCatalogDependencies) {
  const [catalog, setCatalog] = useState<StudioVersionCatalog>(EMPTY_CATALOG);
  const [requestState, setRequestState] = useState<CatalogRequestState>({
    status: "idle",
  });
  const [search, setSearch] = useState("");
  const [supportFilters, setSupportFilters] = useState<VersionSupportFilters>(
    EMPTY_VERSION_SUPPORT_FILTERS,
  );
  const requestInFlight = useRef(false);

  const fetchCatalogPage = useCallback(
    async (page: number, reset = false) => {
      if (requestInFlight.current) return;
      requestInFlight.current = true;
      setRequestState({ status: "loading" });
      try {
        setCatalog(await tauriApi.fetchDownloadableVersions(page, reset));
        setRequestState({ status: "idle" });
      } catch (error) {
        setRequestState({ status: "error", message: errorText(error, t) });
      } finally {
        requestInFlight.current = false;
      }
    },
    [t],
  );

  useEffect(() => {
    let active = true;
    void tauriApi
      .getDownloadableVersionsCache()
      .then((cached) => {
        if (active) setCatalog(cached);
      })
      .catch(() => undefined)
      .finally(() => {
        if (active) void fetchCatalogPage(1);
      });
    return () => {
      active = false;
    };
  }, [fetchCatalogPage]);

  const filteredCatalog = useMemo(
    () => filterCatalog(catalog.versions, search, supportFilters),
    [catalog.versions, search, supportFilters],
  );
  const hasMore = catalogHasMore(catalog);
  const nextPage = nextCatalogPage(catalog);

  const toggleSupportFilter = useCallback((filter: VersionSupportFilter) => {
    setSupportFilters((current) => ({
      ...current,
      [filter]: !current[filter],
    }));
  }, []);
  const resetFilters = useCallback(() => {
    setSearch("");
    setSupportFilters(EMPTY_VERSION_SUPPORT_FILTERS);
  }, []);
  const findVersion = useCallback((version: string) => {
    setSearch(version);
    setSupportFilters(EMPTY_VERSION_SUPPORT_FILTERS);
  }, []);
  const refreshCatalog = useCallback(
    () => fetchCatalogPage(1, true),
    [fetchCatalogPage],
  );
  const loadMore = useCallback(
    () => fetchCatalogPage(nextPage),
    [fetchCatalogPage, nextPage],
  );
  const resolveVersion = useCallback(async (version: string) => {
    const resolved = await tauriApi.resolveDownloadableVersion(version);
    setCatalog((current) => ({
      ...current,
      versions: [
        resolved,
        ...current.versions.filter(
          (candidate) => candidate.version !== resolved.version,
        ),
      ],
    }));
    return resolved;
  }, []);

  return {
    catalog,
    filteredCatalog,
    catalogLoading: requestState.status === "loading",
    catalogError: requestState.status === "error" ? requestState.message : null,
    search,
    supportFilters,
    hasMore,
    setSearch,
    toggleSupportFilter,
    resetFilters,
    findVersion,
    refreshCatalog,
    loadMore,
    resolveVersion,
  };
}
