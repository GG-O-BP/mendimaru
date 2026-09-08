import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ConfirmationState,
  LocalizationBundle,
  ToastKind,
  ViewKey,
} from "../domain/types";
import { useProjects } from "../features/projects/useProjects";
import { useOperations } from "../features/operations/useOperations";
import type { EnvironmentController } from "../features/settings/useEnvironment";
import { useStudio } from "../features/studio/useStudio";
import { traceStudioOverview } from "../features/studio/overviewTrace";
import type { Translate } from "../i18n";
import { AppShell } from "./AppShell";
import { ProjectsView } from "./ProjectsView";
import { OperationsView } from "./OperationsView";
import { SettingsView } from "./SettingsView";
import { StudioView } from "./StudioView";

interface WorkspaceProps {
  t: Translate;
  localization: LocalizationBundle;
  languageChanging: boolean;
  warning: string | null;
  environment: EnvironmentController;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
  requestConfirmation: (state: ConfirmationState) => void;
  runAction: (key: string, action: () => Promise<void>) => Promise<void>;
  isBusy: (key: string) => boolean;
  hasBusyPrefix: (prefix: string) => boolean;
  selectLanguage: (language: string) => Promise<LocalizationBundle | undefined>;
  clearFeedback: () => void;
  onWarning: (message: string | null) => void;
}

export function Workspace({
  t,
  localization,
  languageChanging,
  warning,
  environment,
  notify,
  requestConfirmation,
  runAction,
  isBusy,
  hasBusyPrefix,
  selectLanguage,
  clearFeedback,
  onWarning,
}: WorkspaceProps) {
  const [activeView, setActiveView] = useState<ViewKey>("studio");
  const diagnosticFocus = useRef<string | null>(null);
  const openDiagnostics = () => {
    const first =
      environment.readiness.blockingChecks[0] ??
      environment.status?.health?.attentionChecks[0];
    diagnosticFocus.current = first
      ? `environment-diagnostic-${first}`
      : "environment-diagnostics-heading";
    setActiveView("settings");
  };
  useEffect(() => {
    if (activeView !== "settings" || !diagnosticFocus.current) return;
    const target = document.getElementById(diagnosticFocus.current);
    target?.scrollIntoView?.({ block: "center" });
    target?.focus({ preventScroll: true });
    diagnosticFocus.current = null;
  }, [activeView]);
  const processedSetup = useRef(0);
  const studio = useStudio({
    t,
    installedVersionsSourceKey: installedVersionsSourceKey(environment.config),
    notify,
    requestConfirmation,
    runAction,
    isBusy,
    hasBusyPrefix,
    onWarning,
  });
  const projects = useProjects({
    t,
    sharedDirectory: environment.config?.sharedDirectory ?? "",
    onWarning,
    runAction,
  });
  const operations = useOperations({
    t,
    notify,
    requestConfirmation,
    runAction,
    isBusy,
    onWarning,
  });
  const { refresh: refreshProjects, setSearch: setProjectSearch } = projects;
  const { refreshOverview, resetFilters } = studio;
  const {
    setupCompletion,
    updateLanguagePreference,
    refreshStatus,
    winBoatControl: environmentControl,
  } = environment;

  useEffect(() => {
    traceStudioOverview("workspace-ready");
  }, []);

  useEffect(() => {
    const completion = setupCompletion;
    if (!completion || completion.sequence === processedSetup.current) return;
    processedSetup.current = completion.sequence;
    void Promise.all([
      refreshProjects(),
      completion.containerRecreated ? Promise.resolve() : refreshOverview(),
    ]);
  }, [refreshOverview, refreshProjects, setupCompletion]);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      const nextView = (
        {
          "1": "studio",
          "2": "projects",
          "3": "operations",
          "4": "settings",
        } as const
      )[event.key as "1" | "2" | "3" | "4"];
      if (!nextView) return;
      event.preventDefault();
      setActiveView(nextView);
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  const changeLanguage = useCallback(
    async (language: string) => {
      const next = await selectLanguage(language);
      if (!next) return;
      updateLanguagePreference(next.preference);
      resetFilters();
      setProjectSearch("");
      clearFeedback();
      onWarning(null);
      await refreshStatus();
    },
    [
      clearFeedback,
      onWarning,
      refreshStatus,
      resetFilters,
      selectLanguage,
      setProjectSearch,
      updateLanguagePreference,
    ],
  );

  const winBoatControl = {
    ...environmentControl,
    onAction:
      environmentControl.kind === "settings" ||
      environmentControl.kind === "native"
        ? () => setActiveView("settings")
        : environmentControl.onAction,
  };

  return (
    <AppShell
      t={t}
      localization={localization}
      activeView={activeView}
      online={environment.online}
      connectionLabel={environment.connectionLabel}
      attentionRequired={environment.attentionRequired}
      onOpenDiagnostics={openDiagnostics}
      warning={warning}
      languageChanging={languageChanging}
      winBoatControl={winBoatControl}
      onViewChange={setActiveView}
      onLanguageChange={(language) => void changeLanguage(language)}
      onDismissWarning={() => onWarning(null)}
    >
      {activeView === "studio" && (
        <StudioView
          t={t}
          localization={localization}
          environment={environment}
          studio={studio}
          isBusy={isBusy}
          onOpenDiagnostics={openDiagnostics}
        />
      )}
      {activeView === "projects" && environment.config && (
        <ProjectsView
          t={t}
          localization={localization}
          config={environment.config}
          requiresWinboat={Boolean(
            environment.status?.platform.requiresWinboat,
          )}
          environmentReady={environment.readiness.projects}
          installationReady={environment.readiness.installation}
          projects={projects}
          studio={studio}
          isBusy={isBusy}
          notify={notify}
        />
      )}
      {activeView === "settings" && (
        <SettingsView t={t} environment={environment} isBusy={isBusy} />
      )}
      {activeView === "operations" && (
        <OperationsView
          t={t}
          localization={localization}
          operations={operations}
        />
      )}
    </AppShell>
  );
}

function installedVersionsSourceKey(config: EnvironmentController["config"]) {
  if (!config) return "unconfigured";
  return JSON.stringify([
    config.containerRuntime,
    config.containerName,
    config.apiUrl,
    config.mendixInstallRoot,
    config.mendixDataRoot,
    config.windowsStudioPaths,
  ]);
}
