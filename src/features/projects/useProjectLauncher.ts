import { useCallback } from "react";
import type {
  ConfirmationState,
  MendixProject,
  StudioVersion,
  ToastKind,
} from "../../domain/types";
import type { Translate } from "../../i18n";

interface ProjectLauncherDependencies {
  t: Translate;
  installedVersions: StudioVersion[];
  launchVersion: (
    version: StudioVersion,
    projectMprPath?: string,
    projectName?: string,
  ) => Promise<void>;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
}

export function useProjectLauncher({
  t,
  installedVersions,
  launchVersion,
  notify,
  requestConfirmation,
}: ProjectLauncherDependencies) {
  const launchProject = useCallback(
    (project: MendixProject) => {
      const exact = installedVersions.find(
        (version) => version.version === project.version,
      );
      const fallback = exact ?? installedVersions[0];
      if (!fallback) {
        notify("error", t("toast-no-studio"), t("toast-no-studio-detail"));
        return;
      }
      if (project.version && !exact) {
        requestConfirmation({
          title: t("confirm-open-fallback-title", {
            version: fallback.version,
          }),
          description: t("confirm-project-version-mismatch", {
            projectVersion: project.version,
          }),
          confirmLabel: t("action-open-anyway"),
          action: () => launchVersion(fallback, project.mprPath, project.name),
        });
        return;
      }
      void launchVersion(fallback, project.mprPath, project.name);
    },
    [installedVersions, launchVersion, notify, requestConfirmation, t],
  );

  const launchKeyFor = useCallback(
    (project: MendixProject) => {
      const selected =
        installedVersions.find(
          (version) => version.version === project.version,
        ) ?? installedVersions[0];
      return `launch-${selected?.version ?? "unavailable"}`;
    },
    [installedVersions],
  );

  return { launchProject, launchKeyFor };
}
