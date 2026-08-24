import { RefreshCw, Search } from "lucide-react";
import type { Translate } from "../../i18n";
import type { CatalogModel } from "./types";

export function CatalogTools({
  t,
  catalog,
}: {
  t: Translate;
  catalog: CatalogModel;
}) {
  return (
    <div className="catalog-tools">
      <label className="search-field">
        <Search size={16} />
        <span className="sr-only">{t("search-version-placeholder")}</span>
        <input
          value={catalog.search}
          onChange={(event) => catalog.onSearch(event.target.value)}
          placeholder={t("search-version-placeholder")}
          spellCheck={false}
        />
      </label>
      <div
        className="version-filters"
        role="group"
        aria-label={t("support-filter-aria")}
      >
        {(["lts", "mts"] as const).map((filter) => (
          <button
            type="button"
            className={catalog.supportFilters[filter] ? "active" : ""}
            key={filter}
            aria-pressed={catalog.supportFilters[filter]}
            onClick={() => catalog.onToggleSupportFilter(filter)}
          >
            {filter.toUpperCase()}
          </button>
        ))}
      </div>
      <button
        type="button"
        data-testid="refresh-catalog"
        className="icon-button"
        title={t("refresh-catalog")}
        onClick={catalog.onRefresh}
        disabled={catalog.loading}
      >
        <RefreshCw size={16} className={catalog.loading ? "spin" : ""} />
      </button>
    </div>
  );
}
