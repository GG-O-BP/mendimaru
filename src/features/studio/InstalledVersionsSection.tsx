import {
  AppWindow,
  CircleStop,
  Cable,
  LoaderCircle,
  MonitorUp,
  Play,
  RefreshCw,
  Trash2,
} from "lucide-react";
import type {
  LocalizationBundle,
  StudioSessionStatus,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  EmptyState,
  SectionHeader,
} from "../../shared/components/LayoutPrimitives";
import type { InstalledVersionsModel } from "./types";
import { useLocalizedDates } from "../../shared/hooks/useLocalizedValues";

export function InstalledVersionsSection({
  t,
  localization,
  online,
  model,
  countLabel,
}: {
  t: Translate;
  localization: LocalizationBundle;
  online: boolean;
  model: InstalledVersionsModel;
  countLabel: string;
}) {
  const formattedStarts = useLocalizedDates(
    model.sessions.map((session) => session.startedAt),
    localization,
  );
  const starts = new Map(
    model.sessions.map((session, index) => [
      session.sessionId,
      formattedStarts[index],
    ]),
  );

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
            disabled={model.sessionsLoading}
          >
            <RefreshCw
              size={16}
              className={model.sessionsLoading ? "spin" : undefined}
            />
          </button>
        }
      />
      <div className="installed-grid">
        {model.versions.map((version) => {
          const launchKey = `launch-${version.version}`;
          const uninstallKey = `uninstall-${version.version}`;
          const versionSessions = model.sessions.filter(
            (session) => session.version === version.version,
          );
          const mutationBusy =
            model.isBusy(`install-${version.version}`) ||
            model.isBusy(uninstallKey);
          return (
            <article className="installed-vessel" key={version.version}>
              <span className="vessel-icon">
                <AppWindow size={20} />
              </span>
              <div className="vessel-copy">
                <span className="micro-label">
                  {t("action-installed")}
                  {versionSessions.length > 0 && (
                    <em className="running-badge">
                      {t("studio-session-count", {
                        count: versionSessions.length,
                      })}
                    </em>
                  )}
                </span>
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
                  title={
                    versionSessions.length > 0
                      ? t("running-version-title")
                      : version.removable
                        ? t("remove-version-title", {
                            version: version.version,
                          })
                        : t("removal-unavailable-title")
                  }
                  onClick={() => model.onUninstall(version)}
                  disabled={
                    !online ||
                    !version.removable ||
                    versionSessions.length > 0 ||
                    model.isBusy(uninstallKey)
                  }
                >
                  {model.isBusy(uninstallKey) ? (
                    <LoaderCircle size={16} className="spin" />
                  ) : (
                    <Trash2 size={16} />
                  )}
                </button>
              </div>
              {versionSessions.length > 0 && (
                <div className="studio-session-list">
                  {versionSessions.map((session) => {
                    const reconnectKey = `reconnect-${session.sessionId}`;
                    const stopKey = `stop-${session.sessionId}`;
                    const reconnectBusy = model.isBusy(reconnectKey);
                    const stopBusy = model.isBusy(stopKey);
                    return (
                      <div className="studio-session" key={session.sessionId}>
                        <span
                          className="studio-session-status"
                          aria-hidden="true"
                        />
                        <div className="studio-session-copy">
                          <strong>
                            {session.projectName ??
                              t("studio-session-no-project")}
                          </strong>
                          <span>
                            {session.startedAt
                              ? t("studio-session-started", {
                                  date:
                                    starts.get(session.sessionId) ??
                                    session.startedAt,
                                })
                              : t("studio-session-start-unknown")}
                            {session.processId
                              ? ` · ${t("studio-session-process", {
                                  process: session.processId,
                                })}`
                              : ""}
                          </span>
                          <span className="studio-session-connection">
                            <Cable size={12} />
                            {connectionLabel(t, session)}
                          </span>
                        </div>
                        <div
                          className="studio-session-actions"
                          aria-label={t("studio-session-actions-aria", {
                            version: session.version,
                          })}
                        >
                          <button
                            type="button"
                            className="button light compact"
                            onClick={() => model.onReconnect(session)}
                            disabled={
                              !online ||
                              !session.reconnectable ||
                              reconnectBusy ||
                              stopBusy ||
                              mutationBusy
                            }
                            title={reconnectTitle(t, session)}
                          >
                            {reconnectBusy ? (
                              <LoaderCircle size={15} className="spin" />
                            ) : (
                              <MonitorUp size={15} />
                            )}
                            {session.connection === "native"
                              ? t("action-show-session")
                              : t("action-reconnect-session")}
                          </button>
                          <button
                            type="button"
                            className="icon-button danger inverse"
                            onClick={() => model.onStop(session)}
                            disabled={
                              !online ||
                              stopBusy ||
                              reconnectBusy ||
                              mutationBusy
                            }
                            title={t("action-stop-session")}
                          >
                            {stopBusy ? (
                              <LoaderCircle size={15} className="spin" />
                            ) : (
                              <CircleStop size={15} />
                            )}
                          </button>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
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

function connectionLabel(t: Translate, session: StudioSessionStatus) {
  switch (session.connection) {
    case "connected":
      return t("studio-session-connected");
    case "native":
      return t("studio-session-native");
    default:
      return t("studio-session-disconnected");
  }
}

function reconnectTitle(t: Translate, session: StudioSessionStatus) {
  if (session.reconnectable) {
    return session.connection === "native"
      ? t("action-show-session")
      : t("action-reconnect-session");
  }
  switch (session.reconnectUnavailable) {
    case "already-connected":
      return t("session-reconnect-already-connected");
    case "window-unavailable":
      return t("session-reconnect-window-unavailable");
    default:
      return t("session-reconnect-unsupported");
  }
}
