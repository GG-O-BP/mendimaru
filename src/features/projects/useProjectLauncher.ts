import { useCallback, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type {
  DownloadableVersion,
  DownloadProgress,
  MendixProject,
  StudioVersion,
  ToastKind,
} from "../../domain/types";
import type { Translate } from "../../i18n";

interface ProjectLauncherDependencies {
  t: Translate;
  installedVersions: StudioVersion[];
  installedVersionsLoaded: boolean;
  catalogVersions: DownloadableVersion[];
  downloadProgress: DownloadProgress | null;
  isInstalling: boolean;
  isBusy: (key: string) => boolean;
  resolveVersion: (version: string) => Promise<DownloadableVersion>;
  installVersion: (
    version: DownloadableVersion,
    forceRedownload?: boolean,
    afterInstall?: (installed: StudioVersion) => Promise<void>,
  ) => Promise<void>;
  cancelDownload: () => Promise<void>;
  launchVersion: (
    version: StudioVersion,
    projectMprPath?: string,
    projectName?: string,
    afterLaunch?: () => Promise<void>,
  ) => Promise<void>;
  notify: (kind: ToastKind, title: string, detail?: string) => void;
}

interface LaunchOverride {
  selectedVersion?: string;
  pending: boolean;
}

export type ProjectVersionLookupState = "idle" | "loading" | "error";

export interface ProjectLaunchAssistantState {
  project: MendixProject;
  selectedVersion: string;
  versionInput: string;
  resolvedVersion?: DownloadableVersion;
  lookupState: ProjectVersionLookupState;
  lookupError?: string;
  safetyAcknowledged: boolean;
}

export function useProjectLauncher({
  t,
  installedVersions,
  installedVersionsLoaded,
  catalogVersions,
  downloadProgress,
  isInstalling,
  isBusy,
  resolveVersion,
  installVersion,
  cancelDownload,
  launchVersion,
  notify,
}: ProjectLauncherDependencies) {
  const [assistant, setAssistant] =
    useState<ProjectLaunchAssistantState | null>(null);
  const [overrides, setOverrides] = useState<Map<string, LaunchOverride>>(
    () => new Map(),
  );
  const lookupSequence = useRef(0);
  const actionSequence = useRef(0);
  const dismissalBlocked = useRef(false);
  const preferenceWrites = useRef<Promise<void>>(Promise.resolve());

  const installedByVersion = useMemo(
    () =>
      new Map(installedVersions.map((version) => [version.version, version])),
    [installedVersions],
  );
  const catalogByVersion = useMemo(
    () => new Map(catalogVersions.map((version) => [version.version, version])),
    [catalogVersions],
  );

  const preferredVersionFor = useCallback(
    (project: MendixProject) =>
      overrides.get(project.mprPath)?.selectedVersion ??
      project.preferredVersion,
    [overrides],
  );
  const launchPendingFor = useCallback(
    (project: MendixProject) =>
      overrides.get(project.mprPath)?.pending ?? project.launchPending,
    [overrides],
  );

  const remember = useCallback(
    async (
      project: MendixProject,
      selectedVersion: string | undefined,
      pending: boolean,
    ) => {
      setOverrides((current) => {
        const next = new Map(current);
        next.set(project.mprPath, { selectedVersion, pending });
        return next;
      });
      const write = preferenceWrites.current.then(() =>
        tauriApi.setProjectLaunchPreference(
          project.mprPath,
          selectedVersion,
          pending,
        ),
      );
      preferenceWrites.current = write.catch(() => undefined);
      await write;
    },
    [],
  );

  const reportPreferenceError = useCallback(
    (error: unknown) =>
      notify(
        "error",
        t("project-launch-preference-failed"),
        errorText(error, t),
      ),
    [notify, t],
  );

  const completeLaunch = useCallback(
    async (project: MendixProject, version: StudioVersion) => {
      await launchVersion(version, project.mprPath, project.name, async () => {
        try {
          await remember(project, version.version, false);
        } catch (error) {
          reportPreferenceError(error);
        }
        setAssistant((current) =>
          current?.project.mprPath === project.mprPath ? null : current,
        );
      });
    },
    [launchVersion, remember, reportPreferenceError],
  );

  const resolveForAssistant = useCallback(
    async (project: MendixProject, version: string) => {
      const requested = version.trim();
      const sequence = ++lookupSequence.current;
      setAssistant((current) =>
        current?.project.mprPath === project.mprPath
          ? {
              ...current,
              versionInput: requested,
              lookupState: "loading",
              lookupError: undefined,
            }
          : current,
      );
      let resolved: DownloadableVersion;
      try {
        resolved = await resolveVersion(requested);
        if (sequence !== lookupSequence.current) return;
        setAssistant((current) =>
          current?.project.mprPath === project.mprPath
            ? {
                ...current,
                selectedVersion: resolved.version,
                versionInput: resolved.version,
                resolvedVersion: resolved,
                lookupState: "idle",
                lookupError: undefined,
                safetyAcknowledged: false,
              }
            : current,
        );
      } catch (error) {
        if (sequence !== lookupSequence.current) return;
        setAssistant((current) =>
          current?.project.mprPath === project.mprPath
            ? {
                ...current,
                lookupState: "error",
                lookupError: errorText(error, t),
              }
            : current,
        );
        return;
      }
      try {
        await remember(project, resolved.version, true);
      } catch (error) {
        reportPreferenceError(error);
      }
    },
    [remember, reportPreferenceError, resolveVersion, t],
  );

  const openAssistant = useCallback(
    (project: MendixProject) => {
      actionSequence.current += 1;
      const preferred = preferredVersionFor(project);
      const selected =
        (launchPendingFor(project) && preferred) ||
        project.version ||
        preferred ||
        "";
      const known = selected ? catalogByVersion.get(selected) : undefined;
      setAssistant({
        project,
        selectedVersion: selected,
        versionInput: selected,
        resolvedVersion: known,
        lookupState: "idle",
        safetyAcknowledged: false,
      });
      void remember(project, selected || undefined, true).catch(
        reportPreferenceError,
      );
      if (
        selected &&
        !installedByVersion.has(selected) &&
        !catalogByVersion.has(selected)
      ) {
        void resolveForAssistant(project, selected);
      }
    },
    [
      catalogByVersion,
      installedByVersion,
      launchPendingFor,
      preferredVersionFor,
      remember,
      reportPreferenceError,
      resolveForAssistant,
    ],
  );

  const launchProject = useCallback(
    (project: MendixProject) => {
      if (!installedVersionsLoaded) return;
      if (launchPendingFor(project)) {
        openAssistant(project);
        return;
      }
      const exact = project.version
        ? installedByVersion.get(project.version)
        : undefined;
      if (!exact) {
        openAssistant(project);
        return;
      }
      void remember(project, exact.version, true)
        .then(() => completeLaunch(project, exact))
        .catch(reportPreferenceError);
    },
    [
      completeLaunch,
      installedByVersion,
      installedVersionsLoaded,
      launchPendingFor,
      openAssistant,
      remember,
      reportPreferenceError,
    ],
  );

  const selectVersion = useCallback(
    (version: string) => {
      if (!assistant) return;
      lookupSequence.current += 1;
      actionSequence.current += 1;
      setAssistant({
        ...assistant,
        selectedVersion: version,
        versionInput: version,
        resolvedVersion: catalogByVersion.get(version),
        lookupState: "idle",
        lookupError: undefined,
        safetyAcknowledged: false,
      });
      void remember(assistant.project, version || undefined, true).catch(
        reportPreferenceError,
      );
    },
    [assistant, catalogByVersion, remember, reportPreferenceError],
  );

  const setVersionInput = useCallback((versionInput: string) => {
    lookupSequence.current += 1;
    actionSequence.current += 1;
    setAssistant((current) =>
      current
        ? {
            ...current,
            versionInput,
            lookupState: "idle",
            lookupError: undefined,
          }
        : current,
    );
  }, []);

  const lookupVersion = useCallback(() => {
    if (!assistant?.versionInput.trim()) return;
    void resolveForAssistant(assistant.project, assistant.versionInput);
  }, [assistant, resolveForAssistant]);

  const setSafetyAcknowledged = useCallback((safetyAcknowledged: boolean) => {
    setAssistant((current) =>
      current ? { ...current, safetyAcknowledged } : current,
    );
  }, []);

  const actionBusy = assistant
    ? isBusy(`install-${assistant.selectedVersion}`) ||
      isBusy(`launch-${assistant.selectedVersion}`)
    : false;
  dismissalBlocked.current = actionBusy || isInstalling;

  const closeAssistant = useCallback(() => {
    if (dismissalBlocked.current) return;
    lookupSequence.current += 1;
    actionSequence.current += 1;
    setAssistant(null);
  }, []);

  const continueAssistant = useCallback(() => {
    if (!assistant?.selectedVersion || !installedVersionsLoaded) return;
    const { project, selectedVersion } = assistant;
    const sequence = ++actionSequence.current;
    const installed = installedByVersion.get(selectedVersion);
    const downloadable =
      assistant.resolvedVersion ?? catalogByVersion.get(selectedVersion);
    void remember(project, selectedVersion, true)
      .then(async () => {
        if (sequence !== actionSequence.current) return;
        if (installed) {
          await completeLaunch(project, installed);
          return;
        }
        if (!downloadable) {
          throw new Error(t("project-launch-version-not-resolved"));
        }
        await installVersion(downloadable, false, async (detected) => {
          if (sequence !== actionSequence.current) return;
          await completeLaunch(project, detected);
        });
      })
      .catch(reportPreferenceError);
  }, [
    assistant,
    catalogByVersion,
    completeLaunch,
    installVersion,
    installedByVersion,
    installedVersionsLoaded,
    remember,
    reportPreferenceError,
    t,
  ]);

  const cancelAssistantDownload = useCallback(async () => {
    actionSequence.current += 1;
    await cancelDownload();
  }, [cancelDownload]);

  const launchKeyFor = useCallback(
    (project: MendixProject) => {
      const selected =
        (project.version && installedByVersion.has(project.version)
          ? project.version
          : (preferredVersionFor(project) ?? project.version)) ?? "assistant";
      return `launch-${selected}`;
    },
    [installedByVersion, preferredVersionFor],
  );

  const versionOptions = useMemo(() => {
    const values = new Set<string>();
    for (const version of installedVersions) values.add(version.version);
    for (const version of catalogVersions) values.add(version.version);
    if (assistant?.resolvedVersion)
      values.add(assistant.resolvedVersion.version);
    if (assistant?.selectedVersion) values.add(assistant.selectedVersion);
    return Array.from(values).sort((left, right) =>
      right.localeCompare(left, undefined, { numeric: true }),
    );
  }, [assistant, catalogVersions, installedVersions]);

  const selectedInstalled = assistant
    ? installedByVersion.has(assistant.selectedVersion)
    : false;
  const selectedDownloadable = assistant
    ? Boolean(
        assistant.resolvedVersion ??
        catalogByVersion.get(assistant.selectedVersion),
      )
    : false;
  const safetyRequired = assistant
    ? !assistant.project.version ||
      assistant.selectedVersion !== assistant.project.version
    : false;
  return {
    assistant,
    versionOptions,
    selectedInstalled,
    selectedDownloadable,
    installedVersionsLoaded,
    safetyRequired,
    actionBusy,
    downloadProgress,
    isInstalling,
    launchProject,
    launchKeyFor,
    preferredVersionFor,
    launchPendingFor,
    selectVersion,
    setVersionInput,
    lookupVersion,
    setSafetyAcknowledged,
    continueAssistant,
    closeAssistant,
    cancelDownload: cancelAssistantDownload,
  };
}
