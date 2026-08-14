use crate::models::{LocaleOption, LocalizationBundle, TextDirection};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use rust_embed::RustEmbed;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

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

mod formatting;
mod ui_messages;

#[cfg(test)]
use formatting::format_date_for_locale;
pub use formatting::{format_bytes, format_dates, format_number, format_numbers};
use ui_messages::ui_message_keys;

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
    let messages = ui_message_keys()
        .iter()
        .map(|key| (key.clone(), translate(key)))
        .collect::<BTreeMap<_, _>>();
    let locale = current_locale();
    LocalizationBundle {
        direction: text_direction(&locale),
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

fn text_direction(locale: &str) -> TextDirection {
    match locale.split('-').next().unwrap_or_default() {
        "ar" | "ckb" | "dv" | "fa" | "he" | "ku" | "ps" | "sd" | "ug" | "ur" => TextDirection::Rtl,
        _ => TextDirection::Ltr,
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
    use icu_locale::Locale;
    use regex::Regex;
    use std::collections::{BTreeMap, BTreeSet};

    const EN: &str = include_str!("../i18n/en-US/mendimaru.ftl");
    const KO: &str = include_str!("../i18n/ko-KR/mendimaru.ftl");
    const JA: &str = include_str!("../i18n/ja-JP/mendimaru.ftl");

    fn frontend_source() -> String {
        let source_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
        let mut paths = walkdir::WalkDir::new(source_directory)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("ts" | "tsx")
                )
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
            .into_iter()
            .map(|path| std::fs::read_to_string(path).expect("frontend source is readable"))
            .collect::<Vec<_>>()
            .join("\n")
    }

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
        for key in ui_message_keys() {
            assert!(english.contains_key(key), "missing UI message: {key}");
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
        let bundled = ui_message_keys()
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let source = frontend_source();
        let used = usage
            .captures_iter(&source)
            .chain(label_key.captures_iter(&source))
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
            assert_eq!(text_direction(locale), TextDirection::Ltr);
        }
        assert_eq!(text_direction("ar-EG"), TextDirection::Rtl);
        assert_eq!(text_direction("he-IL"), TextDirection::Rtl);
    }
}
