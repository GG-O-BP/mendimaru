import { CheckCircle2, Download, LoaderCircle, RefreshCw } from "lucide-react";
import type { LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import { useLocalizedDates } from "../../shared/hooks/useLocalizedValues";
import type { CatalogModel } from "./types";

export function VersionCatalogTable({
  t,
  localization,
  online,
  catalog,
}: {
  t: Translate;
  localization: LocalizationBundle;
  online: boolean;
  catalog: CatalogModel;
}) {
  const releaseDates = useLocalizedDates(
    catalog.versions.map((version) => version.releaseDate),
    localization,
  );

  return (
    <div className="manifest-table-wrap">
      <table className="manifest-table">
        <caption className="sr-only">{t("available-title")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("manifest-version")}</th>
            <th scope="col">{t("manifest-support")}</th>
            <th scope="col" className="release-cell">
              {t("manifest-release")}
            </th>
            <th scope="col">{t("manifest-status")}</th>
            <th scope="col">
              <span className="sr-only">{t("manifest-actions")}</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {catalog.versions.map((version, index) => (
            <VersionRow
              t={t}
              key={version.version}
              version={version}
              releaseDate={releaseDates[index] || version.releaseDate || "—"}
              online={online}
              alreadyInstalled={catalog.installedSet.has(version.version)}
              installedVersionsLoaded={catalog.installedVersionsLoaded}
              installing={catalog.isBusy(`install-${version.version}`)}
              installationBusy={catalog.isInstalling}
              onInstall={catalog.onInstall}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function VersionRow({
  t,
  version,
  releaseDate,
  online,
  alreadyInstalled,
  installedVersionsLoaded,
  installing,
  installationBusy,
  onInstall,
}: {
  t: Translate;
  version: CatalogModel["versions"][number];
  releaseDate: string;
  online: boolean;
  alreadyInstalled: boolean;
  installedVersionsLoaded: boolean;
  installing: boolean;
  installationBusy: boolean;
  onInstall: CatalogModel["onInstall"];
}) {
  const availability = !installedVersionsLoaded
    ? "checking"
    : alreadyInstalled
      ? "installed"
      : online
        ? "available"
        : "offline";

  return (
    <tr>
      <td className="version-cell">
        <span className="version-beacon" aria-hidden="true" />
        <div>
          <strong>{version.version}</strong>
          <span>Studio Pro</span>
        </div>
      </td>
      <td className="support-cell">
        <VersionBadges t={t} version={version} />
      </td>
      <td className="release-cell">{releaseDate}</td>
      <td>
        <span className={`availability-state ${availability}`}>
          <i />
          {!installedVersionsLoaded
            ? t("status-checking-installed")
            : alreadyInstalled
              ? t("action-installed")
              : online
                ? t("status-available")
                : t("connection-offline")}
        </span>
      </td>
      <td className="manifest-action">
        <button
          type="button"
          className={`button compact ${alreadyInstalled ? "quiet" : "primary"}`}
          disabled={
            !online ||
            !installedVersionsLoaded ||
            alreadyInstalled ||
            installationBusy
          }
          onClick={() => onInstall(version)}
        >
          {installing ? (
            <LoaderCircle size={16} className="spin" />
          ) : alreadyInstalled ? (
            <CheckCircle2 size={16} />
          ) : (
            <Download size={16} />
          )}
          {installing
            ? t("action-installing")
            : alreadyInstalled
              ? t("action-installed")
              : t("action-install")}
        </button>
        {installedVersionsLoaded && !alreadyInstalled && (
          <button
            type="button"
            className="icon-button compact"
            title={t("action-force-redownload")}
            aria-label={t("action-force-redownload")}
            disabled={!online || installationBusy}
            onClick={() => onInstall(version, true)}
          >
            <RefreshCw size={15} />
          </button>
        )}
      </td>
    </tr>
  );
}

function VersionBadges({
  t,
  version,
}: {
  t: Translate;
  version: CatalogModel["versions"][number];
}) {
  return (
    <span className="badges">
      {version.isLatest && <em className="latest">{t("badge-latest")}</em>}
      {version.isLts && <em className="lts">LTS</em>}
      {version.isMts && <em className="mts">MTS</em>}
      {version.isBeta && <em className="beta">{t("badge-beta")}</em>}
    </span>
  );
}
