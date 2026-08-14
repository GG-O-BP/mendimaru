import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { StudioVersion } from "../../domain/types";
import type { InstalledVersionsDependencies } from "./dependencies";

export function useInstalledVersions({
  t,
  notify,
  requestConfirmation,
  runAction,
  hasBusyPrefix,
  onWarning,
}: InstalledVersionsDependencies) {
  const [installedVersions, setInstalledVersions] = useState<StudioVersion[]>(
    [],
  );
  const launchLock = useRef(false);

  const refreshInstalled = useCallback(
    async (silent = false) => {
      try {
        setInstalledVersions(await tauriApi.getInstalledVersions());
      } catch (error) {
        if (!silent) onWarning(errorText(error, t));
      }
    },
    [onWarning, t],
  );

  useEffect(() => {
    const initialRefresh = window.setTimeout(
      () => void refreshInstalled(true),
      0,
    );
    return () => window.clearTimeout(initialRefresh);
  }, [refreshInstalled]);

  const launchVersion = useCallback(
    (version: StudioVersion, projectMprPath?: string, projectName?: string) => {
      if (launchLock.current) return Promise.resolve();
      launchLock.current = true;
      return runAction(`launch-${version.version}`, async () => {
        await tauriApi.launchStudioPro(version.version, projectMprPath);
        notify(
          "success",
          t("toast-studio-opened", { version: version.version }),
          projectName
            ? t("toast-project-opened", { project: projectName })
            : undefined,
        );
      }).finally(() => {
        launchLock.current = false;
      });
    },
    [notify, runAction, t],
  );

  const askUninstall = useCallback(
    (version: StudioVersion) => {
      requestConfirmation({
        title: t("confirm-uninstall-title", { version: version.version }),
        description: t("confirm-uninstall-description"),
        confirmLabel: t("action-uninstall"),
        danger: true,
        action: () =>
          runAction(`uninstall-${version.version}`, async () => {
            await tauriApi.uninstallStudioPro(version.version);
            setInstalledVersions((current) =>
              current.filter(
                (installed) => installed.version !== version.version,
              ),
            );
            await refreshInstalled();
            notify(
              "success",
              t("toast-uninstall-complete", { version: version.version }),
            );
          }),
      });
    },
    [notify, refreshInstalled, requestConfirmation, runAction, t],
  );

  const installedSet = useMemo(
    () => new Set(installedVersions.map((version) => version.version)),
    [installedVersions],
  );

  return {
    installedVersions,
    installedSet,
    isLaunching: hasBusyPrefix("launch-"),
    refreshInstalled,
    launchVersion,
    askUninstall,
  };
}
