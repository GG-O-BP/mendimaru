import type { AppConfig } from "../../domain/types";
import type { MessageKey, Translate } from "../../i18n";

export const ADVANCED_SETTING_FIELDS = [
  "containerName",
  "apiUrl",
  "rdpHost",
  "rdpPort",
  "windowsSharedDirectory",
  "freerdpBinary",
  "mendixInstallRoot",
  "mendixDataRoot",
  "startupTimeoutSeconds",
] as const;

export type AdvancedSettingField = (typeof ADVANCED_SETTING_FIELDS)[number];

export type AdvancedSettingErrors = Partial<
  Record<AdvancedSettingField, MessageKey>
>;

const WINDOWS_PATH_PATTERN =
  /^(?:[A-Za-z]:[\\/][^<>:"|?*]*|\\\\[^<>:"|?*]+\\[^<>:"|?*]*)$/;
const CONTAINER_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9_.-]*$/;

export function validateAdvancedSettings(
  config: AppConfig,
  t: Translate,
): Partial<Record<AdvancedSettingField, string>> {
  const errors: Partial<Record<AdvancedSettingField, string>> = {};
  const add = (field: AdvancedSettingField, key: MessageKey) => {
    errors[field] = t(key);
  };

  if (!CONTAINER_NAME_PATTERN.test(config.containerName.trim())) {
    add("containerName", "error-settings-container-name");
  }

  const apiUrl = safeUrl(config.apiUrl);
  if (
    !apiUrl ||
    (apiUrl.protocol !== "http:" && apiUrl.protocol !== "https:") ||
    apiUrl.username ||
    apiUrl.password ||
    !isLoopbackHost(apiUrl.hostname)
  ) {
    add("apiUrl", "error-settings-loopback-url");
  }

  if (!isLoopbackHost(config.rdpHost)) {
    add("rdpHost", "error-settings-loopback-host");
  }
  if (!isPort(config.rdpPort)) add("rdpPort", "error-settings-port-range");
  if (!isPort(config.startupTimeoutSeconds, 900)) {
    add("startupTimeoutSeconds", "error-settings-timeout-range");
  }
  if (!config.freerdpBinary.trim().startsWith("/")) {
    add("freerdpBinary", "error-settings-linux-absolute-path");
  }
  for (const field of [
    "windowsSharedDirectory",
    "mendixInstallRoot",
    "mendixDataRoot",
  ] as const satisfies AdvancedSettingField[]) {
    const value = config[field].trim();
    if (value.includes("\0") || !WINDOWS_PATH_PATTERN.test(value)) {
      add(field, "error-settings-windows-absolute-path");
    }
  }

  return errors;
}

export function advancedSettingsAreValid(
  config: AppConfig,
  t: Translate,
): boolean {
  return Object.keys(validateAdvancedSettings(config, t)).length === 0;
}

function safeUrl(value: string): URL | null {
  try {
    return new URL(value.trim());
  } catch {
    return null;
  }
}

function isLoopbackHost(value: string): boolean {
  const host = value.trim().toLowerCase();
  return (
    host === "localhost" ||
    host === "::1" ||
    host === "[::1]" ||
    (/^127(?:\.\d{1,3}){3}$/.test(host) &&
      host
        .split(".")
        .slice(1)
        .every((part) => Number(part) <= 255))
  );
}

function isPort(value: number, maximum = 65_535): boolean {
  return Number.isInteger(value) && value >= 1 && value <= maximum;
}
