import type {
  EnvironmentDiagnostic,
  EnvironmentDiagnosticAction,
  EnvironmentDiagnosticId,
} from "../../domain/types";
import type { MessageKey, Translate } from "../../i18n";

const DIAGNOSTIC_MESSAGES: Record<
  EnvironmentDiagnosticId,
  { title: MessageKey; recovery: MessageKey; target: string }
> = {
  winboat: {
    title: "diagnostic-winboat-title",
    recovery: "diagnostic-winboat-recovery",
    target: "environment-settings-heading",
  },
  compose: {
    title: "diagnostic-compose-title",
    recovery: "diagnostic-compose-recovery",
    target: "compose-file-input",
  },
  "container-runtime": {
    title: "diagnostic-runtime-title",
    recovery: "diagnostic-runtime-recovery",
    target: "container-runtime-input",
  },
  freerdp: {
    title: "diagnostic-freerdp-title",
    recovery: "diagnostic-freerdp-recovery",
    target: "environment-settings-heading",
  },
  "shared-directory": {
    title: "diagnostic-shared-directory-title",
    recovery: "diagnostic-shared-directory-recovery",
    target: "shared-directory-input",
  },
  "shared-mount": {
    title: "diagnostic-shared-mount-title",
    recovery: "diagnostic-shared-mount-recovery",
    target: "apply-mount-input",
  },
  container: {
    title: "diagnostic-container-title",
    recovery: "diagnostic-container-recovery",
    target: "environment-settings-heading",
  },
  "guest-api": {
    title: "diagnostic-guest-api-title",
    recovery: "diagnostic-guest-api-recovery",
    target: "environment-settings-heading",
  },
  rdp: {
    title: "diagnostic-rdp-title",
    recovery: "diagnostic-rdp-recovery",
    target: "environment-settings-heading",
  },
  "marketplace-browser": {
    title: "diagnostic-browser-title",
    recovery: "diagnostic-browser-recovery",
    target: "environment-settings-heading",
  },
};

const ACTION_MESSAGES: Record<EnvironmentDiagnosticAction, MessageKey> = {
  redetect: "diagnostic-action-redetect",
  "start-winboat": "diagnostic-action-start",
  "open-winboat": "diagnostic-action-open-winboat",
  "open-settings": "diagnostic-action-open-settings",
};

export function diagnosticText(
  diagnostic: EnvironmentDiagnostic,
  t: Translate,
) {
  const messages = DIAGNOSTIC_MESSAGES[diagnostic.id];
  return {
    title: t(messages.title),
    detail:
      diagnostic.status === "success"
        ? diagnostic.observed
          ? t("diagnostic-detected", { value: diagnostic.observed })
          : t("diagnostic-check-passed")
        : t(messages.recovery),
    status: t(`diagnostic-status-${diagnostic.status}`),
    action: diagnostic.action ? t(ACTION_MESSAGES[diagnostic.action]) : null,
  };
}

export function diagnosticTarget(id: EnvironmentDiagnosticId) {
  return DIAGNOSTIC_MESSAGES[id].target;
}
