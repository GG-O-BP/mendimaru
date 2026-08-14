use crate::config;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::timeout;

const PROFILE_PREFIX: &str = "mendimaru-marketplace-";
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(60);
static PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) struct BrowserSession {
    browser: Browser,
    handler_task: tokio::task::JoinHandle<()>,
    profile_directory: PathBuf,
}

impl BrowserSession {
    pub(super) async fn new() -> Result<Self, String> {
        let chrome_path =
            chrome_executable().ok_or_else(|| crate::tr!("error-browser-required"))?;
        let profile_directory = next_profile_directory();
        let browser_config = BrowserConfig::builder()
            .chrome_executable(&chrome_path)
            .user_data_dir(&profile_directory)
            .args(browser_arguments())
            .build()
            .map_err(|error| crate::tr!("error-browser-config", error = error))?;
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|error| crate::tr!("error-browser-start", error = error))?;
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

    pub(super) async fn navigate(&self, url: &str) -> Result<Page, String> {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|error| crate::tr!("error-browser-page", error = error))?;
        timeout(NAVIGATION_TIMEOUT, page.goto(url))
            .await
            .map_err(|_| crate::tr!("error-marketplace-connection-timeout"))?
            .map_err(|error| crate::tr!("error-marketplace-open", error = error))?;
        Ok(page)
    }

    pub(super) async fn cleanup(mut self) {
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

fn browser_arguments() -> Vec<&'static str> {
    vec![
        #[cfg(not(target_os = "windows"))]
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
    ]
}

fn chrome_executable() -> Option<String> {
    if let Ok(custom) = std::env::var("MENDIMARU_CHROME_PATH") {
        if Path::new(&custom).is_file() {
            return Some(custom);
        }
    }
    #[cfg(target_os = "windows")]
    {
        for candidate in windows_browser_candidates() {
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    config::find_binary(&[
        #[cfg(target_os = "windows")]
        "msedge",
        #[cfg(target_os = "windows")]
        "chrome",
        "google-chrome-stable",
        "google-chrome",
        "chromium",
        "chromium-browser",
    ])
}

#[cfg(target_os = "windows")]
fn windows_browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let root = PathBuf::from(program_files);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(root.join(r"Chromium\Application\chrome.exe"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        let root = PathBuf::from(program_files_x86);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local_app_data);
        candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(root.join(r"Chromium\Application\chrome.exe"));
    }
    candidates
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{browser_arguments, windows_browser_candidates};

    #[test]
    fn includes_edge_and_chrome_windows_install_locations() {
        let candidates = windows_browser_candidates();
        assert!(candidates.iter().any(|path| {
            path.to_string_lossy()
                .ends_with(r"Microsoft\Edge\Application\msedge.exe")
        }));
        assert!(candidates.iter().any(|path| {
            path.to_string_lossy()
                .ends_with(r"Google\Chrome\Application\chrome.exe")
        }));
    }

    #[test]
    fn keeps_the_chromium_sandbox_enabled_on_windows() {
        assert!(!browser_arguments().contains(&"--no-sandbox"));
    }
}

fn next_profile_directory() -> PathBuf {
    let sequence = PROFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{PROFILE_PREFIX}{}-{sequence}", std::process::id()))
}
