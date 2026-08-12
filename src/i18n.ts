import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { LocalizationBundle } from "./types";

export type TranslationValues = Record<string, string | number>;
export type Translate = (key: string, values?: TranslationValues) => string;

const FALLBACK_MESSAGES: Record<string, string> = {
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

function isolateIfNeeded(value: string, direction?: LocalizationBundle["direction"]) {
  return direction === "rtl" ? `\u2068${value}\u2069` : value;
}

export async function loadLocalization(): Promise<LocalizationBundle> {
  return invoke<LocalizationBundle>("get_localization");
}

export async function selectLanguage(language: string): Promise<LocalizationBundle> {
  return invoke<LocalizationBundle>("set_language_preference", { language });
}

export async function formatDates(values: string[]): Promise<string[]> {
  return invoke<string[]>("format_localized_dates", { values });
}

export async function formatNumbers(values: number[]): Promise<string[]> {
  return invoke<string[]>("format_localized_numbers", { values });
}

export async function formatDuration(totalSeconds: number): Promise<string> {
  return invoke<string>("format_localized_duration", { totalSeconds });
}

export async function formatByteValues(values: number[]): Promise<string[]> {
  return invoke<string[]>("format_localized_bytes", { values });
}

export function applyDocumentLocale(bundle: LocalizationBundle) {
  const title = bundle.messages["app-title"] ?? FALLBACK_MESSAGES["app-title"];
  document.documentElement.lang = bundle.locale;
  document.documentElement.dir = bundle.direction;
  document.title = title;
  document
    .querySelector('meta[name="description"]')
    ?.setAttribute("content", bundle.messages["app-description"] ?? FALLBACK_MESSAGES["app-description"]);
  void getCurrentWindow().setTitle(title).catch(() => undefined);
}
