import type { AppConfig, EnvironmentStatus } from "../../domain/types";
import type { Translate } from "../../i18n";
import type { EnvironmentControlKind } from "../studio/types";

export interface EnvironmentPresentation {
  online: boolean;
  controlKind: EnvironmentControlKind;
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
  const nativeWindows = status?.platform.kind === "windows-native";
  const online = nativeWindows
    ? Boolean(status?.ready)
    : Boolean(status?.guestOnline);
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
): EnvironmentControlKind {
  if (status?.platform.kind === "windows-native") return "native";
  if (!status?.winboatAvailable) return "settings";
  if (!status.winboatInitialized) return "setup";
  if (online || status.containerStatus === "running") return "open";
  return "start";
}

function actionKeyFor(controlKind: EnvironmentControlKind) {
  switch (controlKind) {
    case "settings":
      return "winboat-settings";
    case "setup":
      return "setup-winboat";
    case "open":
      return "open-winboat";
    case "start":
      return "start-windows";
    case "native":
      return "native-windows-settings";
  }
}

function actionLabelFor(
  controlKind: EnvironmentControlKind,
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
    case "native":
      return t("action-open-settings");
  }
}

function offlineGuidanceFor(status: EnvironmentStatus | null, t: Translate) {
  if (status?.platform.kind === "windows-native") {
    return {
      title: t("native-workspace-missing-title"),
      detail: t("native-workspace-missing-detail"),
    };
  }
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
  windowsStudioPaths: true,
  startupTimeoutSeconds: true,
} satisfies Record<keyof AppConfig, true>;

const CONFIG_FIELDS = Object.keys(CONFIG_FIELD_MAP) as Array<keyof AppConfig>;

export function configsEqual(left: AppConfig, right: AppConfig) {
  return CONFIG_FIELDS.every((field) => {
    const leftValue = left[field];
    const rightValue = right[field];
    if (Array.isArray(leftValue) && Array.isArray(rightValue)) {
      return (
        leftValue.length === rightValue.length &&
        leftValue.every((value, index) => value === rightValue[index])
      );
    }
    return leftValue === rightValue;
  });
}
