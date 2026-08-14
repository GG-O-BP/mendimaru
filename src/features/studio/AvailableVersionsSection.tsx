import { Download, LoaderCircle, XCircle } from "lucide-react";
import type { LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  EmptyState,
  SectionHeader,
} from "../../shared/components/LayoutPrimitives";
import { useLocalizedNumbers } from "../../shared/hooks/useLocalizedValues";
import { CatalogTools } from "./CatalogTools";
import { InstallationProgress } from "./InstallationProgress";
import type { CatalogModel, InstallationModel } from "./types";
import { useCatalogLoadMore } from "./useCatalogLoadMore";
import { VersionCatalogTable } from "./VersionCatalogTable";

export function AvailableVersionsSection({
  t,
  localization,
  online,
  catalog,
  installation,
}: {
  t: Translate;
  localization: LocalizationBundle;
  online: boolean;
  catalog: CatalogModel;
  installation: InstallationModel;
}) {
  const loadMoreSentinel = useCatalogLoadMore(catalog);
  const [loadedCountLabel, availableTotalLabel] = useLocalizedNumbers(
    [catalog.loadedCount, catalog.totalCount ?? 0],
    localization,
  );
  const filtering = Boolean(
    catalog.search || catalog.supportFilters.lts || catalog.supportFilters.mts,
  );

  return (
    <section
      className="section-card available-section manifest-panel"
      aria-labelledby="available-heading"
    >
      <SectionHeader
        id="available-heading"
        title={t("available-title")}
        meta={catalogSummary(t, catalog, loadedCountLabel, availableTotalLabel)}
        action={<CatalogTools t={t} catalog={catalog} />}
      />

      {installation.progress && (
        <InstallationProgress
          t={t}
          localization={localization}
          key={installation.progress.version}
          progress={installation.progress}
          isInstalling={installation.isInstalling}
          onCancel={installation.onCancel}
        />
      )}

      {catalog.error && (
        <div className="inline-error">
          <XCircle size={16} />
          <span>{catalog.error}</span>
          <button type="button" onClick={catalog.onRefresh}>
            {t("action-retry")}
          </button>
        </div>
      )}

      {catalog.versions.length > 0 && (
        <VersionCatalogTable
          t={t}
          localization={localization}
          online={online}
          catalog={catalog}
        />
      )}

      <CatalogEmptyState t={t} catalog={catalog} filtering={filtering} />

      {catalog.hasMore && (
        <div
          ref={loadMoreSentinel}
          className={`infinite-scroll-sentinel ${
            catalog.loading ? "loading" : ""
          }`}
          aria-live="polite"
        >
          {catalog.loading && (
            <>
              <LoaderCircle size={15} className="spin" />
              {t("catalog-loading-older")}
            </>
          )}
        </div>
      )}
    </section>
  );
}

function CatalogEmptyState({
  t,
  catalog,
  filtering,
}: {
  t: Translate;
  catalog: CatalogModel;
  filtering: boolean;
}) {
  if (catalog.loading && catalog.versions.length === 0) {
    return (
      <div className="manifest-state">
        <div className="loading-inline">
          <LoaderCircle size={18} className="spin" /> {t("catalog-loading")}
        </div>
      </div>
    );
  }
  if (
    catalog.loading ||
    catalog.versions.length > 0 ||
    catalog.error ||
    catalog.hasMore
  ) {
    return <div className="manifest-state" />;
  }
  return (
    <div className="manifest-state">
      <EmptyState
        icon={Download}
        title={filtering ? t("search-no-results") : t("catalog-empty")}
        detail={
          filtering ? t("filter-no-results-detail") : t("catalog-empty-detail")
        }
      />
    </div>
  );
}

function catalogSummary(
  t: Translate,
  catalog: CatalogModel,
  loadedCount: string,
  totalCount: string,
) {
  if (catalog.totalCount) {
    return t("catalog-loaded-total", {
      loaded: loadedCount,
      total: totalCount,
    });
  }
  return catalog.loadedCount
    ? t("catalog-loaded", { loaded: loadedCount })
    : t("official-marketplace");
}
