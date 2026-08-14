import { AppWindow, LoaderCircle, Play, RefreshCw, Trash2 } from "lucide-react";
import type { Translate } from "../../i18n";
import {
  EmptyState,
  SectionHeader,
} from "../../shared/components/LayoutPrimitives";
import type { InstalledVersionsModel } from "./types";

export function InstalledVersionsSection({
  t,
  online,
  model,
  countLabel,
}: {
  t: Translate;
  online: boolean;
  model: InstalledVersionsModel;
  countLabel: string;
}) {
  return (
    <section
      className="section-card installed-section dock-panel"
      aria-labelledby="installed-heading"
    >
      <SectionHeader
        id="installed-heading"
        title={t("installed-title")}
        count={countLabel}
        meta={t("installed-meta")}
        action={
          <button
            type="button"
            className="icon-button"
            title={t("refresh-installed")}
            onClick={model.onRefresh}
          >
            <RefreshCw size={16} />
          </button>
        }
      />
      <div className="installed-grid">
        {model.versions.map((version) => {
          const launchKey = `launch-${version.version}`;
          const uninstallKey = `uninstall-${version.version}`;
          return (
            <article className="installed-vessel" key={version.version}>
              <span className="vessel-icon">
                <AppWindow size={20} />
              </span>
              <div className="vessel-copy">
                <span className="micro-label">{t("action-installed")}</span>
                <strong>{version.version}</strong>
                <span>{version.displayName}</span>
              </div>
              <div className="vessel-actions">
                <button
                  type="button"
                  className="button light"
                  onClick={() => model.onLaunch(version)}
                  disabled={!online || model.isLaunching}
                >
                  {model.isBusy(launchKey) ? (
                    <LoaderCircle size={17} className="spin" />
                  ) : (
                    <Play size={17} />
                  )}
                  {model.isBusy(launchKey)
                    ? t("action-launching")
                    : t("action-launch")}
                </button>
                <button
                  type="button"
                  className="icon-button danger inverse"
                  title={t("remove-version-title", {
                    version: version.version,
                  })}
                  onClick={() => model.onUninstall(version)}
                  disabled={!online || model.isBusy(uninstallKey)}
                >
                  {model.isBusy(uninstallKey) ? (
                    <LoaderCircle size={16} className="spin" />
                  ) : (
                    <Trash2 size={16} />
                  )}
                </button>
              </div>
            </article>
          );
        })}
        {model.versions.length === 0 && (
          <EmptyState
            icon={AppWindow}
            title={t("empty-installed-title")}
            detail={
              online
                ? t("empty-installed-online")
                : t("empty-installed-offline")
            }
          />
        )}
      </div>
    </section>
  );
}
