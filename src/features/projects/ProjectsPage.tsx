import {
  ArrowDownUp,
  FolderKanban,
  FolderInput,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  Play,
  RefreshCw,
  Search,
  Star,
} from "lucide-react";
import type {
  LocalizationBundle,
  MendixProject,
  ProjectScanResult,
  ProjectSortKey,
} from "../../domain/types";
import type { Translate } from "../../i18n";
import {
  EmptyState,
  PageTitle,
  SectionHeader,
} from "../../shared/components/LayoutPrimitives";
import {
  useLocalizedDates,
  useLocalizedNumbers,
} from "../../shared/hooks/useLocalizedValues";

export interface ProjectsPageModel {
  projects: MendixProject[];
  totalProjects: number;
  totalVisibleProjects: number;
  hasMoreProjects: boolean;
  search: string;
  favoriteOnly: boolean;
  sortKey: ProjectSortKey;
  scanStatus: "loading" | "stale" | "ready" | "error";
  scanError?: string;
  scan?: ProjectScanResult;
  sharedDirectory: string;
  installedSet: Set<string>;
  installedVersionsLoaded: boolean;
  studioLaunchReady: boolean;
  studioSessionsLoading: boolean;
  connectedRemoteAppVersion?: string;
  supportsExternalSelection: boolean;
  externalSelectionBusy: boolean;
  isLaunching: boolean;
  isBusy: (key: string) => boolean;
  launchKeyFor: (project: MendixProject) => string;
  preferredVersionFor: (project: MendixProject) => string | undefined;
  launchPendingFor: (project: MendixProject) => boolean;
  onSearch: (value: string) => void;
  onFavoriteOnly: (value: boolean) => void;
  onSortKey: (value: ProjectSortKey) => void;
  onShowMoreProjects: () => void;
  onToggleFavorite: (project: MendixProject) => void;
  onRefresh: () => void;
  onOpenWorkspace: () => void;
  onOpenFolder: (path: string) => void;
  onSelectExternal: () => void;
  onLaunch: (project: MendixProject) => void;
}

