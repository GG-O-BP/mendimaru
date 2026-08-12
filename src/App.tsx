import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AppWindow,
  CheckCircle2,
  Download,
  FolderKanban,
  FolderOpen,
  Languages,
  Info,
  LoaderCircle,
  Monitor,
  Play,
  RefreshCw,
  Search,
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
      const errors = [statusResult, installedResult, projectResult]
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
            message: t("progress-starting"),
          });
          try {
            await invoke("install_studio_pro", { version: version.version });
            await refreshInstalled();
            notify("success", t("toast-install-complete", { version: version.version }));
            window.setTimeout(() => {
              setDownloadProgress((current) =>
                current?.version === version.version && current.state === "installed"
                  ? null
                  : current,
              );
            }, 5000);
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
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">m</span>
          <strong>mendimaru</strong>
        </div>

        <nav className="tabs" aria-label={t("nav-main-aria")}>
          {TABS.map(({ key, labelKey, icon: Icon }) => (
            <button
              type="button"
              key={key}
              className={activeView === key ? "active" : ""}
              onClick={() => setActiveView(key)}
            >
              <Icon size={16} />
              {t(labelKey)}
            </button>
          ))}
        </nav>

        <div className="winboat-control">
          <label className="language-control" title={t("language-label")}>
            <Languages size={15} />
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
          <span className={`connection ${online ? "online" : "offline"}`}>
            <i />
            {online ? t("connection-online") : t("connection-offline")}
          </span>
          <button
            type="button"
            className={`button compact ${online ? "secondary" : "primary"}`}
            onClick={online ? openWinBoat : startWindows}
            disabled={isBusy(online ? "open-winboat" : "start-windows")}
          >
            {isBusy(online ? "open-winboat" : "start-windows") ? (
              <LoaderCircle size={15} className="spin" />
            ) : online ? (
              <Monitor size={15} />
            ) : (
              <Play size={15} />
            )}
            {online ? t("action-open-windows") : t("action-start-windows")}
          </button>
        </div>
      </header>

      {warning && (
        <div className="global-warning">
          <Info size={16} />
          <span>{warning}</span>
          <button type="button" onClick={() => setWarning(null)} aria-label={t("dismiss-notification")}><X size={15} /></button>
        </div>
      )}

      <main className="page">
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
            onSearch={setVersionSearch}
            onToggleSupportFilter={(filter) =>
              setVersionSupportFilters((current) => ({
                ...current,
                [filter]: !current[filter],
              }))
            }
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
  onSearch,
  onToggleSupportFilter,
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
  onSearch: (value: string) => void;
  onToggleSupportFilter: (value: VersionSupportFilter) => void;
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
        title={t("nav-studio")}
        description={t("studio-description")}
      />

      <section className="section-card installed-section">
        <SectionHeader
          title={t("installed-title")}
          count={installedCountLabel}
          action={
            <button type="button" className="icon-button" title={t("refresh-installed")} onClick={onRefreshInstalled}>
              <RefreshCw size={16} />
            </button>
          }
        />
        <div className="rows compact-rows">
          {installed.map((version) => (
            <div className="data-row" key={version.version}>
              <span className="item-icon"><AppWindow size={17} /></span>
              <div className="row-main">
                <strong>Studio Pro {version.version}</strong>
                <span>{version.displayName}</span>
              </div>
              <div className="row-actions">
                <button
                  type="button"
                  className="button secondary compact"
                  onClick={() => onLaunch(version)}
                  disabled={!online || isLaunching}
                >
                  {isBusy(`launch-${version.version}`) ? <LoaderCircle size={15} className="spin" /> : <Play size={15} />}
                  {isBusy(`launch-${version.version}`) ? t("action-launching") : t("action-launch")}
                </button>
                <button
                  type="button"
                  className="icon-button danger"
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
            </div>
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

      <section className="section-card available-section">
        <SectionHeader
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
                <input
                  value={search}
                  onChange={(event) => onSearch(event.target.value)}
                  placeholder={t("search-version-placeholder")}
                  spellCheck={false}
                />
              </label>
              <div className="version-filters" role="group" aria-label={t("support-filter-aria")}>
                {(["lts", "mts"] as const).map((filter) => (
                  <label key={filter}>
                    <input
                      type="checkbox"
                      checked={supportFilters[filter]}
                      onChange={() => onToggleSupportFilter(filter)}
                    />
                    <span>{filter.toUpperCase()}</span>
                  </label>
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

        <div className="rows version-rows">
          {available.map((version) => {
            const alreadyInstalled = installedSet.has(version.version);
            const installingThis = isBusy(`install-${version.version}`);
            return (
              <div className="data-row" key={version.version}>
                <span className="item-icon download"><Download size={17} /></span>
                <div className="row-main version-copy">
                  <div>
                    <strong>{version.version}</strong>
                    <VersionBadges t={t} version={version} />
                  </div>
                  <span>{version.formattedReleaseDate ?? version.releaseDate ?? ""}</span>
                </div>
                <button
                  type="button"
                  className={`button compact ${alreadyInstalled ? "quiet" : "primary"}`}
                  disabled={!online || alreadyInstalled || isInstalling}
                  onClick={() => onInstall(version)}
                >
                  {installingThis ? <LoaderCircle size={15} className="spin" /> : alreadyInstalled ? <CheckCircle2 size={15} /> : <Download size={15} />}
                  {installingThis ? t("action-installing") : alreadyInstalled ? t("action-installed") : t("action-install")}
                </button>
              </div>
            );
          })}

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

const PROGRESS_ANIMATION_TARGETS: Record<string, number> = {
  starting: 4,
  preparing: 7,
  checking: 11,
  connecting: 14,
  downloading: 70,
  downloaded: 74,
  ready: 74,
};

const TERMINAL_PROGRESS_STATES = new Set(["installed", "failed", "cancelled"]);

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
  const [installElapsedSeconds, setInstallElapsedSeconds] = useState(0);
  const previousState = useRef(progress.state);
  const reportedPercentage = clampPercentage(progress.percentage ?? 0);
  const animationTarget = PROGRESS_ANIMATION_TARGETS[progress.state];
  const isActive = !TERMINAL_PROGRESS_STATES.has(progress.state);

  useEffect(() => {
    const restarted = progress.state === "starting" && previousState.current !== "starting";
    setDisplayedPercentage((current) => {
      if (restarted) return Math.max(2, reportedPercentage);
      return progress.state === "installed" ? 100 : Math.max(current, reportedPercentage);
    });
    previousState.current = progress.state;
  }, [progress.state, reportedPercentage]);

  useEffect(() => {
    if (animationTarget == null || !isActive) return undefined;

    const interval = window.setInterval(() => {
      setDisplayedPercentage((current) => {
        if (current >= animationTarget) return current;
        const remaining = animationTarget - current;
        const step =
          progress.state === "downloading"
            ? Math.max(0.08, remaining * 0.006)
            : Math.max(0.25, remaining * 0.08);
        return Math.min(animationTarget, current + step);
      });
    }, 800);

    return () => window.clearInterval(interval);
  }, [animationTarget, isActive, progress.state]);

  useEffect(() => {
    if (progress.state !== "installing") {
      setInstallElapsedSeconds(0);
      return undefined;
    }

    const startedAt = Date.now();
    const updateElapsed = () => {
      setInstallElapsedSeconds(Math.floor((Date.now() - startedAt) / 1000));
    };
    updateElapsed();
    const interval = window.setInterval(updateElapsed, 1000);
    return () => window.clearInterval(interval);
  }, [progress.state]);

  const roundedPercentage = Math.round(displayedPercentage);
  const progressLabel =
    progress.state === "installed"
      ? t("progress-complete", { percentage: localizedNumber(localization, 100) })
      : progress.state === "installing"
        ? installElapsedSeconds > 0
          ? t("progress-elapsed", {
              duration: formatElapsedTime(installElapsedSeconds, localization, t),
            })
          : t("progress-installing-short")
      : progress.state === "failed"
        ? t("progress-failed-short")
        : progress.state === "cancelled"
          ? t("progress-cancelled-short")
          : t("progress-approximate", {
              percentage: localizedNumber(localization, roundedPercentage),
            });

  return (
    <div className={`download-bar ${progress.state}`} aria-live="polite">
      <div className="download-copy">
        <strong>Studio Pro {progress.version}</strong>
        <span>{progressDescription(progress, t)}</span>
      </div>
      <div
        className={`progress-track ${progress.state === "installing" ? "indeterminate" : ""}`}
        role="progressbar"
        aria-label={t("progress-aria", { version: progress.version })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={progress.state === "installing" ? undefined : roundedPercentage}
        aria-valuetext={`${progressDescription(progress, t)} ${progressLabel}`}
      >
        <span
          className={isActive ? "active" : ""}
          style={{ width: `${Math.max(2, displayedPercentage)}%` }}
        />
      </div>
      <b>{progressLabel}</b>
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
    <div>
      <PageTitle
        title={t("projects-title")}
        description={t("projects-description")}
      />

      <div className="workspace-line">
        <FolderOpen size={17} />
        <span title={sharedDirectory}>{sharedDirectory}</span>
        <button type="button" onClick={onOpenWorkspace}>{t("action-open-folder")}</button>
      </div>

      <section className="section-card">
        <SectionHeader
          title={t("projects-found")}
          count={totalProjectsLabel}
          action={
            <div className="catalog-tools">
              <label className="search-field project-search">
                <Search size={16} />
                <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder={t("search-project-placeholder")} />
              </label>
              <button type="button" className="icon-button" title={t("refresh-projects")} onClick={onRefresh}>
                <RefreshCw size={16} />
              </button>
            </div>
          }
        />

        <div className="rows project-rows">
          {projects.map((project) => {
            const exactVersionInstalled = Boolean(project.version && installedSet.has(project.version));
            const canOpen = !project.version || exactVersionInstalled || installedSet.size > 0;
            return (
              <div className="data-row project-row" key={project.mprPath}>
                <span className="item-icon project"><FolderKanban size={17} /></span>
                <div className="row-main">
                  <strong>{project.name}</strong>
                  <span title={project.directory}>{compactPath(project.directory)}</span>
                </div>
                <div className={`project-version ${project.version && !exactVersionInstalled ? "missing" : ""}`}>
                  <span>Studio Pro</span>
                  <strong>{project.version ?? t("version-unknown")}</strong>
                </div>
                <span className="modified">{project.formattedLastModified ?? ""}</span>
                <div className="row-actions">
                  {project.version && !exactVersionInstalled && (
                    <button type="button" className="button quiet compact" onClick={() => onFindVersion(project.version!)}>
                      {t("action-find-version")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="button primary compact"
                    onClick={() => onLaunch(project)}
                    disabled={!canOpen || isLaunching}
                  >
                    {isBusy(`launch-${project.version ?? "unknown"}`) ? (
                      <LoaderCircle size={15} className="spin" />
                    ) : (
                      <Play size={15} />
                    )}
                    {isBusy(`launch-${project.version ?? "unknown"}`) ? t("action-opening") : t("action-open")}
                  </button>
                  <button type="button" className="icon-button" title={t("open-linux-folder")} onClick={() => onOpenFolder(project.directory)}>
                    <FolderOpen size={16} />
                  </button>
                </div>
              </div>
            );
          })}
          {projects.length === 0 && (
            <EmptyState
              icon={FolderKanban}
              title={totalProjects ? t("projects-search-empty") : t("projects-empty")}
              detail={totalProjects ? t("projects-search-detail") : t("projects-empty-detail")}
            />
          )}
        </div>
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
        title={t("settings-title")}
        description={t("settings-description")}
      />

      <section className="section-card settings-card">
        <div className="settings-heading">
          <div>
            <h2>WinBoat</h2>
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

      <section className="section-card settings-card">
        <div className="settings-heading">
          <div>
            <h2>{t("settings-workspace-title")}</h2>
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

      <div className="settings-actions">
        <span>{changed ? t("settings-unsaved") : t("settings-saved")}</span>
        <button type="button" className="button primary" onClick={onSave} disabled={!changed || isBusy("save-settings")}>
          {isBusy("save-settings") ? <LoaderCircle size={16} className="spin" /> : <CheckCircle2 size={16} />}
          {t("action-save-settings")}
        </button>
      </div>
    </div>
  );
}

function PageTitle({ title, description }: { title: string; description: string }) {
  return <div className="page-title"><h1>{title}</h1><p>{description}</p></div>;
}

function SectionHeader({
  title,
  count,
  meta,
  action,
}: {
  title: string;
  count?: React.ReactNode;
  meta?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="section-header">
      <div><h2>{title}</h2>{count != null && <b>{count}</b>}{meta && <span>{meta}</span>}</div>
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
      <span className="brand-mark large">m</span>
      <strong>mendimaru</strong>
      <LoaderCircle size={18} className="spin" />
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
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onCancel()}>
      <div className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-title">
        <h2 id="confirm-title">{state.title}</h2>
        <p>{state.description}</p>
        <div>
          <button type="button" className="button secondary" onClick={onCancel}>{t("action-cancel")}</button>
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
  "installing",
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

function formatElapsedTime(
  totalSeconds: number,
  localization: LocalizationBundle,
  t: Translate,
) {
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) {
    return t("duration-hours-minutes-seconds", {
      hours: localizedNumber(localization, hours),
      minutes: localizedNumber(localization, minutes),
      seconds: localizedNumber(localization, seconds),
    });
  }
  if (minutes > 0) {
    return t("duration-minutes-seconds", {
      minutes: localizedNumber(localization, minutes),
      seconds: localizedNumber(localization, seconds),
    });
  }
  return t("duration-seconds", { seconds: localizedNumber(localization, seconds) });
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
