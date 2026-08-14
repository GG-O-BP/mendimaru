import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { Translate } from "../../i18n";
import type { WinBoatControlKind } from "../studio/types";

export interface EnvironmentPresentation {
  online: boolean;
  controlKind: WinBoatControlKind;
  actionKey: string;
  actionLabel: string;
  offlineGuidance: {
    title: string;
    detail: string;
  };
}

export function deriveEnvironmentPresentation(
  status: EnvironmentStatus | null,
  t: Translate,
): EnvironmentPresentation {
  const online = Boolean(status?.guestOnline);
  const controlKind = deriveControlKind(status, online);

  return {
    online,
    controlKind,
    actionKey: actionKeyFor(controlKind),
    actionLabel: actionLabelFor(controlKind, status, online, t),
    offlineGuidance: offlineGuidanceFor(status, t),
  };
}

function deriveControlKind(
  status: EnvironmentStatus | null,
  online: boolean,
): WinBoatControlKind {
  if (!status?.winboatAvailable) return "settings";
  if (!status.winboatInitialized) return "setup";
  if (online || status.containerStatus === "running") return "open";
  return "start";
}

function actionKeyFor(controlKind: WinBoatControlKind) {
  switch (controlKind) {
    case "settings":
      return "winboat-settings";
    case "setup":
      return "setup-winboat";
    case "open":
      return "open-winboat";
    case "start":
      return "start-windows";
  }
}

function actionLabelFor(
  controlKind: WinBoatControlKind,
  status: EnvironmentStatus | null,
  online: boolean,
  t: Translate,
) {
  switch (controlKind) {
    case "settings":
      return t("action-check-winboat-settings");
    case "setup":
      return t(
        status?.setupPending
          ? "action-continue-winboat-setup"
          : "action-setup-winboat",
      );
    case "open":
      return t(
        status?.setupPending && !online
          ? "action-open-winboat-setup"
          : "action-open-winboat",
      );
    case "start":
      return t("action-start-windows");
  }
}

function offlineGuidanceFor(status: EnvironmentStatus | null, t: Translate) {
  if (!status?.winboatAvailable) {
    return {
      title: t("winboat-missing-title"),
      detail: t("winboat-missing-detail"),
    };
  }
  if (status.setupPending) {
    return {
      title: t("winboat-setup-pending-title"),
      detail: t("winboat-setup-pending-detail"),
    };
  }
  if (!status.winboatInitialized) {
    return {
      title: t("winboat-setup-required-title"),
      detail: t("winboat-setup-required-detail"),
    };
  }
  if (status.containerStatus === "running") {
    return {
      title: t("windows-preparing-title"),
      detail: t("windows-preparing-detail"),
    };
  }
  return {
    title: t("offline-guidance-title"),
    detail: t("offline-guidance-detail"),
  };
}

const CONFIG_FIELD_MAP = {
  languagePreference: true,
  winboatSetupPending: true,
  winboatExecutable: true,
  composeFile: true,
  containerRuntime: true,
  containerName: true,
  apiUrl: true,
  rdpHost: true,
  rdpPort: true,
  sharedDirectory: true,
  windowsSharedDirectory: true,
  freerdpBinary: true,
  mendixInstallRoot: true,
  mendixDataRoot: true,
  startupTimeoutSeconds: true,
} satisfies Record<keyof AppConfig, true>;

const CONFIG_FIELDS = Object.keys(CONFIG_FIELD_MAP) as Array<keyof AppConfig>;

export function configsEqual(left: AppConfig, right: AppConfig) {
  return CONFIG_FIELDS.every((field) => left[field] === right[field]);
}
