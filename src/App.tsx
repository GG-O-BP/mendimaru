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
  ConfirmationState,
  DownloadableVersion,
  DownloadProgress,
  EnvironmentStatus,
  MendixProject,
  SettingsSaveResult,
  StudioVersion,
  StudioVersionCatalog,
  ToastKind,
  ToastMessage,
  ViewKey,
} from "./types";
import "./App.css";

const EMPTY_CATALOG: StudioVersionCatalog = {
  versions: [],
  loadedPages: [],
};

const TABS: Array<{ key: ViewKey; label: string; icon: LucideIcon }> = [
  { key: "studio", label: "Studio Pro", icon: AppWindow },
  { key: "projects", label: "프로젝트", icon: FolderKanban },
  { key: "settings", label: "설정", icon: Settings },
];

type PathField = "sharedDirectory" | "composeFile" | "winboatExecutable";

function App() {
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
  const [projectSearch, setProjectSearch] = useState("");
  const [applyMountNow, setApplyMountNow] = useState(true);
  const toastId = useRef(0);
  const initialized = useRef(false);
  const actionLocks = useRef<Set<string>>(new Set());
  const launchLock = useRef(false);

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
      setWarning(errorText(error));
    }
  }, []);

  const refreshInstalled = useCallback(async () => {
    try {
      setInstalledVersions(await invoke<StudioVersion[]>("get_installed_versions"));
    } catch (error) {
      setWarning(errorText(error));
    }
  }, []);

  const refreshProjects = useCallback(async () => {
    try {
      setProjects(await invoke<MendixProject[]>("get_projects"));
    } catch (error) {
      setWarning(errorText(error));
    }
  }, []);

  const fetchCatalogPage = useCallback(async (page: number, reset = false) => {
    setCatalogLoading(true);
    setCatalogError(null);
    try {
      const next = await invoke<StudioVersionCatalog>("fetch_downloadable_versions", {
        page,
        reset,
      });
      setCatalog(next);
    } catch (error) {
      setCatalogError(errorText(error));
    } finally {
      setCatalogLoading(false);
    }
  }, []);

  const loadInitialData = useCallback(async () => {
    setWarning(null);
    try {
      const loadedConfig = await invoke<AppConfig>("get_config");
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
      if (projectResult.status === "fulfilled") setProjects(projectResult.value);
      if (cacheResult.status === "fulfilled") setCatalog(cacheResult.value);

      const errors = [statusResult, installedResult, projectResult]
        .filter((result) => result.status === "rejected")
        .map((result) => errorText((result as PromiseRejectedResult).reason));
      if (errors.length) setWarning(errors[0]);
    } catch (error) {
      setWarning(errorText(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!initialized.current) {
      initialized.current = true;
      void loadInitialData().then(() => fetchCatalogPage(1));
    }

    const interval = window.setInterval(() => void refreshStatus(), 15_000);
    let unlisten: (() => void) | undefined;
    void listen<DownloadProgress>("studio-download-progress", (event) => {
      setDownloadProgress(event.payload);
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
        notify("error", "작업을 완료하지 못했습니다", errorText(error));
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
      notify("info", "WinBoat Windows를 시작했습니다", "준비가 끝나면 상태가 온라인으로 바뀝니다.");
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
        `Studio Pro ${version.version} 창을 열었습니다`,
        project ? `${project.name} 프로젝트를 열었습니다.` : undefined,
      );
    }).finally(() => {
      launchLock.current = false;
    });
  };

  const launchProject = (project: MendixProject) => {
    const exact = installedVersions.find((version) => version.version === project.version);
    const fallback = exact ?? installedVersions[0];
    if (!fallback) {
      notify("error", "설치된 Studio Pro가 없습니다", "Studio Pro 탭에서 먼저 버전을 설치해 주세요.");
      return;
    }
    if (project.version && !exact) {
      setConfirmation({
        title: `Studio Pro ${fallback.version}으로 열까요?`,
        description: `이 프로젝트의 버전은 ${project.version}입니다. 다른 버전으로 열면 프로젝트가 업그레이드될 수 있습니다.`,
        confirmLabel: "그래도 열기",
        action: () => launchVersion(fallback, project),
      });
      return;
    }
    void launchVersion(fallback, project);
  };

  const askInstall = (version: DownloadableVersion) => {
    setConfirmation({
      title: `Studio Pro ${version.version}을 설치할까요?`,
      description: "공식 설치 파일을 공유 폴더에 받은 뒤 WinBoat Windows에 설치하고 완료를 확인합니다.",
      confirmLabel: "다운로드 및 설치",
      action: () =>
        runAction(`install-${version.version}`, async () => {
          setDownloadProgress({
            version: version.version,
            state: "starting",
            downloadedBytes: 0,
            percentage: 0,
            message: "설치를 준비하고 있습니다.",
          });
          try {
            await invoke("install_studio_pro", { version: version.version });
            await refreshInstalled();
            notify("success", `Studio Pro ${version.version} 설치를 완료했습니다`);
            window.setTimeout(() => setDownloadProgress(null), 5000);
          } catch (error) {
            setDownloadProgress({
              version: version.version,
              state: "failed",
              downloadedBytes: 0,
              message: errorText(error),
            });
            throw error;
          }
        }),
    });
  };

  const askUninstall = (version: StudioVersion) => {
    setConfirmation({
      title: `Studio Pro ${version.version}을 제거할까요?`,
      description: "WinBoat Windows에서 제거를 완료한 뒤 목록을 자동으로 갱신합니다. 공유 폴더의 프로젝트는 삭제하지 않습니다.",
      confirmLabel: "제거",
      danger: true,
      action: () =>
        runAction(`uninstall-${version.version}`, async () => {
          await invoke("uninstall_studio_pro", { version: version.version });
          setInstalledVersions((current) =>
            current.filter((installed) => installed.version !== version.version),
          );
          await refreshInstalled();
          notify("success", `Studio Pro ${version.version} 제거를 완료했습니다`);
        }),
    });
  };

  const cancelDownload = () =>
    runAction("cancel-download", async () => {
      if (await invoke<boolean>("cancel_studio_download")) {
        notify("info", "다운로드 취소를 요청했습니다");
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
            ? "WinBoat와 공유할 Linux 폴더 선택"
            : field === "composeFile"
              ? "WinBoat Compose 파일 선택"
              : "WinBoat 실행 파일 선택",
        filters:
          field === "composeFile"
            ? [{ name: "Compose YAML", extensions: ["yml", "yaml"] }]
            : undefined,
      });
      if (typeof selected === "string") {
        setDraftConfig((current) => (current ? { ...current, [field]: selected } : current));
      }
    } catch (error) {
      notify("error", "경로 선택기를 열지 못했습니다", errorText(error));
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
          result.containerRecreated ? "설정을 저장하고 WinBoat에 적용했습니다" : "설정을 저장했습니다",
          result.mountChanged && !result.containerRecreated
            ? "공유 폴더 변경은 WinBoat를 다음에 다시 만들 때 반영됩니다."
            : undefined,
        );
        await Promise.all([refreshStatus(), refreshProjects()]);
      });

    if (applyMountNow && status?.containerStatus === "running") {
      setConfirmation({
        title: "공유 폴더 변경을 지금 적용할까요?",
        description: "WinBoat Windows가 한 번 다시 연결됩니다. 설치된 앱과 가상 디스크는 유지됩니다.",
        confirmLabel: "저장하고 다시 연결",
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
      notify("success", "현재 WinBoat 구성을 다시 찾았습니다");
      await Promise.all([refreshStatus(), refreshProjects()]);
    });

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
    return needle
      ? catalog.versions.filter((version) => version.version.toLowerCase().includes(needle))
      : catalog.versions;
  }, [catalog.versions, versionSearch]);
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
  const settingsChanged = Boolean(
    config && draftConfig && JSON.stringify(config) !== JSON.stringify(draftConfig),
  );

  if (loading) return <LoadingScreen />;

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">m</span>
          <strong>mendimaru</strong>
        </div>

        <nav className="tabs" aria-label="주 메뉴">
          {TABS.map(({ key, label, icon: Icon }) => (
            <button
              type="button"
              key={key}
              className={activeView === key ? "active" : ""}
              onClick={() => setActiveView(key)}
            >
              <Icon size={16} />
              {label}
            </button>
          ))}
        </nav>

        <div className="winboat-control">
          <span className={`connection ${online ? "online" : "offline"}`}>
            <i />
            {online ? "WinBoat 온라인" : "WinBoat 오프라인"}
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
            {online ? "Windows 열기" : "Windows 시작"}
          </button>
        </div>
      </header>

      {warning && (
        <div className="global-warning">
          <Info size={16} />
          <span>{warning}</span>
          <button type="button" onClick={() => setWarning(null)} aria-label="알림 닫기"><X size={15} /></button>
        </div>
      )}

      <main className="page">
        {activeView === "studio" && (
          <StudioView
            online={online}
            installed={installedVersions}
            available={filteredCatalog}
            availableTotal={catalog.totalCount}
            loadedCount={catalog.versions.length}
            search={versionSearch}
            catalogLoading={catalogLoading}
            catalogError={catalogError}
            hasMore={hasMoreVersions}
            installedSet={installedSet}
            downloadProgress={downloadProgress}
            isLaunching={isLaunching}
            isInstalling={isInstalling}
            isBusy={isBusy}
            onSearch={setVersionSearch}
            onRefreshInstalled={() => void refreshInstalled()}
            onRefreshCatalog={() => void fetchCatalogPage(1, true)}
            onLoadMore={() => void fetchCatalogPage(nextCatalogPage)}
            onLaunch={(version) => void launchVersion(version)}
            onInstall={askInstall}
            onUninstall={askUninstall}
            onCancelDownload={cancelDownload}
          />
        )}

        {activeView === "projects" && config && (
          <ProjectsView
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
              setActiveView("studio");
            }}
          />
        )}

        {activeView === "settings" && draftConfig && (
          <SettingsView
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
        toasts={toasts}
        onDismiss={(id) => setToasts((current) => current.filter((toast) => toast.id !== id))}
      />
      {confirmation && (
        <ConfirmDialog
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
  online,
  installed,
  available,
  availableTotal,
  loadedCount,
  search,
  catalogLoading,
  catalogError,
  hasMore,
  installedSet,
  downloadProgress,
  isLaunching,
  isInstalling,
  isBusy,
  onSearch,
  onRefreshInstalled,
  onRefreshCatalog,
  onLoadMore,
  onLaunch,
  onInstall,
  onUninstall,
  onCancelDownload,
}: {
  online: boolean;
  installed: StudioVersion[];
  available: DownloadableVersion[];
  availableTotal?: number;
  loadedCount: number;
  search: string;
  catalogLoading: boolean;
  catalogError: string | null;
  hasMore: boolean;
  installedSet: Set<string>;
  downloadProgress: DownloadProgress | null;
  isLaunching: boolean;
  isInstalling: boolean;
  isBusy: (key: string) => boolean;
  onSearch: (value: string) => void;
  onRefreshInstalled: () => void;
  onRefreshCatalog: () => void;
  onLoadMore: () => void;
  onLaunch: (version: StudioVersion) => void;
  onInstall: (version: DownloadableVersion) => void;
  onUninstall: (version: StudioVersion) => void;
  onCancelDownload: () => void;
}) {
  return (
    <div className="studio-page">
      <PageTitle
        title="Studio Pro"
        description="WinBoat Windows에 설치된 버전을 실행하거나, 공식 목록에서 새 버전을 설치합니다."
      />

      <section className="section-card installed-section">
        <SectionHeader
          title="설치된 버전"
          count={installed.length}
          action={
            <button type="button" className="icon-button" title="설치된 버전 새로고침" onClick={onRefreshInstalled}>
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
                  {isBusy(`launch-${version.version}`) ? "실행 중" : "실행"}
                </button>
                <button
                  type="button"
                  className="icon-button danger"
                  title={`${version.version} 제거`}
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
              title="설치된 Studio Pro가 없습니다"
              detail={online ? "아래 공식 목록에서 원하는 버전을 설치하세요." : "WinBoat Windows를 먼저 시작하세요."}
            />
          )}
        </div>
      </section>

      <section className="section-card available-section">
        <SectionHeader
          title="설치할 버전"
          meta={availableTotal ? `${loadedCount} / ${availableTotal}개 불러옴` : loadedCount ? `${loadedCount}개 불러옴` : "공식 Marketplace"}
          action={
            <div className="catalog-tools">
              <label className="search-field">
                <Search size={16} />
                <input
                  value={search}
                  onChange={(event) => onSearch(event.target.value)}
                  placeholder="버전 검색"
                  spellCheck={false}
                />
              </label>
              <button type="button" className="icon-button" title="공식 목록 새로고침" onClick={onRefreshCatalog} disabled={catalogLoading}>
                <RefreshCw size={16} className={catalogLoading ? "spin" : ""} />
              </button>
            </div>
          }
        />

        {downloadProgress && (
          <div className="download-bar">
            <div className="download-copy">
              <strong>Studio Pro {downloadProgress.version}</strong>
              <span>{downloadProgress.message}</span>
            </div>
            <div className="progress-track">
              <span style={{ width: `${Math.max(2, downloadProgress.percentage ?? 3)}%` }} />
            </div>
            <b>{downloadProgress.percentage == null ? "…" : `${Math.round(downloadProgress.percentage)}%`}</b>
            {isInstalling && ["connecting", "downloading"].includes(downloadProgress.state) && (
              <button type="button" onClick={onCancelDownload}>취소</button>
            )}
          </div>
        )}

        {catalogError && (
          <div className="inline-error">
            <XCircle size={16} />
            <span>{catalogError}</span>
            <button type="button" onClick={onRefreshCatalog}>다시 시도</button>
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
                    <VersionBadges version={version} />
                  </div>
                  <span>{formatReleaseDate(version.releaseDate)}</span>
                </div>
                <button
                  type="button"
                  className={`button compact ${alreadyInstalled ? "quiet" : "primary"}`}
                  disabled={!online || alreadyInstalled || isInstalling}
                  onClick={() => onInstall(version)}
                >
                  {installingThis ? <LoaderCircle size={15} className="spin" /> : alreadyInstalled ? <CheckCircle2 size={15} /> : <Download size={15} />}
                  {installingThis ? "설치 중" : alreadyInstalled ? "설치됨" : "설치"}
                </button>
              </div>
            );
          })}

          {catalogLoading && available.length === 0 && (
            <div className="loading-inline"><LoaderCircle size={18} className="spin" /> 공식 버전 목록을 불러오는 중입니다</div>
          )}
          {!catalogLoading && available.length === 0 && !catalogError && (
            <EmptyState
              icon={Download}
              title={search ? "검색 결과가 없습니다" : "버전 목록이 비어 있습니다"}
              detail={search ? "다른 버전 번호를 검색해 보세요." : "새로고침해 공식 목록을 다시 불러오세요."}
            />
          )}
        </div>

        {hasMore && !search && (
          <button type="button" className="load-more" onClick={onLoadMore} disabled={catalogLoading}>
            {catalogLoading ? <LoaderCircle size={15} className="spin" /> : null}
            이전 버전 더 불러오기
          </button>
        )}
      </section>
    </div>
  );
}

