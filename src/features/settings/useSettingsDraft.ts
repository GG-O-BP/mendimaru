import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { PathField } from "./SettingsPage";
import type { SettingsDraftDependencies } from "./dependencies";
import { configsEqual } from "./environmentState";
import type { EnvironmentRefreshOptions } from "./useEnvironmentStatus";

type SettingsState =
  | { status: "loading" }
  | { status: "recovery" }
  | { status: "ready"; config: AppConfig; draft: AppConfig };

interface UseSettingsDraftOptions extends SettingsDraftDependencies {
  environmentStatus: EnvironmentStatus | null;
  refreshStatus: (options?: EnvironmentRefreshOptions) => Promise<void>;
}

export function useSettingsDraft({
  t,
  notify,
  requestConfirmation,
  runAction,
  onWarning,
  environmentStatus,
  refreshStatus,
}: UseSettingsDraftOptions) {
  const [settings, setSettings] = useState<SettingsState>({
    status: "loading",
  });
  const [applyMountNow, setApplyMountNow] = useState(true);

  useEffect(() => {
    if (settings.status !== "loading") return undefined;
    let active = true;
    void tauriApi
      .getConfig()
      .then((config) => {
        if (active) setSettings({ status: "ready", config, draft: config });
      })
      .catch((error: unknown) => {
        if (!active) return;
        setSettings({ status: "recovery" });
        onWarning(errorText(error, t));
      });
    return () => {
      active = false;
    };
  }, [onWarning, settings.status, t]);

  const applyConfig = useCallback((config: AppConfig) => {
    setSettings({ status: "ready", config, draft: config });
  }, []);

  const updateConfigPair = useCallback(
    (update: (config: AppConfig) => AppConfig) => {
      setSettings((current) =>
        current.status === "ready"
          ? {
              ...current,
              config: update(current.config),
              draft: update(current.draft),
            }
          : current,
      );
    },
    [],
  );

  const setDraftConfig = useCallback((draft: AppConfig) => {
    setSettings((current) =>
      current.status === "ready" ? { ...current, draft } : current,
    );
  }, []);

  const choosePath = useCallback(
    async (field: PathField) => {
      if (settings.status !== "ready") return;
      try {
        const selected = await open({
          directory: field === "sharedDirectory",
          multiple: false,
          defaultPath: settings.draft[field],
          title:
            field === "sharedDirectory"
              ? environmentStatus?.platform.kind === "windows-native"
                ? t("dialog-select-workspace-directory")
                : t("dialog-select-shared-directory")
              : field === "composeFile"
                ? t("dialog-select-compose-file")
                : t("dialog-select-winboat-file"),
          filters:
            field === "composeFile"
              ? [
                  {
                    name: t("dialog-compose-filter"),
                    extensions: ["yml", "yaml"],
                  },
                ]
              : undefined,
        });
        if (typeof selected === "string") {
          setSettings((current) =>
            current.status === "ready"
              ? {
                  ...current,
                  draft: { ...current.draft, [field]: selected },
                }
              : current,
          );
        }
      } catch (error) {
        notify("error", t("path-picker-failed"), errorText(error, t));
      }
    },
    [environmentStatus, notify, settings, t],
  );

  const addStudioPath = useCallback(async () => {
    if (settings.status !== "ready") return;
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        defaultPath:
          settings.draft.windowsStudioPaths[
            settings.draft.windowsStudioPaths.length - 1
          ],
        title: t("dialog-select-studio-executable"),
        filters: [
          {
            name: t("dialog-studio-executable-filter"),
            extensions: ["exe"],
          },
        ],
      });
      if (typeof selected === "string") {
        setSettings((current) =>
          current.status === "ready" &&
          !current.draft.windowsStudioPaths.includes(selected)
            ? {
                ...current,
                draft: {
                  ...current.draft,
                  windowsStudioPaths: [
                    ...current.draft.windowsStudioPaths,
                    selected,
                  ],
                },
              }
            : current,
        );
      }
    } catch (error) {
      notify("error", t("path-picker-failed"), errorText(error, t));
    }
  }, [notify, settings, t]);

  const removeStudioPath = useCallback((index: number) => {
    setSettings((current) =>
      current.status === "ready"
        ? {
            ...current,
            draft: {
              ...current.draft,
              windowsStudioPaths: current.draft.windowsStudioPaths.filter(
                (_path, pathIndex) => pathIndex !== index,
              ),
            },
          }
        : current,
    );
  }, []);

  const saveSettings = useCallback(() => {
    if (settings.status !== "ready") return;
    const draft = settings.draft;
    const applyWinBoatMount = Boolean(
      applyMountNow && environmentStatus?.platform.requiresWinboat,
    );
    const persist = async (composeRevision?: string) => {
      const result = await tauriApi.saveConfig(
        draft,
        applyWinBoatMount,
        composeRevision,
      );
      applyConfig(result.config);
      notify(
        "success",
        result.containerRecreated
          ? t("toast-settings-applied")
          : t("toast-settings-saved"),
        result.mountChanged && !result.containerRecreated
          ? t("toast-mount-deferred")
          : undefined,
      );
      await refreshStatus({ sourceChanged: true });
    };
    const execute = (composeRevision?: string) =>
      runAction("save-settings", () => persist(composeRevision));

    void runAction("save-settings", async () => {
      const preview = await tauriApi.previewSettingsSave(
        draft,
        applyWinBoatMount,
      );
      if (!preview) {
        await persist();
        return;
      }
      if (!preview.mountChanged) {
        await persist(preview.composeRevision);
        return;
      }
      const current =
        preview.currentSharedDirectory ?? t("mount-preview-not-mounted");
      const scope = preview.containerWillRecreate
        ? t("mount-preview-service-scope", { service: preview.serviceName })
        : t("mount-preview-deferred");
      requestConfirmation({
        title: t("confirm-mount-change-title"),
        description: t("confirm-mount-change-preview", {
          service: preview.serviceName,
          current,
          next: preview.nextSharedDirectory,
          scope,
        }),
        confirmLabel: preview.containerWillRecreate
          ? t("action-save-reconnect")
          : t("action-save-settings"),
        action: () => execute(preview.composeRevision),
      });
    });
  }, [
    applyConfig,
    applyMountNow,
    environmentStatus,
    notify,
    refreshStatus,
    requestConfirmation,
    runAction,
    settings,
    t,
  ]);

  const redetectSettings = useCallback(
    () =>
      runAction("redetect", async () => {
        const detected = await tauriApi.redetectConfig();
        applyConfig(detected);
        onWarning(null);
        notify("success", t("toast-redetected"));
        await refreshStatus({ sourceChanged: true });
      }),
    [applyConfig, notify, onWarning, refreshStatus, runAction, t],
  );

  const updateLanguagePreference = useCallback(
    (preference: string) => {
      updateConfigPair((config) => ({
        ...config,
        languagePreference: preference,
      }));
    },
    [updateConfigPair],
  );

  const settingsChanged = useMemo(
    () =>
      settings.status === "ready" &&
      !configsEqual(settings.config, settings.draft),
    [settings],
  );

  return {
    config: settings.status === "ready" ? settings.config : null,
    draftConfig: settings.status === "ready" ? settings.draft : null,
    loading: settings.status === "loading",
    settingsChanged,
    applyMountNow,
    setApplyMountNow,
    setDraftConfig,
    applyConfig,
    updateConfigPair,
    choosePath,
    addStudioPath,
    removeStudioPath,
    saveSettings,
    redetectSettings,
    updateLanguagePreference,
  };
}
