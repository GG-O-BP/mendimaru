use crate::config;
use crate::models::{DownloadableVersion, StudioVersionCatalog};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use regex::Regex;
use scraper::{Html, Selector};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio::time::timeout;

const MARKETPLACE_URL: &str = "https://marketplace.mendix.com/link/studiopro";
const ARTIFACTS_BASE_URL: &str = "https://artifacts.rnd.mendix.com/modelers";
const CACHE_FILE_NAME: &str = "studio-version-catalog.json";
const PROFILE_PREFIX: &str = "mendimaru-marketplace-";
const PAGE_SIZE: u32 = 10;
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(60);
const ELEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const PAGE_CHANGE_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const DATAGRID_SELECTOR: &str = "div.widget-datagrid-content";
const DATAGRID_ROW_SELECTOR: &str =
    "div.widget-datagrid-content div.widget-datagrid-grid-body div.tr[role=row] a.mx-name-actionButton_VersionName1";
const NEXT_PAGE_SELECTOR: &str = "button[aria-label='Go to next page']";
const PAGING_STATUS_SELECTOR: &str = "div.paging-status";
const BUILD_NUMBER_SELECTOR: &str = "span.mx-text.pds-heading--sm.pds-mb-0";

static SCRAPE_LOCK: Mutex<()> = Mutex::const_new(());
static PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn load_cached_catalog(app: &AppHandle) -> Result<StudioVersionCatalog, String> {
    let path = cache_path(app)?;
    if !path.is_file() {
        return Ok(StudioVersionCatalog::default());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Studio Pro 버전 캐시를 읽을 수 없습니다: {error}"))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Studio Pro 버전 캐시가 올바르지 않습니다: {error}"))
}

pub async fn fetch_catalog_page(
    app: &AppHandle,
    requested_page: u32,
    reset: bool,
) -> Result<StudioVersionCatalog, String> {
    let target_page = requested_page.max(1);
    let (fresh_versions, total_count) = {
        let _scrape_guard = SCRAPE_LOCK.lock().await;
        scrape_page(target_page).await?
    };

    if fresh_versions.is_empty() {
        return Err("Mendix Marketplace에서 Studio Pro 버전을 찾지 못했습니다.".to_string());
    }

    let mut catalog = if reset {
        StudioVersionCatalog::default()
    } else {
        load_cached_catalog(app).unwrap_or_default()
    };

    for fresh in fresh_versions {
        if let Some(index) = catalog
            .versions
            .iter()
            .position(|cached| cached.version == fresh.version)
        {
            catalog.versions[index] = fresh;
        } else {
            catalog.versions.push(fresh);
        }
    }

    catalog
        .versions
        .sort_by(|left, right| compare_versions_desc(&left.version, &right.version));
    if !catalog.loaded_pages.contains(&target_page) {
        catalog.loaded_pages.push(target_page);
        catalog.loaded_pages.sort_unstable();
    }
    catalog.total_count = total_count.or(catalog.total_count);
    catalog.fetched_at = Some(chrono::Utc::now().to_rfc3339());
    save_catalog(app, &catalog)?;
    Ok(catalog)
}

pub async fn installer_url(version: &str) -> Result<String, String> {
    if major_version(version)? >= 11 {
        return Ok(v11_installer_url(version));
    }

    let build_number = {
        let _scrape_guard = SCRAPE_LOCK.lock().await;
        scrape_build_number(version).await?
    };
    Ok(legacy_installer_url(version, &build_number))
}

fn cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|directory| directory.join(CACHE_FILE_NAME))
        .map_err(|error| format!("앱 캐시 경로를 찾을 수 없습니다: {error}"))
}

fn save_catalog(app: &AppHandle, catalog: &StudioVersionCatalog) -> Result<(), String> {
    let path = cache_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Studio Pro 버전 캐시 폴더를 만들 수 없습니다: {error}"))?;
    }
    let content = serde_json::to_string_pretty(catalog)
        .map_err(|error| format!("Studio Pro 버전 캐시를 만들 수 없습니다: {error}"))?;
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, content)
        .map_err(|error| format!("Studio Pro 버전 캐시를 저장할 수 없습니다: {error}"))?;
    fs::rename(&temporary_path, &path)
        .map_err(|error| format!("Studio Pro 버전 캐시를 확정할 수 없습니다: {error}"))
}