function ProjectsView({
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
  return (
    <div>
      <PageTitle
        title="프로젝트"
        description="설정한 Linux 공유 디렉터리 안의 Mendix 프로젝트만 표시합니다."
      />

      <div className="workspace-line">
        <FolderOpen size={17} />
        <span title={sharedDirectory}>{sharedDirectory}</span>
        <button type="button" onClick={onOpenWorkspace}>폴더 열기</button>
      </div>

      <section className="section-card">
        <SectionHeader
          title="발견한 프로젝트"
          count={totalProjects}
          action={
            <div className="catalog-tools">
              <label className="search-field project-search">
                <Search size={16} />
                <input value={search} onChange={(event) => onSearch(event.target.value)} placeholder="프로젝트 검색" />
              </label>
              <button type="button" className="icon-button" title="프로젝트 다시 찾기" onClick={onRefresh}>
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
                  <strong>{project.version ?? "버전 미상"}</strong>
                </div>
                <span className="modified">{formatModified(project.lastModified)}</span>
                <div className="row-actions">
                  {project.version && !exactVersionInstalled && (
                    <button type="button" className="button quiet compact" onClick={() => onFindVersion(project.version!)}>
                      버전 찾기
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
                    {isBusy(`launch-${project.version ?? "unknown"}`) ? "여는 중" : "열기"}
                  </button>
                  <button type="button" className="icon-button" title="Linux 폴더 열기" onClick={() => onOpenFolder(project.directory)}>
                    <FolderOpen size={16} />
                  </button>
                </div>
              </div>
            );
          })}
          {projects.length === 0 && (
            <EmptyState
              icon={FolderKanban}
              title={totalProjects ? "검색 결과가 없습니다" : "Mendix 프로젝트를 찾지 못했습니다"}
              detail={totalProjects ? "다른 검색어를 입력해 보세요." : "공유 디렉터리에 .mpr 프로젝트를 두고 다시 찾아보세요."}
            />
          )}
        </div>
      </section>
    </div>
  );
}

