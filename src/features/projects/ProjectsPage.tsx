import {
  FolderKanban,
  FolderInput,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  Play,
  RefreshCw,
  Search,
} from "lucide-react";
import type { LocalizationBundle, MendixProject } from "../../domain/types";
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
  search: string;
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
          <div className="manifest-table-wrap">
            <table className="manifest-table projects-table">
              <caption className="sr-only">{t("projects-found")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t("project-column-name")}</th>
                  <th scope="col">{t("project-column-version")}</th>
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
                    project.version && model.installedSet.has(project.version),
                  );
                  const pending = model.launchPendingFor(project);
                  const preferred = model.preferredVersionFor(project);
                  const launchKey = model.launchKeyFor(project);
                  return (
                    <tr key={project.mprPath}>
                      <td className="project-name-cell">
                        <span className="project-flag" aria-hidden="true">
                          <FolderKanban size={18} />
                        </span>
                        <div>
                          <strong>{project.name}</strong>
                          <span title={project.directory}>
                            {compactPath(project.directory)}
                          </span>
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
                          onClick={() => model.onOpenFolder(project.directory)}
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

function compactPath(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : path;
}
