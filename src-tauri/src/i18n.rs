use crate::models::{LocaleOption, LocalizationBundle};
use chrono::{Datelike, NaiveDate};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use icu_datetime::fieldsets::YMD;
use icu_datetime::input::Date;
use icu_datetime::DateTimeFormatter;
use icu_decimal::input::Decimal;
use icu_decimal::DecimalFormatter;
use icu_locale::Locale;
use rust_embed::RustEmbed;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use writeable::Writeable;

const FALLBACK_LOCALE: &str = "en-US";
const DOMAIN: &str = "mendimaru";
const BYTE_UNIT_KEYS: [&str; 5] = [
    "byte-unit-b",
    "byte-unit-kb",
    "byte-unit-mb",
    "byte-unit-gb",
    "byte-unit-tb",
];
const SUPPORTED_LOCALES: [(&str, &str); 3] = [
    ("en-US", "English"),
    ("ko-KR", "한국어"),
    ("ja-JP", "日本語"),
];

pub const UI_MESSAGE_KEYS: &[&str] = &[
    "app-title",
    "app-description",
    "nav-main-aria",
    "nav-studio",
    "nav-projects",
    "nav-settings",
    "language-label",
    "language-system",
    "connection-online",
    "connection-offline",
    "action-open-windows",
    "action-start-windows",
    "dismiss-notification",
    "generic-action-failed",
    "unknown-error",
    "toast-windows-started",
    "toast-windows-started-detail",
    "toast-studio-opened",
    "toast-project-opened",
    "toast-no-studio",
    "toast-no-studio-detail",
    "confirm-open-fallback-title",
    "confirm-project-version-mismatch",
    "action-open-anyway",
    "confirm-install-title",
    "confirm-install-description",
    "action-download-install",
    "toast-install-complete",
    "confirm-uninstall-title",
    "confirm-uninstall-description",
    "action-uninstall",
    "toast-uninstall-complete",
    "toast-download-cancel-requested",
    "dialog-select-shared-directory",
    "dialog-select-compose-file",
    "dialog-select-winboat-file",
    "dialog-compose-filter",
    "path-picker-failed",
    "toast-settings-applied",
    "toast-settings-saved",
    "toast-mount-deferred",
    "confirm-apply-mount-title",
    "confirm-apply-mount-description",
    "action-save-reconnect",
    "toast-redetected",
    "studio-description",
    "installed-title",
    "refresh-installed",
    "action-launching",
    "action-launch",
    "remove-version-title",
    "empty-installed-title",
    "empty-installed-online",
    "empty-installed-offline",
    "available-title",
    "catalog-loaded-total",
    "catalog-loaded",
    "official-marketplace",
    "search-version-placeholder",
    "support-filter-aria",
    "refresh-catalog",
    "action-retry",
    "action-installing",
    "action-installed",
    "action-install",
    "catalog-loading",
    "search-no-results",
    "catalog-empty",
    "filter-no-results-detail",
    "catalog-empty-detail",
    "catalog-loading-older",
    "badge-latest",
    "badge-beta",
    "progress-starting",
    "progress-preparing",
    "progress-checking",
    "progress-connecting",
    "progress-downloading",
    "progress-downloaded",
    "progress-ready",
    "progress-installing",
    "progress-installed",
    "progress-failed",
    "progress-cancelled",
    "progress-elapsed",
    "progress-installing-short",
    "progress-failed-short",
    "progress-cancelled-short",
    "progress-approximate",
    "progress-complete",
    "progress-aria",
    "action-cancel",
    "duration-hours-minutes-seconds",
    "duration-minutes-seconds",
    "duration-seconds",
    "projects-title",
    "projects-description",
    "action-open-folder",
    "projects-found",
    "search-project-placeholder",
    "refresh-projects",
    "version-unknown",
    "action-find-version",
    "action-opening",
    "action-open",
    "open-linux-folder",
    "projects-search-empty",
    "projects-empty",
    "projects-search-detail",
    "projects-empty-detail",
    "settings-title",
    "settings-description",
    "settings-winboat-description",
    "action-auto-detect",
    "settings-winboat-executable",
    "settings-compose-file",
    "settings-container-runtime",
    "settings-workspace-title",
    "settings-workspace-description",
    "mount-connected",
    "mount-pending",
    "settings-shared-directory",
    "settings-apply-now-title",
    "settings-apply-now-detail",
    "settings-unsaved",
    "settings-saved",
    "action-save-settings",
    "action-browse",
];

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LANGUAGE_LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();