function SettingsView({
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
        title="설정"
        description="WinBoat 위치와 Linux 공유 워크스페이스만 지정하면 나머지 연결 정보는 Compose에서 감지합니다."
      />

      <section className="section-card settings-card">
        <div className="settings-heading">
          <div>
            <h2>WinBoat</h2>
            <p>현재 Linux에 설치된 WinBoat와 컨테이너 구성을 지정합니다.</p>
          </div>
          <button type="button" className="button secondary compact" onClick={onRedetect} disabled={isBusy("redetect")}>
            <RefreshCw size={15} className={isBusy("redetect") ? "spin" : ""} /> 자동 감지
          </button>
        </div>
        <PathInput
          label="WinBoat 실행 파일"
          value={config.winboatExecutable}
          onChange={(value) => onChange({ ...config, winboatExecutable: value })}
          onBrowse={() => onChoose("winboatExecutable")}
        />
        <PathInput
          label="Compose 파일"
          value={config.composeFile}
          onChange={(value) => onChange({ ...config, composeFile: value })}
          onBrowse={() => onChoose("composeFile")}
        />
        <label className="simple-field runtime-field">
          <span>컨테이너 런타임</span>
          <select value={config.containerRuntime} onChange={(event) => onChange({ ...config, containerRuntime: event.target.value })}>
            <option value="docker">Docker</option>
            <option value="podman">Podman</option>
          </select>
        </label>
      </section>

      <section className="section-card settings-card">
        <div className="settings-heading">
          <div>
            <h2>공유 워크스페이스</h2>
            <p>프로젝트 목록 탐지와 Windows 설치 파일 전달에 함께 사용합니다.</p>
          </div>
          <span className={`mount-state ${mountMatches ? "ok" : "pending"}`}>
            {mountMatches ? <CheckCircle2 size={14} /> : <Info size={14} />}
            {mountMatches ? "WinBoat와 연결됨" : "적용 필요"}
          </span>
        </div>
        <PathInput
          label="Linux 공유 디렉터리"
          value={config.sharedDirectory}
          onChange={(value) => onChange({ ...config, sharedDirectory: value })}
          onBrowse={() => onChoose("sharedDirectory")}
        />
        <label className="apply-row">
          <input type="checkbox" checked={applyNow} onChange={(event) => onApplyNow(event.target.checked)} />
          <span>
            <strong>공유 폴더 변경을 즉시 적용</strong>
            <small>필요하면 WinBoat Windows를 한 번 다시 연결합니다.</small>
          </span>
        </label>
      </section>

      <div className="settings-actions">
        <span>{changed ? "저장하지 않은 변경 사항이 있습니다." : "설정이 저장되어 있습니다."}</span>
        <button type="button" className="button primary" onClick={onSave} disabled={!changed || isBusy("save-settings")}>
          {isBusy("save-settings") ? <LoaderCircle size={16} className="spin" /> : <CheckCircle2 size={16} />}
          설정 저장
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
  count?: number;
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

function VersionBadges({ version }: { version: DownloadableVersion }) {
  return (
    <span className="badges">
      {version.isLatest && <em className="latest">최신</em>}
      {version.isLts && <em className="lts">LTS</em>}
      {version.isMts && <em className="mts">MTS</em>}
      {version.isBeta && <em className="beta">Beta</em>}
    </span>
  );
}

function PathInput({
  label,
  value,
  onChange,
  onBrowse,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  onBrowse: () => void;
}) {
  return (
    <label className="simple-field path-field">
      <span>{label}</span>
      <div>
        <input value={value} onChange={(event) => onChange(event.target.value)} spellCheck={false} />
        <button type="button" onClick={onBrowse}>찾아보기</button>
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

function ToastStack({ toasts, onDismiss }: { toasts: ToastMessage[]; onDismiss: (id: number) => void }) {
  return (
    <div className="toast-stack" aria-live="polite">
      {toasts.map((toast) => (
        <div className={`toast ${toast.kind}`} key={toast.id}>
          {toast.kind === "success" ? <CheckCircle2 size={18} /> : toast.kind === "error" ? <XCircle size={18} /> : <Info size={18} />}
          <div><strong>{toast.title}</strong>{toast.detail && <span>{toast.detail}</span>}</div>
          <button type="button" onClick={() => onDismiss(toast.id)} aria-label="알림 닫기"><X size={14} /></button>
        </div>
      ))}
    </div>
  );
}

function ConfirmDialog({
  state,
  onCancel,
  onConfirm,
}: {
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
          <button type="button" className="button secondary" onClick={onCancel}>취소</button>
          <button type="button" className={`button ${state.danger ? "danger" : "primary"}`} onClick={onConfirm}>{state.confirmLabel}</button>
        </div>
      </div>
    </div>
  );
}

function formatReleaseDate(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString("ko-KR");
}

function formatModified(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString("ko-KR", { year: "numeric", month: "short", day: "numeric" });
}

function compactPath(path: string) {
  const parts = path.split("/").filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-3).join("/")}` : path;
}

function errorText(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return "알 수 없는 오류가 발생했습니다.";
  }
}

export default App;