async fn scrape_page(target_page: u32) -> Result<(Vec<DownloadableVersion>, Option<u32>), String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let page = session.navigate(MARKETPLACE_URL).await?;
        dismiss_privacy_modal(&page).await;
        wait_for_selector(&page, DATAGRID_ROW_SELECTOR, ELEMENT_TIMEOUT).await?;
        navigate_to_page(&page, target_page).await?;
        let html = element_inner_html(&page, DATAGRID_SELECTOR).await?;
        let versions = parse_datagrid_html(&html)?;
        let total_count = read_total_count(&page).await;
        Ok((versions, total_count))
    }
    .await;
    session.cleanup().await;
    result
}

async fn scrape_build_number(version: &str) -> Result<String, String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let page = session
            .navigate(&format!("{MARKETPLACE_URL}/{version}"))
            .await?;
        dismiss_privacy_modal(&page).await;
        let expression = Regex::new(r"Build\s+(\d+)").expect("valid build number regex");
        let started = Instant::now();

        loop {
            if let Ok(elements) = page.find_elements(BUILD_NUMBER_SELECTOR).await {
                for element in elements {
                    if let Ok(Some(text)) = element.inner_text().await {
                        if let Some(build) = expression
                            .captures(&text)
                            .and_then(|captures| captures.get(1))
                            .map(|value| value.as_str().to_string())
                        {
                            return Ok(build);
                        }
                    }
                }
            }
            if started.elapsed() >= ELEMENT_TIMEOUT {
                return Err(format!(
                    "Studio Pro {version}의 Mendix 빌드 번호를 찾지 못했습니다."
                ));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    .await;
    session.cleanup().await;
    result
}

fn parse_datagrid_html(html: &str) -> Result<Vec<DownloadableVersion>, String> {
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
    Selector::parse(value)
        .map_err(|error| format!("Marketplace 선택자가 올바르지 않습니다: {error}"))
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

fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    let left_parts = numeric_version_parts(left);
    let right_parts = numeric_version_parts(right);
    let length = left_parts.len().max(right_parts.len());
    for index in 0..length {
        let comparison = right_parts
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&left_parts.get(index).copied().unwrap_or_default());
        if comparison != Ordering::Equal {
            return comparison;
        }
    }
    right.cmp(left)
}

fn numeric_version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or_default()
        })
        .collect()
}

fn major_version(version: &str) -> Result<u32, String> {
    version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .ok_or_else(|| format!("Studio Pro 버전을 확인할 수 없습니다: {version}"))
}

fn v11_installer_url(version: &str) -> String {
    format!("{ARTIFACTS_BASE_URL}/Mendix-{version}-Setup.exe")
}

fn legacy_installer_url(version: &str, build_number: &str) -> String {
    format!("{ARTIFACTS_BASE_URL}/Mendix-{version}.{build_number}-Setup.exe")
}

async fn navigate_to_page(page: &Page, target_page: u32) -> Result<(), String> {
    if target_page <= 1 {
        return Ok(());
    }

    for current_page in 1..target_page {
        let expected_start = current_page * PAGE_SIZE + 1;
        click_next_page(page).await?;
        wait_for_page_start(page, expected_start).await?;
    }
    Ok(())
}

