import type { AppConfig, LocalizationBundle, ToastKind } from "../domain/types";
import { ProjectLaunchAssistant } from "../features/projects/ProjectLaunchAssistant";
import {
  ProjectsPage,
  type ProjectsPageModel,
} from "../features/projects/ProjectsPage";
import { useProjectLauncher } from "../features/projects/useProjectLauncher";
import type { useProjects } from "../features/projects/useProjects";
import type { useStudio } from "../features/studio/useStudio";
import type { Translate } from "../i18n";

export function ProjectsView({
  t,
  localization,
  config,
  requiresWinboat,
  projects,
  studio,
  isBusy,
  notify,
}: {
  t: Translate;
  localization: LocalizationBundle;
  config: AppConfig;
  requiresWinboat: boolean;
  projects: ReturnType<typeof useProjects>;
  studio: ReturnType<typeof useStudio>;
  isBusy: (key: string) => boolean;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
}) {
  const launcher = useProjectLauncher({
    t,
    installedVersions: studio.installedVersions,
    installedVersionsLoaded: studio.installedLoaded,
    studioLaunchReady: studio.launchReady,
    studioSessionsLoading: studio.sessionsLoading,
    connectedRemoteAppVersion: requiresWinboat
      ? studio.sessions.find((session) => session.connection === "connected")
          ?.version
      : undefined,
    catalogVersions: studio.catalog.versions,
    downloadProgress: studio.downloadProgress,
    isInstalling: studio.isInstalling,
    isBusy,
    resolveVersion: studio.resolveVersion,
    installVersion: studio.installVersion,
    cancelDownload: studio.cancelDownload,
    launchVersion: studio.launchVersion,
    notify,
  });
  const model: ProjectsPageModel = {
    projects: projects.filteredProjects,
    totalProjects: projects.projects.length,
    search: projects.search,
    sharedDirectory: config.sharedDirectory,
    installedSet: studio.installedSet,
    installedVersionsLoaded: studio.installedLoaded,
    studioLaunchReady: studio.launchReady,
    studioSessionsLoading: studio.sessionsLoading,
    connectedRemoteAppVersion: launcher.connectedRemoteAppVersion,
    isLaunching: studio.isLaunching,
    isBusy,
    launchKeyFor: launcher.launchKeyFor,
    preferredVersionFor: launcher.preferredVersionFor,
    launchPendingFor: launcher.launchPendingFor,
    onSearch: projects.setSearch,
    onRefresh: () => void projects.refresh(),
    onOpenWorkspace: () => void projects.openFolder(config.sharedDirectory),
    onOpenFolder: (path) => void projects.openFolder(path),
    onLaunch: launcher.launchProject,
  };

  return (
    <>
      <ProjectsPage t={t} localization={localization} model={model} />
      <ProjectLaunchAssistant
        t={t}
        localization={localization}
        launcher={launcher}
      />
    </>
  );
}