export function ProjectsPage({
  t,
  localization,
  model,
}: {
  t: Translate;
  localization: LocalizationBundle;
  model: ProjectsPageModel;
}) {
  const [totalProjectsLabel] = useLocalizedNumbers(
    [model.totalProjects],
    localization,
  );
  const modifiedDates = useLocalizedDates(
    model.projects.map((project) => project.lastModified),
    localization,
  );
  const launchedDates = useLocalizedDates(
    model.projects.map((project) => project.lastLaunchedAt),
    localization,
  );

  return (
    <div className="projects-page" data-testid="projects-page">
      <PageTitle
        eyebrow={t("projects-eyebrow")}
        title={t("projects-title")}
        description={t("projects-description")}
      />

      <aside className="workspace-berth">
        <span className="berth-icon">
          <HardDrive size={21} />
        </span>
        <div>
          <span className="micro-label">{t("workspace-current")}</span>
          <strong title={model.sharedDirectory}>{model.sharedDirectory}</strong>
        </div>
        <button
          type="button"
          className="button secondary"
          onClick={model.onOpenWorkspace}
        >
          <FolderOpen size={17} />
          {t("action-open-folder")}
        </button>
      </aside>

      <section
        className="section-card project-manifest"
        aria-labelledby="projects-heading"
      >
        <SectionHeader
          id="projects-heading"
          title={t("projects-found")}
          count={totalProjectsLabel}
          meta={t("projects-manifest-meta")}
          action={<ProjectTools t={t} model={model} />}
        />

        <ProjectScanStatus t={t} model={model} />

        {model.connectedRemoteAppVersion && (
          <div className="project-session-notice" role="status">
            <strong>
              {t("studio-connected-session-blocks-title", {
                version: model.connectedRemoteAppVersion,
              })}
            </strong>
            <span>
              {t("studio-connected-session-blocks-detail", {
                version: model.connectedRemoteAppVersion,
              })}
            </span>
          </div>
        )}

        {model.supportsExternalSelection && (
          <div className="project-session-notice" role="note">
            <strong>{t("external-project-share-title")}</strong>
            <span>{t("external-project-share-detail")}</span>
          </div>
        )}

        {model.projects.length > 0 ? (
          <>
            <div className="manifest-table-wrap">
              <table className="manifest-table projects-table">
                <caption className="sr-only">{t("projects-found")}</caption>
                <thead>
                  <tr>
                    <th scope="col">{t("project-column-name")}</th>
                    <th scope="col">{t("project-column-version")}</th>
                    <th scope="col">{t("project-column-recent")}</th>
                    <th scope="col" className="modified-cell">
                      {t("project-column-modified")}
                    </th>
                    <th scope="col">
                      <span className="sr-only">{t("manifest-actions")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {model.projects.map((project, index) => {
                    const exactVersionInstalled = Boolean(
                      project.version &&
                      model.installedSet.has(project.version),
                    );
                    const pending = model.launchPendingFor(project);
                    const preferred = model.preferredVersionFor(project);
                    const launchKey = model.launchKeyFor(project);
                    return (
                      <tr key={project.mprPath}>
                        <td className="project-name-cell">
                          <div className="project-identity">
                            <button
                              type="button"
                              className={`icon-button favorite-toggle ${
                                project.favorite ? "active" : ""
                              }`}
                              aria-pressed={project.favorite}
                              title={
                                project.location === "explicit-host-selection"
                                  ? t("project-favorite-external-disabled")
                                  : project.favorite
                                    ? t("project-favorite-remove")
                                    : t("project-favorite-add")
                              }
                              disabled={
                                project.location === "explicit-host-selection"
                              }
                              onClick={() => model.onToggleFavorite(project)}
                            >
                              <Star
                                size={16}
                                fill={
                                  project.favorite ? "currentColor" : "none"
                                }
                              />
                            </button>
                            <span className="project-flag" aria-hidden="true">
                              <FolderKanban size={18} />
                            </span>
                            <div>
                              <strong>{project.name}</strong>
                              <span title={project.directory}>
                                {compactPath(project.directory)}
                              </span>
                            </div>
                          </div>
                        </td>
                        <td>
                          <span
                            className={`version-state ${
                              project.version && !exactVersionInstalled
                                ? "missing"
                                : "ready"
                            }`}
                          >
                            {project.version ?? t("version-unknown")}
                            {project.version && (
                              <small>
                                {exactVersionInstalled
                                  ? t("version-ready")
                                  : t("version-missing")}
                              </small>
                            )}
                            {preferred && preferred !== project.version && (
                              <small>
                                {t("project-launch-remembered", {
                                  version: preferred,
                                })}
                              </small>
                            )}
                          </span>
                        </td>
                        <td className="modified-cell">
                          {launchedDates[index] || "—"}
                        </td>
                        <td className="modified-cell">
                          {modifiedDates[index] || "—"}
                        </td>
                        <td className="project-actions">
                          <button
                            type="button"
                            className="button primary compact"
                            onClick={() => model.onLaunch(project)}
                            disabled={
                              !model.studioLaunchReady ||
                              Boolean(model.connectedRemoteAppVersion) ||
                              model.isLaunching
                            }
                            title={
                              model.connectedRemoteAppVersion
                                ? t("studio-connected-session-blocks-detail", {
                                    version: model.connectedRemoteAppVersion,
                                  })
                                : undefined
                            }
                          >
                            {model.isBusy(launchKey) ? (
                              <LoaderCircle size={16} className="spin" />
                            ) : (
                              <Play size={16} />
                            )}
                            {model.isBusy(launchKey)
                              ? t("action-opening")
                              : pending
                                ? t("project-launch-resume")
                                : exactVersionInstalled
                                  ? t("action-open")
                                  : t("project-launch-assist")}
                          </button>
                          <button
                            type="button"
                            className="icon-button"
                            title={t("open-linux-folder")}
                            onClick={() =>
                              model.onOpenFolder(project.directory)
                            }
                          >
                            <FolderOpen size={17} />
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            {model.hasMoreProjects && (
              <div className="project-pagination">
                <button
                  type="button"
                  className="button secondary compact"
                  onClick={model.onShowMoreProjects}
                >
                  {t("projects-show-more")}
                </button>
                <span>
                  {t("projects-visible-count", {
                    visible: model.projects.length,
                    total: model.totalVisibleProjects,
                  })}
                </span>
              </div>
            )}
          </>
        ) : (
          <div className="manifest-state">
            <EmptyState
              icon={FolderKanban}
              title={
                model.totalProjects
                  ? t("projects-search-empty")
                  : t("projects-empty")
              }
              detail={
                model.totalProjects
                  ? t("projects-search-detail")
                  : t("projects-empty-detail")
              }
            />
          </div>
        )}
      </section>
    </div>
  );
}

function ProjectTools({
  t,
  model,
}: {
  t: Translate;
  model: ProjectsPageModel;
}) {
  return (
    <div className="catalog-tools">
      <label className="search-field project-search">
        <Search size={16} />
        <span className="sr-only">{t("search-project-placeholder")}</span>
        <input
          value={model.search}
          onChange={(event) => model.onSearch(event.target.value)}
          placeholder={t("search-project-placeholder")}
        />
      </label>
      <label className="favorite-filter">
        <input
          type="checkbox"
          checked={model.favoriteOnly}
          onChange={(event) => model.onFavoriteOnly(event.target.checked)}
        />
        <span>{t("projects-favorite-filter")}</span>
      </label>
      <label className="project-sort">
        <ArrowDownUp size={15} />
        <span className="sr-only">{t("projects-sort-label")}</span>
        <select
          value={model.sortKey}
          onChange={(event) =>
            model.onSortKey(event.target.value as ProjectSortKey)
          }
        >
          <option value="modified">{t("projects-sort-modified")}</option>
          <option value="name">{t("projects-sort-name")}</option>
          <option value="version">{t("projects-sort-version")}</option>
          <option value="recent">{t("projects-sort-recent")}</option>
        </select>
      </label>
      <button
        type="button"
        data-testid="refresh-projects"
        className="icon-button"
        title={t("refresh-projects")}
        onClick={model.onRefresh}
      >
        <RefreshCw size={16} />
      </button>
      {model.supportsExternalSelection && (
        <button
          type="button"
          className="button secondary compact"
          onClick={model.onSelectExternal}
          disabled={
            !model.studioLaunchReady ||
            Boolean(model.connectedRemoteAppVersion) ||
            model.isLaunching ||
            model.externalSelectionBusy
          }
        >
          {model.externalSelectionBusy ? (
            <LoaderCircle size={16} className="spin" />
          ) : (
            <FolderInput size={16} />
          )}
          {t("action-open-external-project")}
        </button>
      )}
    </div>
  );
}

function ProjectScanStatus({
  t,
  model,
}: {
  t: Translate;
  model: ProjectsPageModel;
}) {
  if (model.scanStatus === "loading") {
    return (
      <div className="project-scan-status" role="status">
        {t("projects-scan-loading")}
      </div>
    );
  }
  if (model.scanStatus === "stale") {
    return (
      <div className="project-scan-status stale" role="status">
        {t("projects-scan-stale")}
      </div>
    );
  }
  if (model.scanStatus === "error") {
    return (
      <div className="project-scan-status error" role="alert">
        <strong>{t("projects-scan-error")}</strong>
        {model.scanError && <span>{model.scanError}</span>}
      </div>
    );
  }
  if (
    !model.scan ||
    (!model.scan.truncated &&
      model.scan.skippedEntries === 0 &&
      model.scan.errorCount === 0)
  ) {
    return null;
  }
  return (
    <div className="project-scan-status partial" role="status">
      <strong>{t("projects-scan-partial")}</strong>
      <span>
        {t("projects-scan-partial-detail", {
          projects: model.scan.projects.length,
          skipped: model.scan.skippedEntries,
          errors: model.scan.errorCount,
        })}
      </span>
      {model.scan.errors[0] && <small>{model.scan.errors[0]}</small>}
    </div>
  );
}

function compactPath(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : path;
}
