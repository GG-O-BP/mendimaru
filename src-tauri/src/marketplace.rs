mod browser;
mod cache;
mod parser;

use crate::models::StudioVersionCatalog;
use tauri::AppHandle;

use browser::SCRAPE_LOCK;
pub use cache::load_cached_catalog;
use parser::{compare_versions_desc, legacy_installer_url, major_version, v11_installer_url};

pub async fn fetch_catalog_page(
    app: &AppHandle,
    requested_page: u32,
    reset: bool,
) -> Result<StudioVersionCatalog, String> {
    let target_page = requested_page.max(1);
    let (fresh_versions, total_count) = {
        let _scrape_guard = SCRAPE_LOCK.lock().await;
        browser::scrape_page(target_page).await?
    };
    if fresh_versions.is_empty() {
        return Err(crate::tr!("error-marketplace-versions-empty"));
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
    cache::save_catalog(app, &catalog)?;
    Ok(catalog)
}

pub async fn installer_url(version: &str) -> Result<String, String> {
    if major_version(version)? >= 11 {
        return Ok(v11_installer_url(version));
    }
    let build_number = {
        let _scrape_guard = SCRAPE_LOCK.lock().await;
        browser::scrape_build_number(version).await?
    };
    Ok(legacy_installer_url(version, &build_number))
}

#[cfg(test)]
use browser::{scrape_build_number, scrape_page};
#[cfg(test)]
use parser::parse_datagrid_html;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

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
