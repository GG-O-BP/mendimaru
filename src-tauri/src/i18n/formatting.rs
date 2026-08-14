use super::{current_locale, translate, BYTE_UNIT_KEYS, FALLBACK_LOCALE};
use chrono::{Datelike, NaiveDate};
use icu_datetime::fieldsets::YMD;
use icu_datetime::input::Date;
use icu_datetime::DateTimeFormatter;
use icu_decimal::input::Decimal;
use icu_decimal::DecimalFormatter;
use icu_locale::Locale;
use writeable::Writeable;

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

fn current_icu_locale() -> Locale {
    current_locale()
        .parse()
        .unwrap_or_else(|_| FALLBACK_LOCALE.parse().expect("valid ICU fallback locale"))
}

pub(super) fn format_date_for_locale(locale: &Locale, value: &str) -> Option<String> {
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
