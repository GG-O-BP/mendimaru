mod browser;
mod cache;
mod parser;

use crate::models::{DownloadableVersion, StudioVersionCatalog};
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
    merge_versions(&mut catalog, fresh_versions);
    if !catalog.loaded_pages.contains(&target_page) {
        catalog.loaded_pages.push(target_page);
        catalog.loaded_pages.sort_unstable();
    }
    catalog.total_count = total_count.or(catalog.total_count);
    catalog.fetched_at = Some(chrono::Utc::now().to_rfc3339());
    cache::save_catalog(app, &catalog)?;
    Ok(catalog)
}

pub async fn resolve_downloadable_version(
    app: &AppHandle,
    version: &str,
) -> Result<DownloadableVersion, String> {
    crate::platform::validate_version(version)?;
    let mut catalog = load_cached_catalog(app).unwrap_or_default();
    if let Some(cached) = catalog
        .versions
        .iter()
        .find(|candidate| candidate.version == version)
    {
        return Ok(cached.clone());
    }

    // The version detail route is independent of the paginated catalog. A
    // matching detail-page heading proves that the exact Marketplace entry
    // exists; unlike constructing an artifact URL, this never treats arbitrary
    // input as an available release. Recent v11 pages no longer expose the
    // legacy numeric build marker, so existence and legacy URL resolution are
    // deliberately separate checks.
    {
        let _scrape_guard = SCRAPE_LOCK.lock().await;
        browser::verify_version_available(version).await?;
    }
    let resolved = direct_version_record(version);
    merge_versions(&mut catalog, vec![resolved.clone()]);
    catalog.fetched_at = Some(chrono::Utc::now().to_rfc3339());
    cache::save_catalog(app, &catalog)?;
    Ok(resolved)
}

fn direct_version_record(version: &str) -> DownloadableVersion {
    DownloadableVersion {
        version: version.to_string(),
        release_date: None,
        release_notes_url: None,
        is_lts: false,
        is_beta: version.to_ascii_lowercase().contains("beta"),
        is_mts: false,
        is_latest: false,
    }
}

fn merge_versions(catalog: &mut StudioVersionCatalog, fresh_versions: Vec<DownloadableVersion>) {
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

pub(crate) fn browser_executable() -> Option<String> {
    browser::browser_executable()
}
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
    fn direct_version_records_never_invent_support_labels() {
        let stable = direct_version_record("11.12.2");
        assert_eq!(stable.version, "11.12.2");
        assert!(!stable.is_latest && !stable.is_lts && !stable.is_mts && !stable.is_beta);
        let beta = direct_version_record("11.14.0-beta.1");
        assert!(beta.is_beta);
    }

    #[test]
    fn orders_numeric_versions_newest_first() {
        assert_eq!(compare_versions_desc("11.13.0", "11.9.1"), Ordering::Less);
        assert_eq!(
            compare_versions_desc("10.24.22", "11.0.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions_desc("11.6.0", "11.6.0-rc.2"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions_desc("11.6.0-rc.2", "11.6.0-beta.3"),
            Ordering::Less
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

    #[tokio::test]
    #[ignore = "uses the installed Chrome to resolve an exact version without catalog paging"]
    async fn live_resolves_an_exact_unloaded_version() {
        crate::i18n::initialize("en-US").expect("localization initializes");
        let _guard = SCRAPE_LOCK.lock().await;
        browser::verify_version_available("11.12.2")
            .await
            .expect("exact Marketplace version resolves");
    }

    #[tokio::test]
    #[ignore = "uses the installed Chrome to reject a nonexistent exact version"]
    async fn live_rejects_a_nonexistent_exact_version() {
        crate::i18n::initialize("en-US").expect("localization initializes");
        let _guard = SCRAPE_LOCK.lock().await;
        let error = browser::verify_version_available("99.99.99")
            .await
            .expect_err("a catalog landing page is not an exact version match");
        assert!(error.contains("99.99.99"));
    }
}
