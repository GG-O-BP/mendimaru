import { useCallback, useEffect, useState } from "react";
import { errorCode, errorText } from "../../api/errors";
import { tauriApi } from "../../api/tauri";
import type { DownloadableVersion, DownloadProgress } from "../../domain/types";
import { useTauriSubscription } from "../../shared/hooks/useTauriSubscription";
import type { StudioInstallationDependencies } from "./dependencies";

interface InstallationDependencies extends StudioInstallationDependencies {
  refreshInstalled: () => Promise<void>;
}

export function useStudioInstallation({
  t,
  notify,
  requestConfirmation,
  runAction,
  hasBusyPrefix,
  onWarning,
  refreshInstalled,
}: InstallationDependencies) {
  const [downloadProgress, setDownloadProgress] =
    useState<DownloadProgress | null>(null);

  const subscribeToProgress = useCallback(
    () =>
      tauriApi.onStudioDownloadProgress((incoming) => {
        setDownloadProgress((current) => {
          if (!current || current.version !== incoming.version) return incoming;
          return {
            ...incoming,
            percentage: Math.max(
              current.percentage ?? 0,
              incoming.percentage ?? 0,
            ),
          };
        });
      }),
    [],
  );
  const handleSubscriptionError = useCallback(
    (error: unknown) => onWarning(errorText(error, t)),
    [onWarning, t],
  );
  useTauriSubscription(subscribeToProgress, handleSubscriptionError);

  useEffect(() => {
    if (downloadProgress?.state !== "installed") return undefined;
    const completedVersion = downloadProgress.version;
    const timeout = window.setTimeout(() => {
      setDownloadProgress((current) =>
        current?.version === completedVersion && current.state === "installed"
          ? null
          : current,
      );
    }, 2_200);
    return () => window.clearTimeout(timeout);
  }, [downloadProgress?.state, downloadProgress?.version]);

  const askInstall = useCallback(
    (version: DownloadableVersion) => {
      requestConfirmation({
        title: t("confirm-install-title", { version: version.version }),
        description: t("confirm-install-description"),
        confirmLabel: t("action-download-install"),
        action: () =>
          runAction(`install-${version.version}`, async () => {
            setDownloadProgress({
              version: version.version,
              state: "starting",
              downloadedBytes: 0,
              percentage: 2,
              estimated: true,
              message: t("progress-starting"),
            });
            try {
              await tauriApi.installStudioPro(version.version);
              notify(
                "success",
                t("toast-install-complete", { version: version.version }),
              );
              await refreshInstalled();
            } catch (error) {
              const message = errorText(error, t);
              const cancelled = errorCode(error) === "download_cancelled";
              setDownloadProgress((current) => ({
                version: version.version,
                state:
                  cancelled || current?.state === "cancelled"
                    ? "cancelled"
                    : "failed",
                downloadedBytes:
                  current?.version === version.version
                    ? current.downloadedBytes
                    : 0,
                totalBytes:
                  current?.version === version.version
                    ? current.totalBytes
                    : undefined,
                percentage:
                  current?.version === version.version
                    ? current.percentage
                    : undefined,
                estimated:
                  current?.version === version.version
                    ? current.estimated
                    : false,
                message,
              }));
              if (!cancelled) throw error;
            }
          }),
      });
    },
    [notify, refreshInstalled, requestConfirmation, runAction, t],
  );

  const cancelDownload = useCallback(
    () =>
      runAction("cancel-download", async () => {
        if (await tauriApi.cancelStudioDownload()) {
          notify("info", t("toast-download-cancel-requested"));
        }
      }),
    [notify, runAction, t],
  );

  return {
    downloadProgress,
    isInstalling: hasBusyPrefix("install-"),
    askInstall,
    cancelDownload,
  };
}
