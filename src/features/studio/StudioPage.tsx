import { Anchor, LoaderCircle, Monitor, Play, Settings } from "lucide-react";
import type { LocalizationBundle } from "../../domain/types";
import type { Translate } from "../../i18n";
import { PageTitle } from "../../shared/components/LayoutPrimitives";
import { useLocalizedNumbers } from "../../shared/hooks/useLocalizedValues";
import { AvailableVersionsSection } from "./AvailableVersionsSection";
import { InstalledVersionsSection } from "./InstalledVersionsSection";
import type {
  CatalogModel,
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
}) {
  const [installedCountLabel] = useLocalizedNumbers(
    [installed.versions.length],
    localization,
  );

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
        online={online}
        model={installed}
        countLabel={installedCountLabel}
      />
      <AvailableVersionsSection
        t={t}
        localization={localization}
        online={online}
        catalog={catalog}
        installation={installation}
      />
    </div>
  );
}
