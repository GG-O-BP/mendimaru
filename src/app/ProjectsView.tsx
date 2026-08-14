import type {
  AppConfig,
  ConfirmationState,
  LocalizationBundle,
  ToastKind,
} from "../domain/types";
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
  projects,
  studio,
  isBusy,
  notify,
  requestConfirmation,
  onFindVersion,
}: {
  t: Translate;
  localization: LocalizationBundle;
  config: AppConfig;
  projects: ReturnType<typeof useProjects>;
  studio: ReturnType<typeof useStudio>;
  isBusy: (key: string) => boolean;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
  onFindVersion: (version: string) => void;
}) {
  const { launchProject, launchKeyFor } = useProjectLauncher({
    t,
    installedVersions: studio.installedVersions,
    launchVersion: studio.launchVersion,
    notify,
    requestConfirmation,
  });
  const model: ProjectsPageModel = {
    projects: projects.filteredProjects,
    totalProjects: projects.projects.length,
    search: projects.search,
    sharedDirectory: config.sharedDirectory,
    installedSet: studio.installedSet,
    isLaunching: studio.isLaunching,
    isBusy,
    launchKeyFor,
    onSearch: projects.setSearch,
    onRefresh: () => void projects.refresh(),
    onOpenWorkspace: () => void projects.openFolder(config.sharedDirectory),
    onOpenFolder: (path) => void projects.openFolder(path),
    onLaunch: launchProject,
    onFindVersion,
  };

  return <ProjectsPage t={t} localization={localization} model={model} />;
}