pub fn language_loader() -> &'static FluentLanguageLoader {
    LANGUAGE_LOADER.get_or_init(|| {
        FluentLanguageLoader::new(
            DOMAIN,
            FALLBACK_LOCALE
                .parse()
                .expect("the fallback locale is a valid BCP 47 identifier"),
        )
    })
}

pub fn initialize(preference: &str) -> Result<String, String> {
    language_loader()
        .load_fallback_language(&Localizations)
        .map_err(|error| format!("Could not load fallback language resources: {error}"))?;
    set_language(preference)
}

pub fn set_language(preference: &str) -> Result<String, String> {
    let preference = normalize_preference(preference)?;
    let requested = requested_languages(&preference)?;
    i18n_embed::select(language_loader(), &Localizations, &requested)
        .map_err(|error| translate_args("error-language-load", &[("error", error.to_string())]))?;
    Ok(current_locale())
}

pub fn normalize_preference(preference: &str) -> Result<String, String> {
    if preference.eq_ignore_ascii_case("system") {
        return Ok("system".to_string());
    }
    let normalized = preference.trim().replace('_', "-");
    let language = normalized
        .split('-')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match language.as_str() {
        "en" => Ok("en-US".to_string()),
        "ko" => Ok("ko-KR".to_string()),
        "ja" => Ok("ja-JP".to_string()),
        _ => Err(translate_args(
            "error-language-unsupported",
            &[("locale", preference.to_string())],
        )),
    }
}

pub fn current_locale() -> String {
    language_loader().current_language().to_string()
}

pub fn bundle(preference: &str) -> LocalizationBundle {
    let messages = UI_MESSAGE_KEYS
        .iter()
        .map(|key| ((*key).to_string(), translate(key)))
        .collect::<BTreeMap<_, _>>();
    let locale = current_locale();
    LocalizationBundle {
        direction: text_direction(&locale).to_string(),
        locale,
        preference: preference.to_string(),
        available_locales: SUPPORTED_LOCALES
            .iter()
            .map(|(id, native_name)| LocaleOption {
                id: (*id).to_string(),
                native_name: (*native_name).to_string(),
            })
            .collect(),
        messages,
        numbers: format_numbers(&(0..=100).collect::<Vec<_>>()),
    }
}

pub fn translate(message_id: &str) -> String {
    language_loader().get(message_id)
}

pub fn translate_args(message_id: &str, args: &[(&str, String)]) -> String {
    let values = args
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect::<HashMap<_, _>>();
    language_loader().get_args(message_id, values)
}

pub fn format_dates(values: &[String]) -> Vec<String> {
    let locale = current_icu_locale();
    values
        .iter()
        .map(|value| format_date_for_locale(&locale, value).unwrap_or_else(|| value.clone()))
        .collect()
}

pub fn format_numbers(values: &[u64]) -> Vec<String> {
    let locale = current_icu_locale();
    let formatter = DecimalFormatter::try_new(locale.into(), Default::default()).ok();
    values
        .iter()
        .map(|value| {
            formatter
                .as_ref()
                .map(|formatter| {
                    let decimal = Decimal::from(*value);
                    formatter.format(&decimal).write_to_string().into_owned()
                })
                .unwrap_or_else(|| value.to_string())
        })
        .collect()
}

pub fn format_number(value: u64) -> String {
    format_numbers(&[value])
        .into_iter()
        .next()
        .unwrap_or_else(|| value.to_string())
}

