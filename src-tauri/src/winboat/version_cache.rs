use crate::models::{AppConfig, StudioVersion};
use sha2::{Digest, Sha256};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const INSTALLED_VERSION_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedVersions {
    source_identity: String,
    captured_at: Instant,
    versions: Vec<StudioVersion>,
}

static INSTALLED_VERSIONS: OnceLock<RwLock<Option<CachedVersions>>> = OnceLock::new();
static INSTALLED_VERSION_REFRESH: Mutex<()> = Mutex::const_new(());

pub(super) fn get(config: &AppConfig) -> Option<Vec<StudioVersion>> {
    let cache = INSTALLED_VERSIONS
        .get_or_init(|| RwLock::new(None))
        .read()
        .ok()?;
    let cached = cache.as_ref()?;
    (cached.source_identity == source_identity(config)
        && cached.captured_at.elapsed() <= INSTALLED_VERSION_CACHE_TTL)
        .then(|| cached.versions.clone())
}

pub(super) fn store(config: &AppConfig, versions: &[StudioVersion]) {
    let Ok(mut cache) = INSTALLED_VERSIONS.get_or_init(|| RwLock::new(None)).write() else {
        return;
    };
    *cache = Some(CachedVersions {
        source_identity: source_identity(config),
        captured_at: Instant::now(),
        versions: versions.to_vec(),
    });
}

pub(super) fn seed(config: &AppConfig, versions: &[StudioVersion]) {
    if versions.is_empty() {
        invalidate();
    } else {
        store(config, versions);
    }
}

pub(super) fn invalidate() {
    if let Ok(mut cache) = INSTALLED_VERSIONS.get_or_init(|| RwLock::new(None)).write() {
        *cache = None;
    }
}

pub(super) async fn refresh_guard() -> tokio::sync::MutexGuard<'static, ()> {
    INSTALLED_VERSION_REFRESH.lock().await
}

fn source_identity(config: &AppConfig) -> String {
    let mut digest = Sha256::new();
    for value in [
        config.container_runtime.as_str(),
        &config.container_name,
        &config.api_url,
        &config.mendix_install_root,
        &config.mendix_data_root,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::{get, invalidate, seed, store};
    use crate::models::{AppConfig, ContainerRuntime, StudioVersion};

    fn config(container: &str) -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "/tmp/compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: container.into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: "/tmp/workspace".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn version() -> StudioVersion {
        StudioVersion {
            version: "11.12.3".into(),
            display_name: "Studio Pro 11.12.3".into(),
            executable_path: r"C:\Program Files\Mendix\11.12.3\modeler\studiopro.exe".into(),
            install_root: r"C:\Program Files\Mendix\11.12.3".into(),
            source: "fixture".into(),
            removable: true,
        }
    }

    #[test]
    fn caches_only_the_matching_environment_and_can_be_invalidated() {
        invalidate();
        let first = config("WinBoat");
        store(&first, &[version()]);
        assert_eq!(get(&first).expect("cached versions")[0].version, "11.12.3");
        assert!(get(&config("OtherWinBoat")).is_none());
        seed(&first, &[]);
        assert!(get(&first).is_none());
        seed(&first, &[version()]);
        assert_eq!(get(&first).expect("seeded versions")[0].version, "11.12.3");
        invalidate();
        assert!(get(&first).is_none());
    }
}
