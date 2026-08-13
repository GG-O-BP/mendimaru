import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Anchor,
  AppWindow,
  CheckCircle2,
  ChevronRight,
  Download,
  FolderKanban,
  FolderOpen,
  HardDrive,
  Languages,
  Info,
  LoaderCircle,
  Monitor,
  Play,
  RefreshCw,
  Search,
  Server,
  Settings,
  Trash2,
  X,
  XCircle,
  type LucideIcon,
} from "lucide-react";
import type {
  AppConfig,
  CommandError,
  ConfirmationState,
  DownloadableVersion,
  DownloadProgress,
  EnvironmentStatus,
  LocalizationBundle,
  MendixProject,
  SettingsSaveResult,
  StudioVersion,
  StudioVersionCatalog,
  ToastKind,
  ToastMessage,
  ViewKey,
} from "./types";
import {
  applyDocumentLocale,
  createTranslate,
  formatByteValues,
  formatDates,
  formatNumbers,
  loadLocalization,
  selectLanguage,
  type Translate,
} from "./i18n";
import "./App.css";

const EMPTY_CATALOG: StudioVersionCatalog = {
  versions: [],
  loadedPages: [],
};

const TABS: Array<{ key: ViewKey; labelKey: string; icon: LucideIcon }> = [
  { key: "studio", labelKey: "nav-studio", icon: AppWindow },
  { key: "projects", labelKey: "nav-projects", icon: FolderKanban },
  { key: "settings", labelKey: "nav-settings", icon: Settings },
];

type PathField = "sharedDirectory" | "composeFile" | "winboatExecutable";
type VersionSupportFilter = "lts" | "mts";
type VersionSupportFilters = Record<VersionSupportFilter, boolean>;
type WinBoatControlKind = "settings" | "setup" | "open" | "start";

const EMPTY_VERSION_SUPPORT_FILTERS: VersionSupportFilters = { lts: false, mts: false };

