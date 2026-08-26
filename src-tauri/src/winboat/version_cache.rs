use crate::models::{AppConfig, StudioVersion};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

const INSTALLED_VERSION_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedVersions {
    source_identity: String,
    captured_at: Instant,
    authoritative: bool,
    versions: Vec<StudioVersion>,
}

struct CompletedRefresh {
    source_identity: String,
    completed_at: Instant,
    result: Result<Vec<StudioVersion>, String>,
}

#[derive(Default)]
struct VersionCacheState {
    generation: u64,
    cached: Option<CachedVersions>,
    completed: Option<CompletedRefresh>,
}

static INSTALLED_VERSIONS: OnceLock<RwLock<VersionCacheState>> = OnceLock::new();
static INSTALLED_VERSION_REFRESH: AsyncMutex<()> = AsyncMutex::const_new(());

pub(super) fn get(config: &AppConfig) -> Option<Vec<StudioVersion>> {
    let source_identity = source_identity(config);
    let cache = state().read().unwrap_or_else(|error| error.into_inner());
    let cached = cache.cached.as_ref()?;
    (cached.source_identity == source_identity
        && cached.captured_at.elapsed() <= INSTALLED_VERSION_CACHE_TTL)
        .then(|| cached.versions.clone())
}

pub(super) fn seed(config: &AppConfig, versions: &[StudioVersion]) {
    let source_identity = source_identity(config);
    let mut cache = state().write().unwrap_or_else(|error| error.into_inner());
    if cache
        .cached
        .as_ref()
        .is_some_and(|cached| cached.source_identity == source_identity && cached.authoritative)
    {
        return;
    }
    cache.cached = (!versions.is_empty()).then(|| CachedVersions {
        source_identity,
        captured_at: Instant::now(),
        authoritative: false,
        versions: versions.to_vec(),
    });
}

pub(super) fn invalidate() {
    let mut cache = state().write().unwrap_or_else(|error| error.into_inner());
    cache.generation = cache.generation.wrapping_add(1);
    cache.cached = None;
    cache.completed = None;
}

pub(super) async fn refresh<F, Fut>(
    config: &AppConfig,
    mut detect: F,
) -> Result<Vec<StudioVersion>, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<StudioVersion>, String>>,
{
    let requested_at = Instant::now();
    let source_identity = source_identity(config);
    let waiting_started_at = Instant::now();
    let _refresh = INSTALLED_VERSION_REFRESH.lock().await;
    if let Some(result) = completed_since(&source_identity, requested_at) {
        drop(_refresh);
        trace_refresh(true, waiting_started_at.elapsed());
        return result;
    }

    loop {
        let generation = current_generation();
        let detection_started_at = Instant::now();
        let result = detect().await;
        if complete_refresh(&source_identity, generation, &result) {
            drop(_refresh);
            trace_refresh(false, detection_started_at.elapsed());
            return result;
        }
    }
}

fn completed_since(
    source_identity: &str,
    requested_at: Instant,
) -> Option<Result<Vec<StudioVersion>, String>> {
    let cache = state().read().unwrap_or_else(|error| error.into_inner());
    let completed = cache.completed.as_ref()?;
    (completed.source_identity == source_identity && completed.completed_at >= requested_at)
        .then(|| completed.result.clone())
}

fn current_generation() -> u64 {
    state()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .generation
}

fn complete_refresh(
    source_identity: &str,
    generation: u64,
    result: &Result<Vec<StudioVersion>, String>,
) -> bool {
    let mut cache = state().write().unwrap_or_else(|error| error.into_inner());
    if cache.generation != generation {
        return false;
    }
    let completed_at = Instant::now();
    cache.completed = Some(CompletedRefresh {
        source_identity: source_identity.to_string(),
        completed_at,
        result: result.clone(),
    });
    if let Ok(versions) = result {
        cache.cached = Some(CachedVersions {
            source_identity: source_identity.to_string(),
            captured_at: completed_at,
            authoritative: true,
            versions: versions.clone(),
        });
    }
    true
}

