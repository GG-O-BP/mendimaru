import { Anchor, LoaderCircle, Monitor, Play, Settings } from "lucide-react";
import type { LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import { PageTitle } from "../../shared/components/LayoutPrimitives";
import { useLocalizedNumbers } from "../../shared/hooks/useLocalizedValues";
import { AvailableVersionsSection } from "./AvailableVersionsSection";
import { InstalledVersionsSection } from "./InstalledVersionsSection";
import type {
  CatalogModel,
  InstallQueueModel,
  InstallationModel,
  InstalledVersionsModel,
  EnvironmentControlKind,
} from "./types";

export function StudioPage({
  t,
  localization,
  online,
  offlineGuidance,
  winBoatControl,
  installed,
  catalog,
  installation,
  queue,
}: {
  t: Translate;
  localization: LocalizationBundle;
  online: boolean;
  offlineGuidance: { title: string; detail: string };
  winBoatControl: {
    kind: EnvironmentControlKind;
    label: string;
    busy: boolean;
    onAction: () => void;
  };
  installed: InstalledVersionsModel;
  catalog: CatalogModel;
  installation: InstallationModel;
  queue: InstallQueueModel;
}) {
  const [installedCountLabel] = useLocalizedNumbers(
    [installed.versions.length],
    localization,
  );
  const visibleInstalledCount =
    installed.loaded || installed.versions.length > 0
      ? installedCountLabel
      : "\u2014";

  return (
    <div className="studio-page" data-testid="studio-page">
      <PageTitle
        eyebrow={t("studio-eyebrow")}
        title={t("nav-studio")}
        description={t("studio-description")}
      />

      {!online && (
        <aside
          className="route-notice"
          aria-labelledby="offline-guidance-title"
        >
          <span className="route-notice-icon">
            <Anchor size={22} />
          </span>
          <div>
            <strong id="offline-guidance-title">{offlineGuidance.title}</strong>
            <p>{offlineGuidance.detail}</p>
          </div>
          <button
            type="button"
            className="button primary"
            onClick={winBoatControl.onAction}
            disabled={
              winBoatControl.kind !== "settings" &&
              winBoatControl.kind !== "native" &&
              winBoatControl.busy
            }
          >
            {winBoatControl.busy ? (
              <LoaderCircle size={17} className="spin" />
            ) : winBoatControl.kind === "settings" ||
              winBoatControl.kind === "native" ||
              winBoatControl.kind === "setup" ? (
              <Settings size={17} />
            ) : winBoatControl.kind === "open" ? (
              <Monitor size={17} />
            ) : (
              <Play size={17} />
            )}
            {winBoatControl.label}
          </button>
        </aside>
      )}

      <InstalledVersionsSection
        t={t}
        localization={localization}
        online={online}
        model={installed}
        countLabel={visibleInstalledCount}
      />
      <AvailableVersionsSection
        t={t}
        localization={localization}
        online={online}
        catalog={catalog}
        installation={installation}
        queue={queue}
      />
    </div>
  );
}
