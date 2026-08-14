use crate::models::DownloadableVersion;
use scraper::{Html, Selector};
use std::cmp::Ordering;

const ARTIFACTS_BASE_URL: &str = "https://artifacts.rnd.mendix.com/modelers";

pub(super) fn parse_datagrid_html(html: &str) -> Result<Vec<DownloadableVersion>, String> {
    let document = Html::parse_fragment(html);
    let row_selector = selector("div.widget-datagrid-grid-body div.tr[role=row]")?;
    let cell_selector = selector("div[role=gridcell]")?;
    let version_selector = selector("div > div > a")?;
    let badge_selector = selector("div > div > span")?;
    let link_selector = selector("a[href]")?;

    let mut versions = Vec::new();
    for row in document.select(&row_selector) {
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        if cells.len() < 3 {
            continue;
        }
        let Some(version_link) = cells[0].select(&version_selector).next() else {
            continue;
        };
        let version = normalized_text(version_link.text());
        if !looks_like_version(&version) {
            continue;
        }

        let badges = cells[0]
            .select(&badge_selector)
            .map(|badge| normalized_text(badge.text()).to_uppercase())
            .collect::<Vec<_>>();
        let release_date = non_empty(normalized_text(cells[1].text()));
        let release_notes_url = cells[2]
            .select(&link_selector)
            .next()
            .and_then(|link| link.value().attr("href"))
            .map(ToString::to_string);

        versions.push(DownloadableVersion {
            version,
            release_date,
            release_notes_url,
            is_lts: badges.iter().any(|badge| badge == "LTS"),
            is_beta: badges.iter().any(|badge| badge == "BETA"),
            is_mts: badges.iter().any(|badge| badge == "MTS"),
            is_latest: badges.iter().any(|badge| badge == "LATEST"),
        });
    }
    Ok(versions)
}

fn selector(value: &str) -> Result<Selector, String> {
    Selector::parse(value).map_err(|error| crate::tr!("error-marketplace-selector", error = error))
}

fn normalized_text<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn looks_like_version(value: &str) -> bool {
    let mut parts = value.split('.');
    parts.next().is_some_and(|part| part.parse::<u32>().is_ok())
        && parts.next().is_some_and(|part| part.parse::<u32>().is_ok())
}

pub(super) fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    version_sort_key(right)
        .cmp(&version_sort_key(left))
        .then_with(|| right.cmp(left))
}

fn version_sort_key(version: &str) -> (Vec<u32>, u8, u32) {
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(core, suffix)| (core, Some(suffix)));
    let numeric = core
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or_default())
        .collect();
    let (stage, sequence) = match prerelease {
        None => (2, u32::MAX),
        Some(suffix) if suffix.to_ascii_lowercase().starts_with("rc") => {
            (1, prerelease_sequence(suffix))
        }
        Some(suffix) => (0, prerelease_sequence(suffix)),
    };
    (numeric, stage, sequence)
}

fn prerelease_sequence(suffix: &str) -> u32 {
    suffix
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or_default()
}

pub(super) fn major_version(version: &str) -> Result<u32, String> {
    version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .ok_or_else(|| crate::tr!("error-version-inspect", version = version))
}

pub(super) fn v11_installer_url(version: &str) -> String {
    format!("{ARTIFACTS_BASE_URL}/Mendix-{version}-Setup.exe")
}

pub(super) fn legacy_installer_url(version: &str, build_number: &str) -> String {
    format!("{ARTIFACTS_BASE_URL}/Mendix-{version}.{build_number}-Setup.exe")
}