pub fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    let formatted = format_numbers(&[hours, minutes, seconds]);
    if hours > 0 {
        replace_template(
            translate("duration-hours-minutes-seconds"),
            &[
                ("hours", &formatted[0]),
                ("minutes", &formatted[1]),
                ("seconds", &formatted[2]),
            ],
        )
    } else if minutes > 0 {
        replace_template(
            translate("duration-minutes-seconds"),
            &[("minutes", &formatted[1]), ("seconds", &formatted[2])],
        )
    } else {
        replace_template(translate("duration-seconds"), &[("seconds", &formatted[2])])
    }
}

fn replace_template(mut template: String, values: &[(&str, &str)]) -> String {
    for (name, value) in values {
        template = template.replace(&format!("%{name}%"), value);
    }
    template
}

pub fn format_bytes(value: u64) -> String {
    let mut unit_index = 0;
    let mut divisor = 1_u64;
    while unit_index + 1 < BYTE_UNIT_KEYS.len() && value >= divisor.saturating_mul(1024) {
        divisor = divisor.saturating_mul(1024);
        unit_index += 1;
    }

    if unit_index == 0 {
        return replace_template(
            translate(BYTE_UNIT_KEYS[unit_index]),
            &[("value", &format_number(value))],
        );
    }

    let whole = value / divisor;
    let decimal = if whole >= 100 {
        Decimal::from(whole)
    } else {
        let tenths = ((value as u128 * 10 + u128::from(divisor / 2)) / u128::from(divisor)) as u64;
        let mut decimal = Decimal::from(tenths);
        decimal.multiply_pow10(-1);
        decimal
    };
    let locale = current_icu_locale();
    let number = DecimalFormatter::try_new(locale.into(), Default::default())
        .map(|formatter| formatter.format(&decimal).write_to_string().into_owned())
        .unwrap_or_else(|_| whole.to_string());
    replace_template(translate(BYTE_UNIT_KEYS[unit_index]), &[("value", &number)])
}

fn requested_languages(
    preference: &str,
) -> Result<Vec<i18n_embed::unic_langid::LanguageIdentifier>, String> {
    if preference.eq_ignore_ascii_case("system") {
        return Ok(DesktopLanguageRequester::requested_languages());
    }

    Ok(vec![preference
        .parse()
        .expect("supported locales are valid BCP 47 identifiers")])
}

fn current_icu_locale() -> Locale {
    current_locale()
        .parse()
        .unwrap_or_else(|_| FALLBACK_LOCALE.parse().expect("valid ICU fallback locale"))
}

fn format_date_for_locale(locale: &Locale, value: &str) -> Option<String> {
    let date = parse_date(value)?;
    let input = Date::try_new_iso(date.year(), date.month() as u8, date.day() as u8).ok()?;
    let formatter = DateTimeFormatter::try_new(locale.clone().into(), YMD::medium()).ok()?;
    Some(formatter.format(&input).write_to_string().into_owned())
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    if let Ok(date_time) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(date_time.date_naive());
    }
    ["%Y-%m-%d", "%B %e, %Y", "%b %e, %Y"]
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(value, format).ok())
}

fn text_direction(locale: &str) -> &'static str {
    match locale.split('-').next().unwrap_or_default() {
        "ar" | "ckb" | "dv" | "fa" | "he" | "ku" | "ps" | "sd" | "ug" | "ur" => "rtl",
        _ => "ltr",
    }
}

