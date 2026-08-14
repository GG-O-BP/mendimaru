import { getCurrentWindow } from "@tauri-apps/api/window";
import { tauriApi } from "./api/tauri";
import type { LocalizationBundle } from "./domain/types";
import uiMessageRegistry from "./shared/contracts/uiMessages.json";

export type TranslationValues = Record<string, string | number>;
export type MessageKey = keyof typeof uiMessageRegistry;
export type Translate = (key: MessageKey, values?: TranslationValues) => string;

const FALLBACK_MESSAGES: Partial<Record<MessageKey, string>> = {
  "app-title": "mendimaru — Studio Pro for Linux",
  "app-description": "Manage Mendix Studio Pro on Linux through WinBoat",
  "language-label": "Language",
  "language-system": "System language",
  "unknown-error": "An unknown error occurred.",
};

export function createTranslate(bundle: LocalizationBundle | null): Translate {
  return (key, values) => {
    const source = bundle?.messages[key] ?? FALLBACK_MESSAGES[key] ?? key;
    if (!values) return source;
    return source.replace(/%([A-Za-z][A-Za-z0-9]*)%/g, (match, name: string) =>
      Object.prototype.hasOwnProperty.call(values, name)
        ? isolateIfNeeded(String(values[name]), bundle?.direction)
        : match,
    );
  };
}

function isolateIfNeeded(
  value: string,
  direction?: LocalizationBundle["direction"],
) {
  return direction === "rtl" ? `\u2068${value}\u2069` : value;
}

export async function loadLocalization(): Promise<LocalizationBundle> {
  return tauriApi.getLocalization();
}

export async function selectLanguage(
  language: string,
): Promise<LocalizationBundle> {
  return tauriApi.setLanguagePreference(language);
}

export async function formatDates(values: string[]): Promise<string[]> {
  return tauriApi.formatDates(values);
}

export async function formatNumbers(values: number[]): Promise<string[]> {
  return tauriApi.formatNumbers(values);
}

export async function formatByteValues(values: number[]): Promise<string[]> {
  return tauriApi.formatBytes(values);
}

export function applyDocumentLocale(bundle: LocalizationBundle) {
  const title = bundle.messages["app-title"] ?? FALLBACK_MESSAGES["app-title"];
  document.documentElement.lang = bundle.locale;
  document.documentElement.dir = bundle.direction;
  document.title = title;
  document
    .querySelector('meta[name="description"]')
    ?.setAttribute(
      "content",
      bundle.messages["app-description"] ??
        FALLBACK_MESSAGES["app-description"],
    );
  void getCurrentWindow()
    .setTitle(title)
    .catch(() => undefined);
}
