import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { PathField } from "./SettingsPage";
import type { SettingsDraftDependencies } from "./dependencies";
import { configsEqual } from "./environmentState";

type SettingsState =
  | { status: "loading" }
  | { status: "recovery" }
  | { status: "ready"; config: AppConfig; draft: AppConfig };

interface UseSettingsDraftOptions extends SettingsDraftDependencies {
  environmentStatus: EnvironmentStatus | null;
  refreshStatus: () => Promise<void>;
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
              ? t("dialog-select-shared-directory")
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
    [notify, settings, t],
  );

  const saveSettings = useCallback(() => {
    if (settings.status !== "ready") return;
    const draft = settings.draft;
    const execute = () =>
      runAction("save-settings", async () => {
        const result = await tauriApi.saveConfig(draft, applyMountNow);
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
        await refreshStatus();
      });

    if (applyMountNow && environmentStatus?.containerStatus === "running") {
      requestConfirmation({
        title: t("confirm-apply-mount-title"),
        description: t("confirm-apply-mount-description"),
        confirmLabel: t("action-save-reconnect"),
        action: execute,
      });
    } else {
      void execute();
    }
  }, [
    applyConfig,
    applyMountNow,
    environmentStatus?.containerStatus,
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
        await refreshStatus();
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
    saveSettings,
    redetectSettings,
    updateLanguagePreference,
  };
}