async fn click_next_page(page: &Page) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if let Ok(button) = page.find_element(NEXT_PAGE_SELECTOR).await {
            if button.attribute("disabled").await.ok().flatten().is_none() {
                page.evaluate(
                    "document.querySelector(\"button[aria-label='Go to next page']:not([disabled])\")?.click()",
                )
                    .await
                    .map_err(|error| format!("다음 버전 페이지를 열 수 없습니다: {error}"))?;
                tokio::time::sleep(Duration::from_secs(2)).await;
                return Ok(());
            }
        }
        if started.elapsed() >= PAGE_CHANGE_TIMEOUT {
            return Err("더 이상 불러올 Studio Pro 버전 페이지가 없습니다.".to_string());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_page_start(page: &Page, expected_start: u32) -> Result<(), String> {
    let status_pattern =
        Regex::new(r"(\d+)\s+(?:to|-)\s+(\d+)\s+of\s+(\d+)").expect("valid paging status regex");
    let started = Instant::now();
    let mut last_status = String::new();
    loop {
        if let Ok(status) = page.find_element(PAGING_STATUS_SELECTOR).await {
            if let Ok(Some(text)) = status.inner_text().await {
                last_status = text.trim().to_string();
                let current_start = status_pattern
                    .captures(&last_status)
                    .and_then(|captures| captures.get(1))
                    .and_then(|value| value.as_str().parse::<u32>().ok());
                if current_start == Some(expected_start) {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    return Ok(());
                }
            }
        }
        if started.elapsed() >= PAGE_CHANGE_TIMEOUT {
            return Err(format!(
                "Studio Pro 버전 목록의 {expected_start}번째 항목으로 이동하지 못했습니다. 마지막 페이지 상태: {last_status}"
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn read_total_count(page: &Page) -> Option<u32> {
    let status = page.find_element(PAGING_STATUS_SELECTOR).await.ok()?;
    let text = status.inner_text().await.ok().flatten()?;
    Regex::new(r"of\s+(\d+)")
        .ok()?
        .captures(&text)?
        .get(1)?
        .as_str()
        .parse::<u32>()
        .ok()
}

async fn wait_for_selector(
    page: &Page,
    value: &str,
    wait_duration: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if page
            .find_elements(value)
            .await
            .is_ok_and(|elements| !elements.is_empty())
        {
            return Ok(());
        }
        if started.elapsed() >= wait_duration {
            return Err(format!(
                "Mendix Marketplace 응답 대기 시간이 초과되었습니다: {value}"
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn element_inner_html(page: &Page, value: &str) -> Result<String, String> {
    let element = page
        .find_element(value)
        .await
        .map_err(|error| format!("Studio Pro 버전 목록을 찾을 수 없습니다: {error}"))?;
    element
        .inner_html()
        .await
        .map_err(|error| format!("Studio Pro 버전 목록을 읽을 수 없습니다: {error}"))?
        .ok_or_else(|| "Studio Pro 버전 목록이 비어 있습니다.".to_string())
}

async fn dismiss_privacy_modal(page: &Page) {
    for selector in [
        "[data-testid='uc-deny-all-button']",
        "[data-testid='uc-reject-all-button']",
        "button[class*='reject']",
        "button[class*='deny']",
    ] {
        if let Ok(button) = page.find_element(selector).await {
            let _ = button.click().await;
            break;
        }
    }
}

fn chrome_executable() -> Option<String> {
    if let Ok(custom) = std::env::var("MENDIMARU_CHROME_PATH") {
        if Path::new(&custom).is_file() {
            return Some(custom);
        }
    }
    config::find_binary(&[
        "google-chrome-stable",
        "google-chrome",
        "chromium",
        "chromium-browser",
    ])
}

fn next_profile_directory() -> PathBuf {
    let sequence = PROFILE_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    std::env::temp_dir().join(format!("{PROFILE_PREFIX}{}-{sequence}", std::process::id()))
}

struct BrowserSession {
    browser: Browser,
    handler_task: tokio::task::JoinHandle<()>,
    profile_directory: PathBuf,
}

impl BrowserSession {
    async fn new() -> Result<Self, String> {
        let chrome_path = chrome_executable().ok_or_else(|| {
            "Studio Pro 버전 목록을 읽으려면 Google Chrome 또는 Chromium이 필요합니다. MENDIMARU_CHROME_PATH로 직접 지정할 수도 있습니다."
                .to_string()
        })?;
        let profile_directory = next_profile_directory();
        let browser_config = BrowserConfig::builder()
            .chrome_executable(&chrome_path)
            .user_data_dir(&profile_directory)
            .args(vec![
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-gpu",
                "--disable-extensions",
                "--disable-background-timer-throttling",
                "--disable-default-apps",
                "--disable-sync",
                "--no-first-run",
                "--no-default-browser-check",
                "--headless=new",
            ])
            .build()
            .map_err(|error| format!("Marketplace 브라우저 설정을 만들 수 없습니다: {error}"))?;
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|error| format!("Marketplace 브라우저를 시작할 수 없습니다: {error}"))?;
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    continue;
                }
            }
        });
        Ok(Self {
            browser,
            handler_task,
            profile_directory,
        })
    }

    async fn navigate(&self, url: &str) -> Result<Page, String> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|error| format!("Marketplace 페이지를 만들 수 없습니다: {error}"))?;
        timeout(NAVIGATION_TIMEOUT, page.goto(url))
            .await
            .map_err(|_| "Mendix Marketplace 연결 시간이 초과되었습니다.".to_string())?
            .map_err(|error| format!("Mendix Marketplace를 열 수 없습니다: {error}"))?;
        Ok(page)
    }

    async fn cleanup(mut self) {
        let shutdown_timeout = Duration::from_secs(5);
        if timeout(shutdown_timeout, self.browser.close())
            .await
            .is_err()
        {
            let _ = self.browser.kill().await;
        }
        let _ = timeout(shutdown_timeout, self.browser.wait()).await;
        self.handler_task.abort();
        for _ in 0..4 {
            match tokio::fs::remove_dir_all(&self.profile_directory).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_GRID: &str = r##"
      <div class="widget-datagrid-grid-body table-content" role="rowgroup">
        <div class="tr" role="row">
          <div role="gridcell"><div><div>
            <a href="#"> 11.13.0 </a><span>Latest</span>
          </div></div></div>
          <div role="gridcell"><span>July 28, 2026</span></div>
          <div role="gridcell"><a href="https://docs.mendix.com/releasenotes/studio-pro/11.13/#11130">Release Notes</a></div>
        </div>
        <div class="tr" role="row">
          <div role="gridcell"><div><div>
            <a href="#">11.12.2</a><span>LTS</span>
          </div></div></div>
          <div role="gridcell"><span>July 27, 2026</span></div>
          <div role="gridcell"><a href="https://docs.mendix.com/releasenotes/studio-pro/11.12/#11122">Release Notes</a></div>
        </div>
      </div>
    "##;

    #[test]
    fn parses_current_marketplace_datagrid_shape() {
        let versions = parse_datagrid_html(SAMPLE_GRID).expect("parse versions");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, "11.13.0");
        assert!(versions[0].is_latest);
        assert_eq!(versions[0].release_date.as_deref(), Some("July 28, 2026"));
        assert_eq!(versions[1].version, "11.12.2");
        assert!(versions[1].is_lts);
    }

    #[test]
    fn constructs_reference_install_urls() {
        assert_eq!(
            v11_installer_url("11.12.2"),
            "https://artifacts.rnd.mendix.com/modelers/Mendix-11.12.2-Setup.exe"
        );
        assert_eq!(
            legacy_installer_url("10.24.22", "113362"),
            "https://artifacts.rnd.mendix.com/modelers/Mendix-10.24.22.113362-Setup.exe"
        );
    }

    #[test]
    fn orders_numeric_versions_newest_first() {
        assert_eq!(compare_versions_desc("11.13.0", "11.9.1"), Ordering::Less);
        assert_eq!(
            compare_versions_desc("10.24.22", "11.0.0"),
            Ordering::Greater
        );
    }

    #[tokio::test]
    #[ignore = "uses the installed Chrome against the live Mendix Marketplace"]
    async fn live_scrapes_the_first_catalog_page() {
        let _guard = SCRAPE_LOCK.lock().await;
        let (versions, total) = scrape_page(1).await.expect("live catalog page");
        assert_eq!(versions.len(), 10);
        assert!(total.is_some_and(|count| count >= 10));
    }

    #[tokio::test]
    #[ignore = "uses the installed Chrome against the live Mendix Marketplace"]
    async fn live_scrapes_the_second_catalog_page() {
        let _guard = SCRAPE_LOCK.lock().await;
        let (versions, total) = scrape_page(2).await.expect("live second catalog page");
        assert_eq!(versions.len(), 10);
        assert!(total.is_some_and(|count| count >= 20));
    }

    #[tokio::test]
    #[ignore = "uses the installed Chrome against the live Mendix Marketplace"]
    async fn live_extracts_reference_legacy_build_number() {
        let _guard = SCRAPE_LOCK.lock().await;
        assert_eq!(
            scrape_build_number("10.24.22").await.expect("build number"),
            "113362"
        );
    }
}