#[macro_export]
macro_rules! tr {
    ($message_id:literal) => {
        i18n_embed_fl::fl!($crate::i18n::language_loader(), $message_id)
    };
    ($message_id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        i18n_embed_fl::fl!(
            $crate::i18n::language_loader(),
            $message_id,
            $($name = ($value).to_string()),+
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::collections::{BTreeMap, BTreeSet};

    const EN: &str = include_str!("../i18n/en-US/mendimaru.ftl");
    const KO: &str = include_str!("../i18n/ko-KR/mendimaru.ftl");
    const JA: &str = include_str!("../i18n/ja-JP/mendimaru.ftl");
    const APP_TSX: &str = include_str!("../../src/App.tsx");

    fn signatures(source: &str) -> BTreeMap<String, BTreeSet<String>> {
        let message = Regex::new(r"^([a-z][a-z0-9-]*)\s*=").expect("message regex");
        let variable = Regex::new(r"\{\s*\$([A-Za-z][A-Za-z0-9-]*)\s*\}").expect("variable regex");
        let template = Regex::new(r"%([A-Za-z][A-Za-z0-9]*)%").expect("template regex");
        let mut result = BTreeMap::<String, BTreeSet<String>>::new();
        let mut current = None;
        for line in source.lines() {
            if let Some(captures) = message.captures(line) {
                let key = captures[1].to_string();
                result.entry(key.clone()).or_default();
                current = Some(key);
            }
            let Some(key) = current.as_ref() else {
                continue;
            };
            for captures in variable.captures_iter(line) {
                result
                    .get_mut(key)
                    .expect("current message exists")
                    .insert(format!("fluent:{}", &captures[1]));
            }
            for captures in template.captures_iter(line) {
                result
                    .get_mut(key)
                    .expect("current message exists")
                    .insert(format!("template:{}", &captures[1]));
            }
        }
        result
    }

    #[test]
    fn every_locale_has_the_same_messages_and_variables() {
        let english = signatures(EN);
        assert_eq!(signatures(KO), english);
        assert_eq!(signatures(JA), english);
    }

    #[test]
    fn every_frontend_message_is_registered() {
        let english = signatures(EN);
        for key in UI_MESSAGE_KEYS {
            assert!(english.contains_key(*key), "missing UI message: {key}");
        }
        for key in BYTE_UNIT_KEYS {
            assert!(
                english.contains_key(key),
                "missing byte-unit message: {key}"
            );
        }
    }

    #[test]
    fn every_static_frontend_translation_call_is_bundled() {
        let usage = Regex::new(r#"\bt\(\s*\"([a-z][a-z0-9-]*)\""#)
            .expect("frontend translation-call regex");
        let label_key =
            Regex::new(r#"labelKey:\s*\"([a-z][a-z0-9-]*)\""#).expect("navigation-label regex");
        let bundled = UI_MESSAGE_KEYS.iter().copied().collect::<BTreeSet<_>>();
        let used = usage
            .captures_iter(APP_TSX)
            .chain(label_key.captures_iter(APP_TSX))
            .map(|captures| captures[1].to_string())
            .collect::<BTreeSet<_>>();
        assert!(
            used.len() > 100,
            "translation-call scan unexpectedly found too few keys"
        );
        for key in used {
            assert!(
                bundled.contains(key.as_str()),
                "frontend message is not bundled: {key}"
            );
        }
    }

    #[test]
    fn every_supported_fluent_resource_loads() {
        let loader = FluentLanguageLoader::new(
            DOMAIN,
            FALLBACK_LOCALE.parse().expect("valid fallback locale"),
        );
        loader
            .load_fallback_language(&Localizations)
            .expect("fallback resource loads");
        for locale in ["en-US", "ko-KR", "ja-JP"] {
            let requested = vec![locale.parse().expect("valid supported locale")];
            i18n_embed::select(&loader, &Localizations, &requested)
                .expect("supported resource loads");
            assert!(!loader.get("app-title").is_empty());
        }
    }

    #[test]
    fn icu_formats_dates_for_each_supported_locale() {
        for locale in ["en-US", "ko-KR", "ja-JP"] {
            let locale = locale.parse::<Locale>().expect("valid locale");
            let formatted = format_date_for_locale(&locale, "2026-07-28").expect("formatted date");
            assert!(!formatted.is_empty());
            assert_ne!(formatted, "2026-07-28");
        }
    }

    #[test]
    fn icu_formats_numbers_and_byte_sizes() {
        initialize("en-US").expect("English localization initializes");
        assert_eq!(format_number(12_345), "12,345");
        assert_eq!(format_bytes(1_536), "1.5 KB");
    }

    #[test]
    fn text_direction_supports_future_rtl_locales() {
        for locale in ["en-US", "ko-KR", "ja-JP"] {
            assert_eq!(text_direction(locale), "ltr");
        }
        assert_eq!(text_direction("ar-EG"), "rtl");
        assert_eq!(text_direction("he-IL"), "rtl");
    }
}