function App() {
  const [localization, setLocalization] = useState<LocalizationBundle | null>(null);
  const [activeView, setActiveView] = useState<ViewKey>("studio");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [draftConfig, setDraftConfig] = useState<AppConfig | null>(null);
  const [status, setStatus] = useState<EnvironmentStatus | null>(null);
  const [installedVersions, setInstalledVersions] = useState<StudioVersion[]>([]);
  const [catalog, setCatalog] = useState<StudioVersionCatalog>(EMPTY_CATALOG);
  const [projects, setProjects] = useState<MendixProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [busyActions, setBusyActions] = useState<Set<string>>(new Set());
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  const [confirmation, setConfirmation] = useState<ConfirmationState | null>(null);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [versionSearch, setVersionSearch] = useState("");
  const [versionSupportFilters, setVersionSupportFilters] =
    useState<VersionSupportFilters>(EMPTY_VERSION_SUPPORT_FILTERS);
  const [projectSearch, setProjectSearch] = useState("");
  const [applyMountNow, setApplyMountNow] = useState(true);
  const [languageChanging, setLanguageChanging] = useState(false);
  const toastId = useRef(0);
  const initialized = useRef(false);
  const actionLocks = useRef<Set<string>>(new Set());
  const catalogRequestInFlight = useRef(false);
  const launchLock = useRef(false);
  const languageChangeLock = useRef(false);
  const setupCompletionScheduled = useRef(false);
  const localizationRef = useRef<LocalizationBundle | null>(null);
  const t = useMemo(() => createTranslate(localization), [localization]);

  const applyLocalization = useCallback((bundle: LocalizationBundle) => {
    localizationRef.current = bundle;
    setLocalization(bundle);
    applyDocumentLocale(bundle);
    return bundle;
  }, []);

  const notify = useCallback((kind: ToastKind, title: string, detail?: string) => {
    const id = ++toastId.current;
    setToasts((current) => [...current, { id, kind, title, detail }]);
    window.setTimeout(() => {
      setToasts((current) => current.filter((toast) => toast.id !== id));
    }, 5000);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await invoke<EnvironmentStatus>("get_environment_status"));
    } catch (error) {
      setWarning(errorText(error, t));
    }
  }, [t]);

  const refreshInstalled = useCallback(async () => {
    try {
      setInstalledVersions(await invoke<StudioVersion[]>("get_installed_versions"));
    } catch (error) {
      setWarning(errorText(error, t));
    }
  }, [t]);

  const refreshProjects = useCallback(async () => {
    try {
      setProjects(await localizeProjects(await invoke<MendixProject[]>("get_projects")));
    } catch (error) {
      setWarning(errorText(error, t));
    }
  }, [t]);

  const fetchCatalogPage = useCallback(async (page: number, reset = false) => {
    if (catalogRequestInFlight.current) return;
    catalogRequestInFlight.current = true;
    setCatalogLoading(true);
    setCatalogError(null);
    try {
      const next = await invoke<StudioVersionCatalog>("fetch_downloadable_versions", {
        page,
        reset,
      });
      setCatalog(await localizeCatalog(next));
    } catch (error) {
      setCatalogError(errorText(error, t));
    } finally {
      catalogRequestInFlight.current = false;
      setCatalogLoading(false);
    }
  }, [t]);

  const loadInitialData = useCallback(async () => {
    setWarning(null);
    try {
      const [loadedLocalization, loadedConfig] = await Promise.all([
        loadLocalization(),
        invoke<AppConfig>("get_config"),
      ]);
      applyLocalization(loadedLocalization);
      setConfig(loadedConfig);
      setDraftConfig(loadedConfig);

      const [statusResult, installedResult, projectResult, cacheResult] = await Promise.allSettled([
        invoke<EnvironmentStatus>("get_environment_status"),
        invoke<StudioVersion[]>("get_installed_versions"),
        invoke<MendixProject[]>("get_projects"),
        invoke<StudioVersionCatalog>("get_downloadable_versions_cache"),
      ]);

      if (statusResult.status === "fulfilled") setStatus(statusResult.value);
      if (installedResult.status === "fulfilled") setInstalledVersions(installedResult.value);
      if (projectResult.status === "fulfilled") {
        setProjects(await localizeProjects(projectResult.value));
      }
      if (cacheResult.status === "fulfilled") {
        setCatalog(await localizeCatalog(cacheResult.value));
      }

      const loadedTranslate = createTranslate(loadedLocalization);
      const expectedInstalledOffline =
        statusResult.status === "fulfilled" && !statusResult.value.guestOnline;
      const errors = [
        statusResult,
        ...(expectedInstalledOffline ? [] : [installedResult]),
        projectResult,
      ]
        .filter((result) => result.status === "rejected")
        .map((result) => errorText((result as PromiseRejectedResult).reason, loadedTranslate));
      if (errors.length) setWarning(errors[0]);
    } catch (error) {
      setWarning(errorText(error, createTranslate(localizationRef.current)));
    } finally {
      setLoading(false);
    }
  }, [applyLocalization]);

  useEffect(() => {
    if (!initialized.current) {
      initialized.current = true;
      void loadInitialData().then(() => fetchCatalogPage(1));
    }

    const interval = window.setInterval(() => void refreshStatus(), 15_000);
    let unlisten: (() => void) | undefined;
    void listen<DownloadProgress>("studio-download-progress", (event) => {
      setDownloadProgress((current) => {
        const incoming = event.payload;
        if (!current || current.version !== incoming.version) return incoming;
        return {
          ...incoming,
          percentage: Math.max(current.percentage ?? 0, incoming.percentage ?? 0),
        };
      });
    }).then((dispose) => {
      unlisten = dispose;
    });

    return () => {
      window.clearInterval(interval);
      unlisten?.();
    };
  }, [fetchCatalogPage, loadInitialData, refreshStatus]);

  useEffect(() => {
    if (downloadProgress?.state !== "installed") return undefined;
    const completedVersion = downloadProgress.version;
    const timeout = window.setTimeout(() => {
      setDownloadProgress((current) =>
        current?.version === completedVersion && current.state === "installed"
          ? null
          : current,
      );
    }, 2200);
    return () => window.clearTimeout(timeout);
  }, [downloadProgress?.state, downloadProgress?.version]);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      const nextView = ({ "1": "studio", "2": "projects", "3": "settings" } as const)[
        event.key as "1" | "2" | "3"
      ];
      if (!nextView) return;
      event.preventDefault();
      setActiveView(nextView);
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  const runAction = useCallback(
    async (key: string, action: () => Promise<void>) => {
      if (actionLocks.current.has(key)) return;
      actionLocks.current.add(key);
      setBusyActions((current) => new Set(current).add(key));
      try {
        await action();
      } catch (error) {
        const currentTranslate = createTranslate(localizationRef.current);
        notify(
          "error",
          currentTranslate("generic-action-failed"),
          errorText(error, currentTranslate),
        );
      } finally {
        actionLocks.current.delete(key);
        setBusyActions((current) => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
      }
    },
    [notify],
  );

  const startWindows = () =>
    runAction("start-windows", async () => {
      await invoke("start_winboat_windows");
      notify("info", t("toast-windows-started"), t("toast-windows-started-detail"));
      window.setTimeout(() => void refreshStatus(), 3500);
    });

  const openWinBoat = () =>
    runAction("open-winboat", async () => {
      await invoke("open_winboat");
    });

  const beginWinBoatSetup = () =>
    runAction("setup-winboat", async () => {
      await invoke("begin_winboat_setup");
      setConfig((current) =>
        current ? { ...current, winboatSetupPending: true } : current,
      );
      setDraftConfig((current) =>
        current ? { ...current, winboatSetupPending: true } : current,
      );
      notify("info", t("toast-winboat-setup-opened"), t("toast-winboat-setup-opened-detail"));
      await refreshStatus();
    });

  useEffect(() => {
    if (!status?.setupPending || !status.guestOnline) {
      if (!status?.setupPending) setupCompletionScheduled.current = false;
      return undefined;
    }
    if (setupCompletionScheduled.current) return undefined;
    setupCompletionScheduled.current = true;

    const timeout = window.setTimeout(() => {
      void runAction("complete-winboat-setup", async () => {
        const result = await invoke<SettingsSaveResult>("complete_winboat_setup");
        setConfig(result.config);
        setDraftConfig(result.config);
        notify(
          "success",
          t("toast-winboat-setup-complete"),
          result.containerRecreated ? t("toast-winboat-setup-complete-reconnected") : undefined,
        );
        await Promise.all([refreshStatus(), refreshProjects()]);
        if (!result.containerRecreated) await refreshInstalled();
      }).finally(() => {
        setupCompletionScheduled.current = false;
      });
    }, 5000);

    return () => {
      window.clearTimeout(timeout);
      setupCompletionScheduled.current = false;
    };
  }, [notify, refreshInstalled, refreshProjects, refreshStatus, runAction, status, t]);

  const launchVersion = (version: StudioVersion, project?: MendixProject) => {
    if (launchLock.current) return Promise.resolve();
    launchLock.current = true;
    return runAction(`launch-${version.version}`, async () => {
      await invoke("launch_studio_pro", {
        version: version.version,
        projectMprPath: project?.mprPath ?? null,
      });
      notify(
        "success",
        t("toast-studio-opened", { version: version.version }),
        project ? t("toast-project-opened", { project: project.name }) : undefined,
      );
    }).finally(() => {
      launchLock.current = false;
    });
  };

  const launchProject = (project: MendixProject) => {
    const exact = installedVersions.find((version) => version.version === project.version);
    const fallback = exact ?? installedVersions[0];
    if (!fallback) {
      notify("error", t("toast-no-studio"), t("toast-no-studio-detail"));
      return;
    }
    if (project.version && !exact) {
      setConfirmation({
        title: t("confirm-open-fallback-title", { version: fallback.version }),
        description: t("confirm-project-version-mismatch", {
          projectVersion: project.version,
        }),
        confirmLabel: t("action-open-anyway"),
        action: () => launchVersion(fallback, project),
      });
      return;
    }
    void launchVersion(fallback, project);
  };

  const askInstall = (version: DownloadableVersion) => {
    setConfirmation({
      title: t("confirm-install-title", { version: version.version }),
      description: t("confirm-install-description"),
      confirmLabel: t("action-download-install"),
      action: () =>
        runAction(`install-${version.version}`, async () => {
          const zeroBytesLabel = (await formatByteValues([0]))[0] ?? "0 B";
          setDownloadProgress({
            version: version.version,
            state: "starting",
            downloadedBytes: 0,
            downloadedBytesLabel: zeroBytesLabel,
            percentage: 2,
            estimated: true,
            message: t("progress-starting"),
          });
          try {
            await invoke("install_studio_pro", { version: version.version });
            notify("success", t("toast-install-complete", { version: version.version }));
            await refreshInstalled();
          } catch (error) {
            const message = errorText(error, t);
            const cancelled = errorCode(error) === "download_cancelled";
            setDownloadProgress((current) => ({
              version: version.version,
              state: cancelled || current?.state === "cancelled" ? "cancelled" : "failed",
              downloadedBytes:
                current?.version === version.version ? current.downloadedBytes : 0,
              downloadedBytesLabel:
                current?.version === version.version
                  ? current.downloadedBytesLabel
                  : zeroBytesLabel,
              totalBytes:
                current?.version === version.version ? current.totalBytes : undefined,
              totalBytesLabel:
                current?.version === version.version ? current.totalBytesLabel : undefined,
              percentage:
                current?.version === version.version ? current.percentage : undefined,
              estimated:
                current?.version === version.version ? current.estimated : undefined,
              message,
            }));
            if (cancelled) return;
            throw error;
          }
        }),
    });
  };

  const askUninstall = (version: StudioVersion) => {
    setConfirmation({
      title: t("confirm-uninstall-title", { version: version.version }),
      description: t("confirm-uninstall-description"),
      confirmLabel: t("action-uninstall"),
      danger: true,
      action: () =>
        runAction(`uninstall-${version.version}`, async () => {
          await invoke("uninstall_studio_pro", { version: version.version });
          setInstalledVersions((current) =>
            current.filter((installed) => installed.version !== version.version),
          );
          await refreshInstalled();
          notify("success", t("toast-uninstall-complete", { version: version.version }));
        }),
    });
  };

  const cancelDownload = () =>
    runAction("cancel-download", async () => {
      if (await invoke<boolean>("cancel_studio_download")) {
        notify("info", t("toast-download-cancel-requested"));
      }
    });

  const openFolder = (path: string) =>
    runAction(`folder-${path}`, async () => {
      await invoke("open_linux_folder", { path });
    });

  const choosePath = async (field: PathField) => {
    if (!draftConfig) return;
    try {
      const directory = field === "sharedDirectory";
      const selected = await open({
        directory,
        multiple: false,
        defaultPath: draftConfig[field],
        title:
          field === "sharedDirectory"
            ? t("dialog-select-shared-directory")
            : field === "composeFile"
              ? t("dialog-select-compose-file")
              : t("dialog-select-winboat-file"),
        filters:
          field === "composeFile"
            ? [{ name: t("dialog-compose-filter"), extensions: ["yml", "yaml"] }]
            : undefined,
      });
      if (typeof selected === "string") {
        setDraftConfig((current) => (current ? { ...current, [field]: selected } : current));
      }
    } catch (error) {
      notify("error", t("path-picker-failed"), errorText(error, t));
    }
  };

  const saveSettings = () => {
    if (!draftConfig) return;
    const execute = () =>
      runAction("save-settings", async () => {
        const result = await invoke<SettingsSaveResult>("save_config", {
          config: draftConfig,
          applyMount: applyMountNow,
        });
        setConfig(result.config);
        setDraftConfig(result.config);
        notify(
          "success",
          result.containerRecreated ? t("toast-settings-applied") : t("toast-settings-saved"),
          result.mountChanged && !result.containerRecreated
            ? t("toast-mount-deferred")
            : undefined,
        );
        await Promise.all([refreshStatus(), refreshProjects()]);
      });

    if (applyMountNow && status?.containerStatus === "running") {
      setConfirmation({
        title: t("confirm-apply-mount-title"),
        description: t("confirm-apply-mount-description"),
        confirmLabel: t("action-save-reconnect"),
        action: execute,
      });
    } else {
      void execute();
    }
  };

  const redetectSettings = () =>
    runAction("redetect", async () => {
      const detected = await invoke<AppConfig>("redetect_config");
      setConfig(detected);
      setDraftConfig(detected);
      notify("success", t("toast-redetected"));
      await Promise.all([refreshStatus(), refreshProjects()]);
    });

  const changeLanguage = async (language: string) => {
    if (languageChangeLock.current) return;
    languageChangeLock.current = true;
    setLanguageChanging(true);
    try {
      const next = applyLocalization(await selectLanguage(language));
      setConfig((current) =>
        current ? { ...current, languagePreference: next.preference } : current,
      );
      setDraftConfig((current) =>
        current ? { ...current, languagePreference: next.preference } : current,
      );
      setVersionSearch("");
      setProjectSearch("");
      setConfirmation(null);
      setToasts([]);
      setWarning(null);
      setCatalogError(null);
      const progressSnapshot = downloadProgress;
      const releaseDateSnapshot = catalog.versions.map((version) => ({
        id: version.version,
        source: version.releaseDate ?? "",
      }));
      const projectDateSnapshot = projects.map((project) => ({
        id: project.mprPath,
        source: project.lastModified ?? "",
      }));
      const byteValues = progressSnapshot
        ? [progressSnapshot.downloadedBytes].concat(
            progressSnapshot.totalBytes == null ? [] : [progressSnapshot.totalBytes],
          )
        : [];
      const [nextStatus, nextDates, nextByteLabels] = await Promise.all([
        invoke<EnvironmentStatus>("get_environment_status"),
        formatDates(
          releaseDateSnapshot
            .map(({ source }) => source)
            .concat(projectDateSnapshot.map(({ source }) => source)),
        ),
        formatByteValues(byteValues),
      ]);
      setStatus(nextStatus);
      const releaseDates = new Map(
        releaseDateSnapshot.map(({ id, source }, index) => [
          `${id}\0${source}`,
          nextDates[index],
        ]),
      );
      const projectDateOffset = releaseDateSnapshot.length;
      const projectDates = new Map(
        projectDateSnapshot.map(({ id, source }, index) => [
          `${id}\0${source}`,
          nextDates[projectDateOffset + index],
        ]),
      );
      setCatalog((current) => ({
        ...current,
        versions: current.versions.map((version) => ({
          ...version,
          formattedReleaseDate:
            releaseDates.get(`${version.version}\0${version.releaseDate ?? ""}`) ??
            version.formattedReleaseDate,
        })),
      }));
      setProjects((current) =>
        current.map((project) => ({
          ...project,
          formattedLastModified:
            projectDates.get(`${project.mprPath}\0${project.lastModified ?? ""}`) ??
            project.formattedLastModified,
        })),
      );
      if (progressSnapshot) {
        const nextTranslate = createTranslate(next);
        setDownloadProgress((current) =>
          current
            ? {
                ...current,
                downloadedBytesLabel:
                  current.downloadedBytes === progressSnapshot.downloadedBytes
                    ? nextByteLabels[0] ?? current.downloadedBytesLabel
                    : current.downloadedBytesLabel,
                totalBytesLabel:
                  current.totalBytes == null
                    ? undefined
                    : current.totalBytes === progressSnapshot.totalBytes
                      ? nextByteLabels[1] ?? current.totalBytesLabel
                      : current.totalBytesLabel,
                message:
                  current.state === "failed"
                    ? nextTranslate("progress-failed")
                    : current.message,
              }
            : current,
        );
      }
    } catch (error) {
      const currentTranslate = createTranslate(localizationRef.current);
      notify(
        "error",
        currentTranslate("generic-action-failed"),
        errorText(error, currentTranslate),
      );
    } finally {
      languageChangeLock.current = false;
      setLanguageChanging(false);
    }
  };

  const online = Boolean(status?.guestOnline);
  const isBusy = (key: string) => busyActions.has(key);
  const winBoatControlKind: WinBoatControlKind = !status?.winboatAvailable
    ? "settings"
    : !status.winboatInitialized
      ? "setup"
      : online || status.containerStatus === "running"
        ? "open"
        : "start";
  const winBoatActionKey =
    winBoatControlKind === "setup"
      ? "setup-winboat"
      : winBoatControlKind === "open"
        ? "open-winboat"
        : winBoatControlKind === "start"
          ? "start-windows"
          : "winboat-settings";
  const winBoatActionLabel =
    winBoatControlKind === "settings"
      ? t("action-check-winboat-settings")
      : winBoatControlKind === "setup"
        ? t(status?.setupPending ? "action-continue-winboat-setup" : "action-setup-winboat")
        : winBoatControlKind === "open"
          ? t(status?.setupPending && !online ? "action-open-winboat-setup" : "action-open-winboat")
          : t("action-start-windows");
  const offlineGuidance = !status?.winboatAvailable
    ? {
        title: t("winboat-missing-title"),
        detail: t("winboat-missing-detail"),
      }
    : status.setupPending
      ? {
          title: t("winboat-setup-pending-title"),
          detail: t("winboat-setup-pending-detail"),
        }
      : !status.winboatInitialized
        ? {
            title: t("winboat-setup-required-title"),
            detail: t("winboat-setup-required-detail"),
          }
        : status.containerStatus === "running"
          ? {
              title: t("windows-preparing-title"),
              detail: t("windows-preparing-detail"),
            }
          : {
              title: t("offline-guidance-title"),
              detail: t("offline-guidance-detail"),
            };
  const runPrimaryWinBoatAction = () => {
    if (winBoatControlKind === "settings") {
      setActiveView("settings");
    } else if (winBoatControlKind === "setup") {
      void beginWinBoatSetup();
    } else if (winBoatControlKind === "open") {
      void openWinBoat();
    } else {
      void startWindows();
    }
  };
  const isLaunching = Array.from(busyActions).some((key) => key.startsWith("launch-"));
  const isInstalling = Array.from(busyActions).some((key) => key.startsWith("install-"));
  const installedSet = useMemo(
    () => new Set(installedVersions.map((version) => version.version)),
    [installedVersions],
  );
  const filteredCatalog = useMemo(() => {
    const needle = versionSearch.trim().toLowerCase();
    return catalog.versions.filter((version) => {
      const matchesSearch = !needle || version.version.toLowerCase().includes(needle);
      const supportFilterEnabled = versionSupportFilters.lts || versionSupportFilters.mts;
      const matchesSupport =
        !supportFilterEnabled ||
        (versionSupportFilters.lts && version.isLts) ||
        (versionSupportFilters.mts && version.isMts);
      return matchesSearch && matchesSupport;
    });
  }, [catalog.versions, versionSearch, versionSupportFilters]);
  const filteredProjects = useMemo(() => {
    const needle = projectSearch.trim().toLowerCase();
    return needle
      ? projects.filter((project) =>
          `${project.name} ${project.directory} ${project.version ?? ""}`.toLowerCase().includes(needle),
        )
      : projects;
  }, [projectSearch, projects]);
  const nextCatalogPage = Math.max(0, ...catalog.loadedPages) + 1;
  const hasMoreVersions = catalog.totalCount
    ? catalog.versions.length < catalog.totalCount
    : catalog.loadedPages.length > 0 && catalog.versions.length >= catalog.loadedPages.length * 10;
  const loadMoreCatalog = useCallback(() => {
    void fetchCatalogPage(nextCatalogPage);
  }, [fetchCatalogPage, nextCatalogPage]);
  const settingsChanged = Boolean(
    config && draftConfig && JSON.stringify(config) !== JSON.stringify(draftConfig),
  );

  if (loading || !localization) return <LoadingScreen />;

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
              onClick={() => setActiveView(key)}
              aria-current={activeView === key ? "page" : undefined}
              aria-keyshortcuts={`Control+${index + 1}`}
              title={`${t(labelKey)} · Ctrl+${index + 1}`}
            >
              <span className="nav-icon"><Icon size={19} /></span>
              <span className="nav-copy">{t(labelKey)}<small>0{index + 1}</small></span>
              <ChevronRight className="nav-arrow" size={16} aria-hidden="true" />
            </button>
          ))}
        </nav>

        <div className="sidebar-waterline" aria-hidden="true">
          <i /><i /><i />
        </div>
      </aside>

      <div className="app-workspace">
        <header className="app-header">
          <div className={`route-status ${online ? "online" : "offline"}`} aria-label={t("route-aria")}>
            <span className="route-node host-node">
              <Server size={16} aria-hidden="true" />
              <span>{t("route-linux")}</span>
            </span>
            <span className="route-track" aria-hidden="true"><i /></span>
            <span className="route-node windows-node">
              <Monitor size={16} aria-hidden="true" />
              <span>{t("route-windows")}</span>
            </span>
            <strong><i />{online ? t("connection-online") : t("connection-offline")}</strong>
          </div>

          <div className="winboat-control">
            <label className="language-control" title={t("language-label")}>
              <Languages size={16} aria-hidden="true" />
              <span className="sr-only">{t("language-label")}</span>
              <select
                value={localization.preference}
                onChange={(event) => void changeLanguage(event.target.value)}
                aria-label={t("language-label")}
                aria-busy={languageChanging}
                disabled={languageChanging}
              >
                <option value="system">{t("language-system")}</option>
                {localization.availableLocales.map((locale) => (
                  <option key={locale.id} value={locale.id}>{locale.nativeName}</option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className={`button ${online ? "secondary" : "primary"}`}
              onClick={runPrimaryWinBoatAction}
              disabled={winBoatControlKind !== "settings" && isBusy(winBoatActionKey)}
            >
              {isBusy(winBoatActionKey) ? (
                <LoaderCircle size={17} className="spin" />
              ) : winBoatControlKind === "settings" || winBoatControlKind === "setup" ? (
                <Settings size={17} />
              ) : winBoatControlKind === "open" ? (
                <Monitor size={17} />
              ) : (
                <Play size={17} />
              )}
              {winBoatActionLabel}
            </button>
          </div>
        </header>

        {warning && (
          <div className="global-warning" role="alert">
            <Info size={18} />
            <span>{warning}</span>
            <button type="button" onClick={() => setWarning(null)} aria-label={t("dismiss-notification")}><X size={17} /></button>
          </div>
        )}

        <main className="page" id="main-content">
          {activeView === "studio" && (
            <StudioView
              t={t}
              localization={localization}
              online={online}
              installed={installedVersions}
              available={filteredCatalog}
              availableTotal={catalog.totalCount}
              loadedCount={catalog.versions.length}
              search={versionSearch}
              supportFilters={versionSupportFilters}
              catalogLoading={catalogLoading}
              catalogError={catalogError}
              hasMore={hasMoreVersions}
              installedSet={installedSet}
              downloadProgress={downloadProgress}
              isLaunching={isLaunching}
              isInstalling={isInstalling}
              isBusy={isBusy}
              offlineTitle={offlineGuidance.title}
              offlineDetail={offlineGuidance.detail}
              winBoatControlKind={winBoatControlKind}
              winBoatActionLabel={winBoatActionLabel}
              winBoatActionBusy={isBusy(winBoatActionKey)}
              onSearch={setVersionSearch}
              onToggleSupportFilter={(filter) =>
                setVersionSupportFilters((current) => ({
                  ...current,
                  [filter]: !current[filter],
                }))
              }
              onWinBoatAction={runPrimaryWinBoatAction}
              onRefreshInstalled={() => void refreshInstalled()}
              onRefreshCatalog={() => void fetchCatalogPage(1, true)}
              onLoadMore={loadMoreCatalog}
              onLaunch={(version) => void launchVersion(version)}
              onInstall={askInstall}
              onUninstall={askUninstall}
              onCancelDownload={cancelDownload}
            />
          )}

          {activeView === "projects" && config && (
            <ProjectsView
              t={t}
              localization={localization}
              projects={filteredProjects}
              totalProjects={projects.length}
              search={projectSearch}
              sharedDirectory={config.sharedDirectory}
              installedSet={installedSet}
              isLaunching={isLaunching}
              isBusy={isBusy}
              onSearch={setProjectSearch}
              onRefresh={() => void refreshProjects()}
              onOpenWorkspace={() => void openFolder(config.sharedDirectory)}
              onOpenFolder={(path) => void openFolder(path)}
              onLaunch={launchProject}
              onFindVersion={(version) => {
                setVersionSearch(version);
                setVersionSupportFilters(EMPTY_VERSION_SUPPORT_FILTERS);
                setActiveView("studio");
              }}
            />
          )}

          {activeView === "settings" && draftConfig && (
            <SettingsView
              t={t}
              config={draftConfig}
              changed={settingsChanged}
              mountMatches={Boolean(status?.sharedMountMatches)}
              applyNow={applyMountNow}
              isBusy={isBusy}
              onChange={setDraftConfig}
              onChoose={(field) => void choosePath(field)}
              onApplyNow={setApplyMountNow}
              onSave={saveSettings}
              onRedetect={redetectSettings}
            />
          )}
        </main>
      </div>

      <ToastStack
        t={t}
        toasts={toasts}
        onDismiss={(id) => setToasts((current) => current.filter((toast) => toast.id !== id))}
      />
      {confirmation && (
        <ConfirmDialog
          t={t}
          state={confirmation}
          onCancel={() => setConfirmation(null)}
          onConfirm={() => {
            const action = confirmation.action;
            setConfirmation(null);
            void action();
          }}
        />
      )}
    </div>
  );
}

function StudioView({
  t,
  localization,
  online,
  installed,
  available,
  availableTotal,
  loadedCount,
  search,
  supportFilters,
  catalogLoading,
  catalogError,
  hasMore,
  installedSet,
  downloadProgress,
  isLaunching,
  isInstalling,
  isBusy,
  offlineTitle,
  offlineDetail,
  winBoatControlKind,
  winBoatActionLabel,
  winBoatActionBusy,
  onSearch,
  onToggleSupportFilter,
  onWinBoatAction,
  onRefreshInstalled,
  onRefreshCatalog,
  onLoadMore,
  onLaunch,
  onInstall,
  onUninstall,
  onCancelDownload,
}: {
  t: Translate;
  localization: LocalizationBundle;
  online: boolean;
  installed: StudioVersion[];
  available: DownloadableVersion[];
  availableTotal?: number;
  loadedCount: number;
  search: string;
  supportFilters: VersionSupportFilters;
  catalogLoading: boolean;
  catalogError: string | null;
  hasMore: boolean;
  installedSet: Set<string>;
  downloadProgress: DownloadProgress | null;
  isLaunching: boolean;
  isInstalling: boolean;
  isBusy: (key: string) => boolean;
  offlineTitle: string;
  offlineDetail: string;
  winBoatControlKind: WinBoatControlKind;
  winBoatActionLabel: string;
  winBoatActionBusy: boolean;
  onSearch: (value: string) => void;
  onToggleSupportFilter: (value: VersionSupportFilter) => void;
  onWinBoatAction: () => void;
  onRefreshInstalled: () => void;
  onRefreshCatalog: () => void;
  onLoadMore: () => void;
  onLaunch: (version: StudioVersion) => void;
  onInstall: (version: DownloadableVersion) => void;
  onUninstall: (version: StudioVersion) => void;
  onCancelDownload: () => void;
}) {
  const loadMoreSentinel = useRef<HTMLDivElement>(null);
  const [installedCountLabel, loadedCountLabel, availableTotalLabel] = useLocalizedNumbers(
    [installed.length, loadedCount, availableTotal ?? 0],
    localization,
  );

  useEffect(() => {
    const sentinel = loadMoreSentinel.current;
    if (!sentinel || !hasMore || catalogLoading || catalogError) return undefined;

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        observer.disconnect();
        onLoadMore();
      },
      {
        root: sentinel.closest(".page"),
        rootMargin: "0px 0px 240px 0px",
      },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [catalogError, catalogLoading, hasMore, onLoadMore]);

  return (
    <div className="studio-page">
      <PageTitle
        eyebrow={t("studio-eyebrow")}
        title={t("nav-studio")}
        description={t("studio-description")}
      />

      {!online && (
        <aside className="route-notice" aria-labelledby="offline-guidance-title">
          <span className="route-notice-icon"><Anchor size={22} /></span>
          <div>
            <strong id="offline-guidance-title">{offlineTitle}</strong>
            <p>{offlineDetail}</p>
          </div>
          <button
            type="button"
            className="button primary"
            onClick={onWinBoatAction}
            disabled={winBoatControlKind !== "settings" && winBoatActionBusy}
          >
            {winBoatActionBusy ? (
              <LoaderCircle size={17} className="spin" />
            ) : winBoatControlKind === "settings" || winBoatControlKind === "setup" ? (
              <Settings size={17} />
            ) : winBoatControlKind === "open" ? (
              <Monitor size={17} />
            ) : (
              <Play size={17} />
            )}
            {winBoatActionLabel}
          </button>
        </aside>
      )}

      <section className="section-card installed-section dock-panel" aria-labelledby="installed-heading">
        <SectionHeader
          id="installed-heading"
          title={t("installed-title")}
          count={installedCountLabel}
          meta={t("installed-meta")}
          action={
            <button type="button" className="icon-button" title={t("refresh-installed")} onClick={onRefreshInstalled}>
              <RefreshCw size={16} />
            </button>
          }
        />
        <div className="installed-grid">
          {installed.map((version) => (
            <article className="installed-vessel" key={version.version}>
              <span className="vessel-icon"><AppWindow size={20} /></span>
              <div className="vessel-copy">
                <span className="micro-label">{t("action-installed")}</span>
                <strong>{version.version}</strong>
                <span>{version.displayName}</span>
              </div>
              <div className="vessel-actions">
                <button
                  type="button"
                  className="button light"
                  onClick={() => onLaunch(version)}
                  disabled={!online || isLaunching}
                >
                  {isBusy(`launch-${version.version}`) ? <LoaderCircle size={17} className="spin" /> : <Play size={17} />}
                  {isBusy(`launch-${version.version}`) ? t("action-launching") : t("action-launch")}
                </button>
                <button
                  type="button"
                  className="icon-button danger inverse"
                  title={t("remove-version-title", { version: version.version })}
                  onClick={() => onUninstall(version)}
                  disabled={!online || isBusy(`uninstall-${version.version}`)}
                >
                  {isBusy(`uninstall-${version.version}`) ? (
                    <LoaderCircle size={16} className="spin" />
                  ) : (
                    <Trash2 size={16} />
                  )}
                </button>
              </div>
            </article>
          ))}
          {installed.length === 0 && (
            <EmptyState
              icon={AppWindow}
              title={t("empty-installed-title")}
              detail={online ? t("empty-installed-online") : t("empty-installed-offline")}
            />
          )}
        </div>
      </section>

      <section className="section-card available-section manifest-panel" aria-labelledby="available-heading">
        <SectionHeader
          id="available-heading"
          title={t("available-title")}
          meta={
            availableTotal
              ? t("catalog-loaded-total", {
                  loaded: loadedCountLabel,
                  total: availableTotalLabel,
                })
              : loadedCount
                ? t("catalog-loaded", { loaded: loadedCountLabel })
                : t("official-marketplace")
          }
          action={
            <div className="catalog-tools">
              <label className="search-field">
                <Search size={16} />
                <span className="sr-only">{t("search-version-placeholder")}</span>
                <input
                  value={search}
                  onChange={(event) => onSearch(event.target.value)}
                  placeholder={t("search-version-placeholder")}
                  spellCheck={false}
                />
              </label>
              <div className="version-filters" role="group" aria-label={t("support-filter-aria")}>
                {(["lts", "mts"] as const).map((filter) => (
                  <button
                    type="button"
                    className={supportFilters[filter] ? "active" : ""}
                    key={filter}
                    aria-pressed={supportFilters[filter]}
                    onClick={() => onToggleSupportFilter(filter)}
                  >
                    {filter.toUpperCase()}
                  </button>
                ))}
              </div>
              <button type="button" className="icon-button" title={t("refresh-catalog")} onClick={onRefreshCatalog} disabled={catalogLoading}>
                <RefreshCw size={16} className={catalogLoading ? "spin" : ""} />
              </button>
            </div>
          }
        />

        {downloadProgress && (
          <InstallationProgress
            t={t}
            localization={localization}
            key={downloadProgress.version}
            progress={downloadProgress}
            isInstalling={isInstalling}
            onCancel={onCancelDownload}
          />
        )}

        {catalogError && (
          <div className="inline-error">
            <XCircle size={16} />
            <span>{catalogError}</span>
            <button type="button" onClick={onRefreshCatalog}>{t("action-retry")}</button>
          </div>
        )}

        {available.length > 0 && (
          <div className="manifest-table-wrap">
            <table className="manifest-table">
              <caption className="sr-only">{t("available-title")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t("manifest-version")}</th>
                  <th scope="col">{t("manifest-support")}</th>
                  <th scope="col" className="release-cell">{t("manifest-release")}</th>
                  <th scope="col">{t("manifest-status")}</th>
                  <th scope="col"><span className="sr-only">{t("manifest-actions")}</span></th>
                </tr>
              </thead>
              <tbody>
                {available.map((version) => {
                  const alreadyInstalled = installedSet.has(version.version);
                  const installingThis = isBusy(`install-${version.version}`);
                  return (
                    <tr key={version.version}>
                      <td className="version-cell">
                        <span className="version-beacon" aria-hidden="true" />
                        <div><strong>{version.version}</strong><span>Studio Pro</span></div>
                      </td>
                      <td className="support-cell"><VersionBadges t={t} version={version} /></td>
                      <td className="release-cell">{version.formattedReleaseDate ?? version.releaseDate ?? "—"}</td>
                      <td>
                        <span className={`availability-state ${alreadyInstalled ? "installed" : online ? "available" : "offline"}`}>
                          <i />
                          {alreadyInstalled
                            ? t("action-installed")
                            : online
                              ? t("status-available")
                              : t("connection-offline")}
                        </span>
                      </td>
                      <td className="manifest-action">
                        <button
                          type="button"
                          className={`button compact ${alreadyInstalled ? "quiet" : "primary"}`}
                          disabled={!online || alreadyInstalled || isInstalling}
                          onClick={() => onInstall(version)}
                        >
                          {installingThis ? <LoaderCircle size={16} className="spin" /> : alreadyInstalled ? <CheckCircle2 size={16} /> : <Download size={16} />}
                          {installingThis ? t("action-installing") : alreadyInstalled ? t("action-installed") : t("action-install")}
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}

        <div className="manifest-state">
          {catalogLoading && available.length === 0 && (
            <div className="loading-inline"><LoaderCircle size={18} className="spin" /> {t("catalog-loading")}</div>
          )}
          {!catalogLoading && available.length === 0 && !catalogError && !hasMore && (
            <EmptyState
              icon={Download}
              title={search || supportFilters.lts || supportFilters.mts ? t("search-no-results") : t("catalog-empty")}
              detail={
                search || supportFilters.lts || supportFilters.mts
                  ? t("filter-no-results-detail")
                  : t("catalog-empty-detail")
              }
            />
          )}
        </div>

        {hasMore && (
          <div
            ref={loadMoreSentinel}
            className={`infinite-scroll-sentinel ${catalogLoading ? "loading" : ""}`}
            aria-live="polite"
          >
            {catalogLoading && (
              <><LoaderCircle size={15} className="spin" /> {t("catalog-loading-older")}</>
            )}
          </div>
        )}
      </section>
    </div>
  );
}

const TERMINAL_PROGRESS_STATES = new Set(["installed", "failed", "cancelled"]);
const INSTALLATION_STAGE_KEYS = [
  "progress-stage-prepare",
  "progress-stage-download",
  "progress-stage-staging",
  "progress-stage-install",
  "progress-stage-verify",
] as const;

function installationStageIndex(state: string, percentage: number) {
  if (["starting", "preparing", "checking"].includes(state)) return 0;
  if (["connecting", "downloading", "downloaded", "ready"].includes(state)) return 1;
  if (state === "staging") return 2;
  if (["installing", "finalizing"].includes(state)) return 3;
  if (["verifying", "installed"].includes(state)) return 4;
  if (percentage < 10) return 0;
  if (percentage < 60) return 1;
  if (percentage < 68) return 2;
  if (percentage < 97) return 3;
  return 4;
}

function InstallationProgress({
  t,
  localization,
  progress,
  isInstalling,
  onCancel,
}: {
  t: Translate;
  localization: LocalizationBundle;
  progress: DownloadProgress;
  isInstalling: boolean;
  onCancel: () => void;
}) {
  const [displayedPercentage, setDisplayedPercentage] = useState(() =>
    clampPercentage(progress.percentage ?? 2),
  );
  const previousState = useRef(progress.state);
  const reportedPercentage = clampPercentage(progress.percentage ?? 0);
  const isActive = !TERMINAL_PROGRESS_STATES.has(progress.state);

  useEffect(() => {
    const restarted = progress.state === "starting" && previousState.current !== "starting";
    setDisplayedPercentage((current) => {
      if (restarted) return Math.max(2, reportedPercentage);
      return progress.state === "installed" ? 100 : Math.max(current, reportedPercentage);
    });
    previousState.current = progress.state;
  }, [progress.state, reportedPercentage]);

  const boundedPercentage = progress.state === "installed"
    ? 100
    : Math.min(99, displayedPercentage);
  const roundedPercentage = Math.round(boundedPercentage);
  const currentStage = installationStageIndex(progress.state, boundedPercentage);
  const progressLabel =
    progress.state === "installed"
      ? t("progress-complete", { percentage: localizedNumber(localization, 100) })
      : progress.state === "failed"
        ? t("progress-failed-short")
        : progress.state === "cancelled"
          ? t("progress-cancelled-short")
          : t(progress.estimated ? "progress-approximate" : "progress-percent", {
              percentage: localizedNumber(localization, roundedPercentage),
            });

  return (
    <div
      className={`download-bar ${progress.state}`}
      aria-live="polite"
      aria-busy={isActive}
    >
      <div className="download-copy">
        <strong>Studio Pro {progress.version}</strong>
        <span>{progressDescription(progress, t)}</span>
      </div>
      <div className="progress-visual">
        <div
          className="progress-track"
          role="progressbar"
          aria-label={t("progress-aria", { version: progress.version })}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={roundedPercentage}
          aria-valuetext={`${progressDescription(progress, t)} ${progressLabel}`}
        >
          <span
            className={isActive ? "active" : ""}
            style={{ width: `${Math.max(2, boundedPercentage)}%` }}
          />
          <div className="progress-boundaries" aria-hidden="true">
            {[10, 58, 68, 97].map((boundary) => (
              <i key={boundary} style={{ insetInlineStart: `${boundary}%` }} />
            ))}
          </div>
        </div>
        <div className="progress-stages" aria-hidden="true">
          {INSTALLATION_STAGE_KEYS.map((labelKey, index) => (
            <span
              className={
                progress.state === "installed" || index < currentStage
                  ? "complete"
                  : index === currentStage
                    ? "current"
                    : ""
              }
              key={labelKey}
            >
              <i />
              {t(labelKey)}
            </span>
          ))}
        </div>
      </div>
      <b className="progress-percentage">{progressLabel}</b>
      {isInstalling && ["connecting", "downloading"].includes(progress.state) && (
        <button type="button" onClick={onCancel}>{t("action-cancel")}</button>
      )}
    </div>
  );
}

function ProjectsView({
  t,
  localization,
  projects,
  totalProjects,
  search,
  sharedDirectory,
  installedSet,
  isLaunching,
  isBusy,
  onSearch,
  onRefresh,
  onOpenWorkspace,
  onOpenFolder,
  onLaunch,
  onFindVersion,
}: {
  t: Translate;
  localization: LocalizationBundle;
  projects: MendixProject[];
  totalProjects: number;
  search: string;
  sharedDirectory: string;
  installedSet: Set<string>;
  isLaunching: boolean;
  isBusy: (key: string) => boolean;
  onSearch: (value: string) => void;
  onRefresh: () => void;
  onOpenWorkspace: () => void;
  onOpenFolder: (path: string) => void;
  onLaunch: (project: MendixProject) => void;
  onFindVersion: (version: string) => void;
}) {
  const [totalProjectsLabel] = useLocalizedNumbers([totalProjects], localization);

  return (
    <div className="projects-page">
      <PageTitle
        eyebrow={t("projects-eyebrow")}
        title={t("projects-title")}
        description={t("projects-description")}
      />

      <aside className="workspace-berth">
        <span className="berth-icon"><HardDrive size={21} /></span>
        <div>
          <span className="micro-label">{t("workspace-current")}</span>
          <strong title={sharedDirectory}>{sharedDirectory}</strong>
        </div>
        <button type="button" className="button secondary" onClick={onOpenWorkspace}>
          <FolderOpen size={17} />{t("action-open-folder")}
        </button>
      </aside>

      <section className="section-card project-manifest" aria-labelledby="projects-heading">
        <SectionHeader
          id="projects-heading"
          title={t("projects-found")}
          count={totalProjectsLabel}
          meta={t("projects-manifest-meta")}
          action={
            <div className="catalog-tools">
              <label className="search-field project-search">
                <Search size={16} />
                <span className="sr-only">{t("search-project-placeholder")}</span>
                <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder={t("search-project-placeholder")} />
              </label>
              <button type="button" className="icon-button" title={t("refresh-projects")} onClick={onRefresh}>
                <RefreshCw size={16} />
              </button>
            </div>
          }
        />

        {projects.length > 0 ? (
          <div className="manifest-table-wrap">
            <table className="manifest-table projects-table">
              <caption className="sr-only">{t("projects-found")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t("project-column-name")}</th>
                  <th scope="col">{t("project-column-version")}</th>
                  <th scope="col" className="modified-cell">{t("project-column-modified")}</th>
                  <th scope="col"><span className="sr-only">{t("manifest-actions")}</span></th>
                </tr>
              </thead>
              <tbody>
                {projects.map((project) => {
                  const exactVersionInstalled = Boolean(project.version && installedSet.has(project.version));
                  const canOpen = !project.version || exactVersionInstalled || installedSet.size > 0;
                  return (
                    <tr key={project.mprPath}>
                      <td className="project-name-cell">
                        <span className="project-flag" aria-hidden="true"><FolderKanban size={18} /></span>
                        <div>
                          <strong>{project.name}</strong>
                          <span title={project.directory}>{compactPath(project.directory)}</span>
                        </div>
                      </td>
                      <td>
                        <span className={`version-state ${project.version && !exactVersionInstalled ? "missing" : "ready"}`}>
                          {project.version ?? t("version-unknown")}
                          {project.version && <small>{exactVersionInstalled ? t("version-ready") : t("version-missing")}</small>}
                        </span>
                      </td>
                      <td className="modified-cell">{project.formattedLastModified ?? "—"}</td>
                      <td className="project-actions">
                        {project.version && !exactVersionInstalled && (
                          <button type="button" className="button quiet compact" onClick={() => onFindVersion(project.version!)}>
                            {t("action-install-version", { version: project.version })}
                          </button>
                        )}
                        <button
                          type="button"
                          className="button primary compact"
                          onClick={() => onLaunch(project)}
                          disabled={!canOpen || isLaunching}
                        >
                          {isBusy(`launch-${project.version ?? "unknown"}`) ? (
                            <LoaderCircle size={16} className="spin" />
                          ) : (
                            <Play size={16} />
                          )}
                          {isBusy(`launch-${project.version ?? "unknown"}`) ? t("action-opening") : t("action-open")}
                        </button>
                        <button type="button" className="icon-button" title={t("open-linux-folder")} onClick={() => onOpenFolder(project.directory)}>
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
              title={totalProjects ? t("projects-search-empty") : t("projects-empty")}
              detail={totalProjects ? t("projects-search-detail") : t("projects-empty-detail")}
            />
          </div>
        )}
      </section>
    </div>
  );
}

function SettingsView({
  t,
  config,
  changed,
  mountMatches,
  applyNow,
  isBusy,
  onChange,
  onChoose,
  onApplyNow,
  onSave,
  onRedetect,
}: {
  t: Translate;
  config: AppConfig;
  changed: boolean;
  mountMatches: boolean;
  applyNow: boolean;
  isBusy: (key: string) => boolean;
  onChange: (config: AppConfig) => void;
  onChoose: (field: PathField) => void;
  onApplyNow: (value: boolean) => void;
  onSave: () => void;
  onRedetect: () => void;
}) {
  return (
    <div className="settings-page">
      <PageTitle
        eyebrow={t("settings-eyebrow")}
        title={t("settings-title")}
        description={t("settings-description")}
      />

      <div className="settings-route" aria-label={t("settings-route-aria")}>
        <span className="active"><b>01</b>{t("settings-step-winboat")}</span>
        <i aria-hidden="true" />
        <span className={mountMatches ? "complete" : "active"}><b>02</b>{t("settings-step-workspace")}</span>
      </div>

      <section className="section-card settings-card" aria-labelledby="winboat-settings-heading">
        <div className="settings-heading">
          <span className="settings-number" aria-hidden="true">01</span>
          <div>
            <span className="micro-label">{t("settings-step-environment")}</span>
            <h2 id="winboat-settings-heading">WinBoat</h2>
            <p>{t("settings-winboat-description")}</p>
          </div>
          <button type="button" className="button secondary compact" onClick={onRedetect} disabled={isBusy("redetect")}>
            <RefreshCw size={15} className={isBusy("redetect") ? "spin" : ""} /> {t("action-auto-detect")}
          </button>
        </div>
        <PathInput
          label={t("settings-winboat-executable")}
          browseLabel={t("action-browse")}
          value={config.winboatExecutable}
          onChange={(value) => onChange({ ...config, winboatExecutable: value })}
          onBrowse={() => onChoose("winboatExecutable")}
        />
        <PathInput
          label={t("settings-compose-file")}
          browseLabel={t("action-browse")}
          value={config.composeFile}
          onChange={(value) => onChange({ ...config, composeFile: value })}
          onBrowse={() => onChoose("composeFile")}
        />
        <label className="simple-field runtime-field">
          <span>{t("settings-container-runtime")}</span>
          <select value={config.containerRuntime} onChange={(event) => onChange({ ...config, containerRuntime: event.target.value })}>
            <option value="docker">Docker</option>
            <option value="podman">Podman</option>
          </select>
        </label>
      </section>

      <section className="section-card settings-card" aria-labelledby="workspace-settings-heading">
        <div className="settings-heading">
          <span className="settings-number amber" aria-hidden="true">02</span>
          <div>
            <span className="micro-label">{t("settings-step-cargo")}</span>
            <h2 id="workspace-settings-heading">{t("settings-workspace-title")}</h2>
            <p>{t("settings-workspace-description")}</p>
          </div>
          <span className={`mount-state ${mountMatches ? "ok" : "pending"}`}>
            {mountMatches ? <CheckCircle2 size={14} /> : <Info size={14} />}
            {mountMatches ? t("mount-connected") : t("mount-pending")}
          </span>
        </div>
        <PathInput
          label={t("settings-shared-directory")}
          browseLabel={t("action-browse")}
          value={config.sharedDirectory}
          onChange={(value) => onChange({ ...config, sharedDirectory: value })}
          onBrowse={() => onChoose("sharedDirectory")}
        />
        <label className="apply-row">
          <input type="checkbox" checked={applyNow} onChange={(event) => onApplyNow(event.target.checked)} />
          <span>
            <strong>{t("settings-apply-now-title")}</strong>
            <small>{t("settings-apply-now-detail")}</small>
          </span>
        </label>
      </section>

      <div className={`settings-actions ${changed ? "changed" : ""}`} aria-live="polite">
        <span>{changed ? <Info size={17} /> : <CheckCircle2 size={17} />}{changed ? t("settings-unsaved") : t("settings-saved")}</span>
        <button type="button" className="button primary" onClick={onSave} disabled={!changed || isBusy("save-settings")}>
          {isBusy("save-settings") ? <LoaderCircle size={16} className="spin" /> : <CheckCircle2 size={16} />}
          {t("action-save-settings")}
        </button>
      </div>
    </div>
  );
}

function HarborMark({ large = false }: { large?: boolean }) {
  return (
    <span className={`harbor-mark ${large ? "large" : ""}`} aria-hidden="true">
      <img src="/mendimaru.png" alt="" draggable={false} />
    </span>
  );
}

function PageTitle({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return (
    <header className="page-title">
      <span>{eyebrow}</span>
      <h1>{title}</h1>
      <p>{description}</p>
    </header>
  );
}

function SectionHeader({
  id,
  title,
  count,
  meta,
  action,
}: {
  id?: string;
  title: string;
  count?: React.ReactNode;
  meta?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="section-header">
      <div><h2 id={id}>{title}</h2>{count != null && <b>{count}</b>}{meta && <span>{meta}</span>}</div>
      {action}
    </div>
  );
}

function VersionBadges({ t, version }: { t: Translate; version: DownloadableVersion }) {
  return (
    <span className="badges">
      {version.isLatest && <em className="latest">{t("badge-latest")}</em>}
      {version.isLts && <em className="lts">LTS</em>}
      {version.isMts && <em className="mts">MTS</em>}
      {version.isBeta && <em className="beta">{t("badge-beta")}</em>}
    </span>
  );
}

function PathInput({
  label,
  browseLabel,
  value,
  onChange,
  onBrowse,
}: {
  label: string;
  browseLabel: string;
  value: string;
  onChange: (value: string) => void;
  onBrowse: () => void;
}) {
  return (
    <label className="simple-field path-field">
      <span>{label}</span>
      <div>
        <input value={value} onChange={(event) => onChange(event.target.value)} spellCheck={false} />
        <button type="button" onClick={onBrowse}>{browseLabel}</button>
      </div>
    </label>
  );
}

function EmptyState({
  icon: Icon,
  title,
  detail,
}: {
  icon: LucideIcon;
  title: string;
  detail: string;
}) {
  return (
    <div className="empty-state">
      <Icon size={23} />
      <div><strong>{title}</strong><span>{detail}</span></div>
    </div>
  );
}

function LoadingScreen() {
  return (
    <div className="loading-screen">
      <HarborMark large />
      <div><strong>mendimaru</strong><span>Studio Pro port</span></div>
      <LoaderCircle size={20} className="spin" />
    </div>
  );
}

function ToastStack({
  t,
  toasts,
  onDismiss,
}: {
  t: Translate;
  toasts: ToastMessage[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="toast-stack" aria-live="polite">
      {toasts.map((toast) => (
        <div className={`toast ${toast.kind}`} key={toast.id}>
          {toast.kind === "success" ? <CheckCircle2 size={18} /> : toast.kind === "error" ? <XCircle size={18} /> : <Info size={18} />}
          <div><strong>{toast.title}</strong>{toast.detail && <span>{toast.detail}</span>}</div>
          <button type="button" onClick={() => onDismiss(toast.id)} aria-label={t("dismiss-notification")}><X size={14} /></button>
        </div>
      ))}
    </div>
  );
}

function ConfirmDialog({
  t,
  state,
  onCancel,
  onConfirm,
}: {
  t: Translate;
  state: ConfirmationState;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    cancelRef.current?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = Array.from(
        dialogRef.current?.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      );
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      previouslyFocused?.focus();
    };
  }, [onCancel]);

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onCancel()}>
      <div
        ref={dialogRef}
        className="confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
      >
        <span className="dialog-symbol" aria-hidden="true">{state.danger ? <Trash2 size={21} /> : <Anchor size={21} />}</span>
        <h2 id={titleId}>{state.title}</h2>
        <p id={descriptionId}>{state.description}</p>
        <div>
          <button ref={cancelRef} type="button" className="button secondary" onClick={onCancel}>{t("action-cancel")}</button>
          <button type="button" className={`button ${state.danger ? "danger" : "primary"}`} onClick={onConfirm}>{state.confirmLabel}</button>
        </div>
      </div>
    </div>
  );
}

const LOCALIZED_PROGRESS_STATES = new Set([
  "starting",
  "preparing",
  "checking",
  "connecting",
  "downloading",
  "downloaded",
  "ready",
  "staging",
  "installing",
  "finalizing",
  "verifying",
  "installed",
  "cancelled",
]);

function progressDescription(progress: DownloadProgress, t: Translate) {
  const message = LOCALIZED_PROGRESS_STATES.has(progress.state)
    ? t(`progress-${progress.state}`)
    : progress.message;
  if (progress.state !== "downloading" || !progress.totalBytesLabel) return message;
  return `${message} ${progress.downloadedBytesLabel} / ${progress.totalBytesLabel}`;
}

function clampPercentage(value: number) {
  return Math.min(100, Math.max(0, value));
}

function compactPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : path;
}

function localizedNumber(localization: LocalizationBundle, value: number) {
  return Number.isInteger(value) && value >= 0 && value < localization.numbers.length
    ? localization.numbers[value]
    : String(value);
}

function useLocalizedNumbers(values: number[], localization: LocalizationBundle) {
  const requestKey = `${localization.locale}:${values.join(",")}`;
  const [result, setResult] = useState<{ key: string; values: string[] } | null>(null);

  useEffect(() => {
    let active = true;
    void formatNumbers(values)
      .then((formatted) => {
        if (active) setResult({ key: requestKey, values: formatted });
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [requestKey]);

  return result?.key === requestKey
    ? result.values
    : values.map((value) => localizedNumber(localization, value));
}

async function localizeCatalog(catalog: StudioVersionCatalog): Promise<StudioVersionCatalog> {
  const dates = await formatDates(catalog.versions.map((version) => version.releaseDate ?? ""));
  return {
    ...catalog,
    versions: catalog.versions.map((version, index) => ({
      ...version,
      formattedReleaseDate: dates[index] ?? version.releaseDate ?? "",
    })),
  };
}

async function localizeProjects(projects: MendixProject[]): Promise<MendixProject[]> {
  const dates = await formatDates(projects.map((project) => project.lastModified ?? ""));
  return projects.map((project, index) => ({
    ...project,
    formattedLastModified: dates[index] ?? "",
  }));
}

function errorText(error: unknown, t: Translate) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (isCommandError(error)) return error.message;
  try {
    return JSON.stringify(error) ?? t("unknown-error");
  } catch {
    return t("unknown-error");
  }
}

function errorCode(error: unknown) {
  return isCommandError(error) ? error.code : undefined;
}

function isCommandError(error: unknown): error is CommandError {
  return Boolean(
    error &&
      typeof error === "object" &&
      "code" in error &&
      typeof error.code === "string" &&
      "message" in error &&
      typeof error.message === "string",
  );
}

export default App;
