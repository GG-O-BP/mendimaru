mod page;
mod session;

use super::parser::parse_datagrid_html;
use crate::models::DownloadableVersion;
use page::{
    dismiss_privacy_modal, element_inner_html, find_build_number, navigate_to_page,
    read_total_count, verify_exact_version_page, wait_for_selector,
};
use session::BrowserSession;
use std::time::Duration;
use tokio::sync::Mutex;

const MARKETPLACE_URL: &str = "https://marketplace.mendix.com/link/studiopro";
#[cfg(any(feature = "e2e", test))]
const E2E_MARKETPLACE_URL: &str = "MENDIMARU_E2E_MARKETPLACE_URL";
const ELEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const DATAGRID_SELECTOR: &str = "div.widget-datagrid-content";
const DATAGRID_ROW_SELECTOR: &str =
    "div.widget-datagrid-content div.widget-datagrid-grid-body div.tr[role=row] a.mx-name-actionButton_VersionName1, \
     div.widget-datagrid-content div.widget-datagrid-grid-body div.tr[role=row] a.mx-name-pDSLink1";
const BUILD_NUMBER_SELECTOR: &str = "span.mx-text.pds-heading--sm.pds-mb-0";

pub(super) static SCRAPE_LOCK: Mutex<()> = Mutex::const_new(());

pub(crate) fn browser_executable() -> Option<String> {
    session::chrome_executable()
}

#[cfg(target_os = "linux")]
pub(crate) async fn browser_sandbox_available() -> bool {
    session::sandbox_available().await
}

fn finish_session<T>(
    operation: Result<T, String>,
    cleanup: Result<(), String>,
) -> Result<T, String> {
    match operation {
        Ok(value) => cleanup.map(|()| value),
        Err(error) => Err(error),
    }
}

pub(super) async fn scrape_page(
    target_page: u32,
) -> Result<(Vec<DownloadableVersion>, Option<u32>), String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let page = session.navigate(&marketplace_url()?).await?;
        dismiss_privacy_modal(&page).await;
        wait_for_selector(&page, DATAGRID_ROW_SELECTOR, ELEMENT_TIMEOUT).await?;
        navigate_to_page(&page, target_page).await?;
        let html = element_inner_html(&page, DATAGRID_SELECTOR).await?;
        let versions = parse_datagrid_html(&html)?;
        let total_count = read_total_count(&page).await;
        Ok((versions, total_count))
    }
    .await;
    let cleanup = session.cleanup().await;
    finish_session(result, cleanup)
}

pub(super) async fn scrape_build_number(version: &str) -> Result<String, String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let marketplace_url = marketplace_url()?;
        let page = session
            .navigate(&format!("{marketplace_url}/{version}"))
            .await?;
        dismiss_privacy_modal(&page).await;
        find_build_number(&page, BUILD_NUMBER_SELECTOR, version, ELEMENT_TIMEOUT).await
    }
    .await;
    let cleanup = session.cleanup().await;
    finish_session(result, cleanup)
}

pub(super) async fn verify_version_available(version: &str) -> Result<(), String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let marketplace_url = marketplace_url()?;
        let page = session
            .navigate(&format!("{marketplace_url}/{version}"))
            .await?;
        dismiss_privacy_modal(&page).await;
        verify_exact_version_page(&page, version, ELEMENT_TIMEOUT).await
    }
    .await;
    let cleanup = session.cleanup().await;
    finish_session(result, cleanup)
}

fn marketplace_url() -> Result<String, String> {
    #[cfg(feature = "e2e")]
    if let Some(value) = std::env::var_os(E2E_MARKETPLACE_URL) {
        return validate_e2e_marketplace_url(&value.to_string_lossy());
    }
    Ok(MARKETPLACE_URL.to_string())
}

#[cfg(any(feature = "e2e", test))]
fn validate_e2e_marketplace_url(value: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| format!("{E2E_MARKETPLACE_URL} must be an absolute loopback URL"))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if parsed.scheme() != "http"
        || !loopback
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!(
            "{E2E_MARKETPLACE_URL} must be an unauthenticated HTTP loopback URL"
        ));
    }
    Ok(value.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod e2e_url_tests {
    use super::*;

    #[test]
    fn accepts_only_an_uncredentialed_loopback_marketplace_override() {
        assert_eq!(
            validate_e2e_marketplace_url("http://127.0.0.1:49152/catalog/").expect("loopback URL"),
            "http://127.0.0.1:49152/catalog"
        );
        for invalid in [
            "https://127.0.0.1/catalog",
            "http://example.com/catalog",
            "http://user@127.0.0.1/catalog",
            "http://127.0.0.1/catalog?secret=value",
        ] {
            assert!(validate_e2e_marketplace_url(invalid).is_err(), "{invalid}");
        }
    }
}
