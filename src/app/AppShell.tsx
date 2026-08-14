import type { ReactNode } from "react";
import {
  AppWindow,
  ChevronRight,
  FolderKanban,
  Info,
  Languages,
  LoaderCircle,
  ListChecks,
  Monitor,
  Play,
  Server,
  Settings,
  X,
  type LucideIcon,
} from "lucide-react";
import type { LocalizationBundle, ViewKey } from "../domain/types";
import type { MessageKey, Translate } from "../i18n";
import { HarborMark } from "../shared/components/LayoutPrimitives";
import type { EnvironmentControlKind } from "../features/studio/types";

const TABS: Array<{ key: ViewKey; labelKey: MessageKey; icon: LucideIcon }> = [
  { key: "studio", labelKey: "nav-studio", icon: AppWindow },
  { key: "projects", labelKey: "nav-projects", icon: FolderKanban },
  { key: "operations", labelKey: "nav-operations", icon: ListChecks },
  { key: "settings", labelKey: "nav-settings", icon: Settings },
];

export function AppShell({
  t,
  localization,
  activeView,
  online,
  warning,
  languageChanging,
  winBoatControl,
  children,
  onViewChange,
  onLanguageChange,
  onDismissWarning,
}: {
  t: Translate;
  localization: LocalizationBundle;
  activeView: ViewKey;
  online: boolean;
  warning: string | null;
  languageChanging: boolean;
  winBoatControl: {
    kind: EnvironmentControlKind;
    label: string;
    busy: boolean;
    onAction: () => void;
  };
  children: ReactNode;
  onViewChange: (view: ViewKey) => void;
  onLanguageChange: (language: string) => void;
  onDismissWarning: () => void;
}) {
  const nativeWindows = winBoatControl.kind === "native";
  return (
    <div className="app-shell">
      <aside className="harbor-sidebar">
        <div className="brand-lockup">
          <HarborMark />
          <div>
            <strong>mendimaru</strong>
            <span>{t("brand-tagline")}</span>
          </div>
        </div>

        <nav className="harbor-nav" aria-label={t("nav-main-aria")}>
          {TABS.map(({ key, labelKey, icon: Icon }, index) => (
            <button
              type="button"
              key={key}
              className={activeView === key ? "active" : ""}
              onClick={() => onViewChange(key)}
              aria-current={activeView === key ? "page" : undefined}
              aria-keyshortcuts={`Control+${index + 1}`}
              title={`${t(labelKey)} · Ctrl+${index + 1}`}
            >
              <span className="nav-icon">
                <Icon size={19} />
              </span>
              <span className="nav-copy">
                {t(labelKey)}
                <small>0{index + 1}</small>
              </span>
              <ChevronRight
                className="nav-arrow"
                size={16}
                aria-hidden="true"
              />
            </button>
          ))}
        </nav>

        <div className="sidebar-waterline" aria-hidden="true">
          <i />
          <i />
          <i />
        </div>
      </aside>

      <div className="app-workspace">
        <header className="app-header">
          <div
            className={`route-status ${online ? "online" : "offline"}`}
            aria-label={t("route-aria")}
          >
            {!nativeWindows && (
              <>
                <span className="route-node host-node">
                  <Server size={16} aria-hidden="true" />
                  <span>{t("route-linux")}</span>
                </span>
                <span className="route-track" aria-hidden="true">
                  <i />
                </span>
              </>
            )}
            <span className="route-node windows-node">
              <Monitor size={16} aria-hidden="true" />
              <span>
                {nativeWindows ? t("route-native-windows") : t("route-windows")}
              </span>
            </span>
            <strong>
              <i />
              {nativeWindows
                ? online
                  ? t("connection-native")
                  : t("connection-native-not-ready")
                : online
                  ? t("connection-online")
                  : t("connection-offline")}
            </strong>
          </div>

          <div className="winboat-control">
            <label className="language-control" title={t("language-label")}>
              <Languages size={16} aria-hidden="true" />
              <span className="sr-only">{t("language-label")}</span>
              <select
                value={localization.preference}
                onChange={(event) => onLanguageChange(event.target.value)}
                aria-label={t("language-label")}
                aria-busy={languageChanging}
                disabled={languageChanging}
              >
                <option value="system">{t("language-system")}</option>
                {localization.availableLocales.map((locale) => (
                  <option key={locale.id} value={locale.id}>
                    {locale.nativeName}
                  </option>
                ))}
              </select>
            </label>
            {!nativeWindows && (
              <button
                type="button"
                className={`button ${online ? "secondary" : "primary"}`}
                onClick={winBoatControl.onAction}
                disabled={
                  winBoatControl.kind !== "settings" && winBoatControl.busy
                }
              >
                {winBoatControl.busy ? (
                  <LoaderCircle size={17} className="spin" />
                ) : winBoatControl.kind === "settings" ||
                  winBoatControl.kind === "setup" ? (
                  <Settings size={17} />
                ) : winBoatControl.kind === "open" ? (
                  <Monitor size={17} />
                ) : (
                  <Play size={17} />
                )}
                {winBoatControl.label}
              </button>
            )}
          </div>
        </header>

        {warning && (
          <div className="global-warning" role="alert">
            <Info size={18} />
            <span>{warning}</span>
            <button
              type="button"
              onClick={onDismissWarning}
              aria-label={t("dismiss-notification")}
            >
              <X size={17} />
            </button>
          </div>
        )}

        <main className="page" id="main-content">
          {children}
        </main>
      </div>
    </div>
  );
}
