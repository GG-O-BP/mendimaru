mod page;
mod session;

use super::parser::parse_datagrid_html;
use crate::models::DownloadableVersion;
use page::{
    dismiss_privacy_modal, element_inner_html, find_build_number, navigate_to_page,
    read_total_count, wait_for_selector,
};
use session::BrowserSession;
use std::time::Duration;
use tokio::sync::Mutex;

const MARKETPLACE_URL: &str = "https://marketplace.mendix.com/link/studiopro";
const ELEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const DATAGRID_SELECTOR: &str = "div.widget-datagrid-content";
const DATAGRID_ROW_SELECTOR: &str =
    "div.widget-datagrid-content div.widget-datagrid-grid-body div.tr[role=row] a.mx-name-actionButton_VersionName1";
const BUILD_NUMBER_SELECTOR: &str = "span.mx-text.pds-heading--sm.pds-mb-0";

pub(super) static SCRAPE_LOCK: Mutex<()> = Mutex::const_new(());

pub(super) async fn scrape_page(
    target_page: u32,
) -> Result<(Vec<DownloadableVersion>, Option<u32>), String> {
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

pub(super) async fn scrape_build_number(version: &str) -> Result<String, String> {
    let session = BrowserSession::new().await?;
    let result = async {
        let page = session
            .navigate(&format!("{MARKETPLACE_URL}/{version}"))
            .await?;
        dismiss_privacy_modal(&page).await;
        find_build_number(&page, BUILD_NUMBER_SELECTOR, version, ELEMENT_TIMEOUT).await
    }
    .await;
    session.cleanup().await;
    result
}
