use chromiumoxide::page::Page;
use regex::Regex;
use std::time::{Duration, Instant};

const PAGE_SIZE: u32 = 10;
const PAGE_CHANGE_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const NEXT_PAGE_SELECTOR: &str = "button[aria-label='Go to next page']";
const PAGING_STATUS_SELECTOR: &str = "div.paging-status";
const VERSION_HEADING_SELECTOR: &str = "h1.pds-page-title--md";
const WINDOWS_INSTALLER_HEADING_SELECTOR: &str = "span.pds-heading--sm";

pub(super) async fn navigate_to_page(page: &Page, target_page: u32) -> Result<(), String> {
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

pub(super) async fn read_total_count(page: &Page) -> Option<u32> {
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

pub(super) async fn wait_for_any_selector(
    page: &Page,
    selectors: &[&str],
    wait_duration: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let mut found = false;
        for selector in selectors {
            if page
                .find_elements(*selector)
                .await
                .is_ok_and(|elements| !elements.is_empty())
            {
                found = true;
                break;
            }
        }
        if found {
            return Ok(());
        }
        if started.elapsed() >= wait_duration {
            return Err(crate::tr!(
                "error-marketplace-response-timeout",
                selector = selectors.first().copied().unwrap_or_default()
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

pub(super) async fn element_inner_html(page: &Page, value: &str) -> Result<String, String> {
    let element = page
        .find_element(value)
        .await
        .map_err(|error| crate::tr!("error-version-list-find", error = error))?;
    element
        .inner_html()
        .await
        .map_err(|error| crate::tr!("error-version-list-read", error = error))?
        .ok_or_else(|| crate::tr!("error-version-list-empty"))
}

pub(super) async fn dismiss_privacy_modal(page: &Page) {
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

pub(super) async fn find_build_number(
    page: &Page,
    selector: &str,
    version: &str,
    timeout: Duration,
) -> Result<String, String> {
    let expression = Regex::new(r"Build\s+(\d+)").expect("valid build number regex");
    let started = Instant::now();
    loop {
        if let Ok(elements) = page.find_elements(selector).await {
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
        if started.elapsed() >= timeout {
            return Err(crate::tr!("error-build-number-missing", version = version));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

pub(super) async fn verify_exact_version_page(
    page: &Page,
    version: &str,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let mut exact_heading = false;
        if let Ok(headings) = page.find_elements(VERSION_HEADING_SELECTOR).await {
            for heading in headings {
                if let Ok(Some(text)) = heading.inner_text().await {
                    if version_heading_matches(&text, version) {
                        exact_heading = true;
                        break;
                    }
                }
            }
        }
        if exact_heading {
            if let Ok(headings) = page.find_elements(WINDOWS_INSTALLER_HEADING_SELECTOR).await {
                for heading in headings {
                    if let Ok(Some(text)) = heading.inner_text().await {
                        if windows_installer_heading_matches(&text, version) {
                            return Ok(());
                        }
                    }
                }
            }
        }
        if started.elapsed() >= timeout {
            return Err(crate::tr!(
                "error-marketplace-version-missing",
                version = version
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn version_heading_matches(text: &str, version: &str) -> bool {
    normalized_text(text) == format!("Studio Pro {version}")
}

fn windows_installer_heading_matches(text: &str, version: &str) -> bool {
    normalized_text(text) == format!("Studio Pro {version} for Windows")
}

fn normalized_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
                    .map_err(|error| crate::tr!("error-marketplace-open", error = error))?;
                tokio::time::sleep(Duration::from_secs(2)).await;
                return Ok(());
            }
        }
        if started.elapsed() >= PAGE_CHANGE_TIMEOUT {
            return Err(crate::tr!("error-marketplace-next-page"));
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
            return Err(crate::tr!(
                "error-marketplace-page-position",
                position = crate::i18n::format_number(u64::from(expected_start)),
                status = &last_status
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{version_heading_matches, windows_installer_heading_matches};

    #[test]
    fn exact_version_heading_does_not_accept_a_catalog_or_other_release() {
        assert!(version_heading_matches(" Studio Pro\n11.12.2 ", "11.12.2"));
        assert!(!version_heading_matches("Download Studio Pro", "11.12.2"));
        assert!(!version_heading_matches("Studio Pro 11.12.20", "11.12.2"));
        assert!(windows_installer_heading_matches(
            "Studio Pro 11.12.2 for Windows ",
            "11.12.2"
        ));
        assert!(!windows_installer_heading_matches(
            "Studio Pro 11.12.2 for MacOS",
            "11.12.2"
        ));
    }
}