fn state() -> &'static RwLock<VersionCacheState> {
    INSTALLED_VERSIONS.get_or_init(|| RwLock::new(VersionCacheState::default()))
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

fn trace_refresh(coalesced: bool, duration: Duration) {
    if !crate::studio_trace::enabled() {
        return;
    }
    eprintln!(
        "[studio-overview] installed-refresh coalesced={coalesced} duration_ms={}",
        duration.as_millis()
    );
}

#[cfg(test)]
mod tests {
    use super::{get, invalidate, refresh, seed, state, INSTALLED_VERSION_CACHE_TTL};
    use crate::models::{AppConfig, ContainerRuntime, StudioVersion};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LazyLock};
    use std::time::{Duration, Instant};
    use tokio::sync::{Mutex, Notify};

    static TEST_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

    fn version(value: &str) -> StudioVersion {
        StudioVersion {
            version: value.into(),
            display_name: format!("Studio Pro {value}"),
            executable_path: format!(r"C:\Program Files\Mendix\{value}\modeler\studiopro.exe"),
            install_root: format!(r"C:\Program Files\Mendix\{value}"),
            source: "fixture".into(),
            removable: true,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn caches_only_the_matching_environment_and_can_be_invalidated() {
        let _serial = TEST_SERIAL.lock().await;
        invalidate();
        let first = config("WinBoat-seed");
        seed(&first, &[version("11.12.3")]);
        assert_eq!(get(&first).expect("cached versions")[0].version, "11.12.3");
        assert!(get(&config("OtherWinBoat-seed")).is_none());
        seed(&first, &[]);
        assert!(get(&first).is_none());
        seed(&first, &[version("11.12.3")]);
        invalidate();
        assert!(get(&first).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authoritative_results_resist_stale_seeds_and_expire_at_the_ttl() {
        let _serial = TEST_SERIAL.lock().await;
        invalidate();
        let config = config("WinBoat-ttl");
        refresh(&config, || async { Ok(vec![version("11.12.3")]) })
            .await
            .expect("authoritative refresh");
        seed(&config, &[version("10.24.24")]);
        assert_eq!(
            get(&config).expect("authoritative cache")[0].version,
            "11.12.3"
        );

        state()
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .cached
            .as_mut()
            .expect("cached versions")
            .captured_at = Instant::now() - INSTALLED_VERSION_CACHE_TTL - Duration::from_millis(1);
        assert!(get(&config).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn coalesces_concurrent_authoritative_successes_and_errors() {
        let _serial = TEST_SERIAL.lock().await;
        for should_fail in [false, true] {
            invalidate();
            let config = config(if should_fail {
                "WinBoat-error"
            } else {
                "WinBoat-success"
            });
            let calls = Arc::new(AtomicUsize::new(0));
            let release = Arc::new(Notify::new());
            let detect = || {
                let calls = Arc::clone(&calls);
                let release = Arc::clone(&release);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    release.notified().await;
                    if should_fail {
                        Err("guest apps unavailable".to_string())
                    } else {
                        Ok(vec![version("11.12.3")])
                    }
                }
            };
            let first = refresh(&config, detect);
            let second = refresh(&config, detect);
            let unblock = async {
                while calls.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                release.notify_one();
            };
            let (first, second, ()) = tokio::join!(first, second, unblock);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(first, second);
            assert_eq!(first.is_err(), should_fail);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retries_a_detection_invalidated_by_a_completed_mutation() {
        let _serial = TEST_SERIAL.lock().await;
        invalidate();
        let config = config("WinBoat-mutation");
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let detect = || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let release = Arc::clone(&release);
            async move {
                if call == 0 {
                    release.notified().await;
                    Ok(vec![version("10.24.24")])
                } else {
                    Ok(vec![version("11.12.3")])
                }
            }
        };
        let detection = refresh(&config, detect);
        let mutation = async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            invalidate();
            release.notify_one();
        };
        let (detected, ()) = tokio::join!(detection, mutation);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(detected.expect("refreshed versions")[0].version, "11.12.3");
        assert_eq!(
            get(&config).expect("authoritative cache")[0].version,
            "11.12.3"
        );
    }
}
