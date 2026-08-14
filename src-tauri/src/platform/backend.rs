use crate::contracts::{
    ArtifactDescriptor, BackendError, BackendId, BackendResult, BrowserTestRequest,
    BrowserTestSummary, Capability, CapabilityId, CapabilityLimitation, CapabilityManifest,
    PlatformId, RuntimeBuildRequest, RuntimeBuildResult, RuntimeLogBatch, RuntimeStartRequest,
    RuntimeStatus, StudioSessionStatus, UiActionRequest, UiAutomationCapabilities, UiElement,
    UiFindRequest, UiTree, UiWaitRequest, CONTRACT_SCHEMA_VERSION,
};
use crate::models::{AppConfig, StudioInstallProgress, StudioVersion};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = BackendResult<T>> + Send + 'a>>;
pub type ProgressCallback<'a> = &'a mut (dyn FnMut(StudioInstallProgress) + Send);

pub trait BackendIdentity: Send + Sync {
    fn backend_id(&self) -> BackendId;

    fn manifest(&self, architecture: &str) -> CapabilityManifest {
        manifest_for(self.backend_id(), architecture)
    }
}

pub trait StudioBackend: BackendIdentity {
    fn detect(&self) -> BackendFuture<'_, Vec<StudioVersion>> {
        unsupported(self.backend_id(), CapabilityId::StudioDetect)
    }

    fn install<'a>(
        &'a self,
        _version: &'a str,
        _installer_path: &'a Path,
        _expected_sha256: &'a str,
        _on_progress: ProgressCallback<'a>,
    ) -> BackendFuture<'a, String> {
        unsupported(self.backend_id(), CapabilityId::StudioInstall)
    }

    fn uninstall<'a>(&'a self, _version: &'a str) -> BackendFuture<'a, ()> {
        unsupported(self.backend_id(), CapabilityId::StudioUninstall)
    }

    fn start<'a>(
        &'a self,
        _version: &'a str,
        _project_mpr_path: Option<&'a str>,
    ) -> BackendFuture<'a, ()> {
        unsupported(self.backend_id(), CapabilityId::StudioStart)
    }

    fn status<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, StudioSessionStatus> {
        unsupported(self.backend_id(), CapabilityId::StudioStatus)
    }

    fn stop<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, ()> {
        unsupported(self.backend_id(), CapabilityId::StudioStop)
    }
}

pub trait RuntimeBackend: BackendIdentity {
    fn build<'a>(
        &'a self,
        _request: &'a RuntimeBuildRequest,
    ) -> BackendFuture<'a, RuntimeBuildResult> {
        unsupported(self.backend_id(), CapabilityId::RuntimeBuild)
    }

    fn start<'a>(&'a self, _request: &'a RuntimeStartRequest) -> BackendFuture<'a, RuntimeStatus> {
        unsupported(self.backend_id(), CapabilityId::RuntimeStart)
    }

    fn wait<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, RuntimeStatus> {
        unsupported(self.backend_id(), CapabilityId::RuntimeWait)
    }

    fn url<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, String> {
        unsupported(self.backend_id(), CapabilityId::RuntimeUrl)
    }

    fn stop<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, ()> {
        unsupported(self.backend_id(), CapabilityId::RuntimeStop)
    }

    fn logs<'a>(
        &'a self,
        _session_id: &'a str,
        _cursor: Option<&'a str>,
    ) -> BackendFuture<'a, RuntimeLogBatch> {
        unsupported(self.backend_id(), CapabilityId::RuntimeLogs)
    }
}

pub trait UiAutomationBackend: BackendIdentity {
    fn capabilities<'a>(
        &'a self,
        _session_id: &'a str,
    ) -> BackendFuture<'a, UiAutomationCapabilities> {
        unsupported(self.backend_id(), CapabilityId::UiCapabilities)
    }

    fn tree<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, UiTree> {
        unsupported(self.backend_id(), CapabilityId::UiTree)
    }

    fn find<'a>(&'a self, _request: &'a UiFindRequest) -> BackendFuture<'a, Vec<UiElement>> {
        unsupported(self.backend_id(), CapabilityId::UiFind)
    }

    fn action<'a>(&'a self, _request: &'a UiActionRequest) -> BackendFuture<'a, UiElement> {
        unsupported(self.backend_id(), CapabilityId::UiAction)
    }

    fn wait<'a>(&'a self, _request: &'a UiWaitRequest) -> BackendFuture<'a, UiElement> {
        unsupported(self.backend_id(), CapabilityId::UiWait)
    }

    fn screenshot<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, ArtifactDescriptor> {
        unsupported(self.backend_id(), CapabilityId::UiScreenshot)
    }
}

pub trait BrowserBackend: BackendIdentity {
    fn test<'a>(
        &'a self,
        _request: &'a BrowserTestRequest,
    ) -> BackendFuture<'a, BrowserTestSummary> {
        unsupported(self.backend_id(), CapabilityId::BrowserTest)
    }

    fn artifacts<'a>(&'a self, _session_id: &'a str) -> BackendFuture<'a, Vec<ArtifactDescriptor>> {
        unsupported(self.backend_id(), CapabilityId::BrowserArtifacts)
    }
}

pub trait PlatformBackend:
    StudioBackend + RuntimeBackend + UiAutomationBackend + BrowserBackend
{
}

fn unsupported<'a, T>(backend: BackendId, capability: CapabilityId) -> BackendFuture<'a, T>
where
    T: 'a,
{
    let reason = capability_for(backend, capability, std::env::consts::ARCH)
        .limitation
        .unwrap_or_else(|| CapabilityLimitation::not_implemented(capability));
    Box::pin(async move {
        Err(BackendError::unsupported_with_reason(
            backend, capability, reason,
        ))
    })
}

pub fn expected_backend(host: PlatformId) -> Option<BackendId> {
    match host {
        PlatformId::Linux => Some(BackendId::LinuxWinboat),
        PlatformId::Windows => Some(BackendId::WindowsNative),
        PlatformId::Macos => Some(BackendId::MacNative),
        PlatformId::Unsupported => None,
    }
}

pub fn select_backend_id(
    host: PlatformId,
    requested: Option<BackendId>,
) -> BackendResult<BackendId> {
    let expected = expected_backend(host);
    match (requested, expected) {
        (Some(requested), Some(expected)) if requested == expected => Ok(requested),
        (None, Some(expected)) => Ok(expected),
        (Some(requested), expected) => {
            Err(BackendError::backend_mismatch(requested, host, expected))
        }
        (None, None) => Err(BackendError::invalid_request(format!(
            "no Mendimaru backend is available for {host:?}"
        ))),
    }
}

pub fn manifest_for(backend: BackendId, architecture: &str) -> CapabilityManifest {
    let (host_platform, studio_platform) = match backend {
        BackendId::LinuxWinboat => (PlatformId::Linux, PlatformId::Windows),
        BackendId::WindowsNative => (PlatformId::Windows, PlatformId::Windows),
        BackendId::MacNative => (PlatformId::Macos, PlatformId::Macos),
    };
    let capabilities = CapabilityId::ALL
        .into_iter()
        .map(|id| capability_for(backend, id, architecture))
        .collect();
    CapabilityManifest {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        backend,
        host_platform,
        studio_platform,
        runtime_platform: None,
        runtime_mode: None,
        architecture: architecture.to_string(),
        capabilities,
    }
}

fn capability_for(backend: BackendId, id: CapabilityId, architecture: &str) -> Capability {
    let architecture_supported =
        backend != BackendId::WindowsNative || matches!(architecture, "x86_64" | "aarch64");
    if matches!(backend, BackendId::LinuxWinboat | BackendId::WindowsNative)
        && architecture_supported
        && matches!(
            id,
            CapabilityId::StudioDetect
                | CapabilityId::StudioInstall
                | CapabilityId::StudioUninstall
                | CapabilityId::StudioStart
        )
    {
        return Capability::supported(id, required_permissions(backend, id));
    }

    let mut limitation = CapabilityLimitation::not_implemented(id);
    if backend == BackendId::WindowsNative
        && !architecture_supported
        && matches!(
            id,
            CapabilityId::StudioDetect
                | CapabilityId::StudioInstall
                | CapabilityId::StudioUninstall
                | CapabilityId::StudioStart
        )
    {
        limitation.message =
            "the windows-native Studio adapter requires a supported 64-bit architecture"
                .to_string();
        limitation.required_version = Some("x86_64 or aarch64 Windows host".to_string());
    } else if backend == BackendId::MacNative
        && matches!(
            id,
            CapabilityId::StudioDetect
                | CapabilityId::StudioInstall
                | CapabilityId::StudioUninstall
                | CapabilityId::StudioStart
        )
    {
        limitation.message =
            "the mac-native Studio adapter is tracked by issue #11 and is not available yet"
                .to_string();
        limitation.required_version =
            Some("supported Apple Silicon macOS and a compatible Studio Pro release".to_string());
    }
    Capability::unsupported(id, limitation)
        .with_required_permissions(required_permissions(backend, id))
}

fn required_permissions(backend: BackendId, id: CapabilityId) -> &'static [&'static str] {
    match (backend, id) {
        (BackendId::LinuxWinboat, CapabilityId::StudioDetect) => &["winboat-guest-online"],
        (BackendId::LinuxWinboat, CapabilityId::StudioInstall)
        | (BackendId::LinuxWinboat, CapabilityId::StudioUninstall) => {
            &["winboat-interactive-session", "windows-administrator"]
        }
        (BackendId::LinuxWinboat, CapabilityId::StudioStart) => &["winboat-interactive-session"],
        (BackendId::WindowsNative, CapabilityId::StudioInstall)
        | (BackendId::WindowsNative, CapabilityId::StudioUninstall) => &["windows-uac-consent"],
        (BackendId::WindowsNative, CapabilityId::StudioStart) => &["interactive-desktop-session"],
        (BackendId::MacNative, CapabilityId::StudioInstall)
        | (BackendId::MacNative, CapabilityId::StudioUninstall) => {
            &["macos-administrator-approval"]
        }
        (BackendId::MacNative, CapabilityId::StudioStart) => &["interactive-aqua-session"],
        (BackendId::LinuxWinboat, CapabilityId::UiCapabilities)
        | (BackendId::LinuxWinboat, CapabilityId::UiTree)
        | (BackendId::LinuxWinboat, CapabilityId::UiFind)
        | (BackendId::LinuxWinboat, CapabilityId::UiAction)
        | (BackendId::LinuxWinboat, CapabilityId::UiWait)
        | (BackendId::LinuxWinboat, CapabilityId::UiScreenshot) => &["winboat-interactive-session"],
        (BackendId::WindowsNative, CapabilityId::UiCapabilities)
        | (BackendId::WindowsNative, CapabilityId::UiTree)
        | (BackendId::WindowsNative, CapabilityId::UiFind)
        | (BackendId::WindowsNative, CapabilityId::UiAction)
        | (BackendId::WindowsNative, CapabilityId::UiWait)
        | (BackendId::WindowsNative, CapabilityId::UiScreenshot) => {
            &["interactive-desktop-session"]
        }
        (BackendId::MacNative, CapabilityId::UiScreenshot) => {
            &["macos-accessibility", "macos-screen-recording"]
        }
        (BackendId::MacNative, CapabilityId::UiCapabilities)
        | (BackendId::MacNative, CapabilityId::UiTree)
        | (BackendId::MacNative, CapabilityId::UiFind)
        | (BackendId::MacNative, CapabilityId::UiAction)
        | (BackendId::MacNative, CapabilityId::UiWait) => &["macos-accessibility"],
        _ => &[],
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxWinboatBackend<'a> {
    config: &'a AppConfig,
}

#[cfg(target_os = "linux")]
impl BackendIdentity for LinuxWinboatBackend<'_> {
    fn backend_id(&self) -> BackendId {
        BackendId::LinuxWinboat
    }
}

#[cfg(target_os = "linux")]
impl StudioBackend for LinuxWinboatBackend<'_> {
    fn detect(&self) -> BackendFuture<'_, Vec<StudioVersion>> {
        Box::pin(async move {
            crate::winboat::installed_versions(self.config)
                .await
                .map_err(|error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioDetect, error)
                })
        })
    }

    fn install<'a>(
        &'a self,
        version: &'a str,
        installer_path: &'a Path,
        expected_sha256: &'a str,
        on_progress: ProgressCallback<'a>,
    ) -> BackendFuture<'a, String> {
        Box::pin(async move {
            let windows_installer_path = crate::projects::linux_path_to_windows_share(
                Path::new(&self.config.shared_directory),
                installer_path,
                &self.config.windows_shared_directory,
            )
            .map_err(|error| {
                BackendError::operation(self.backend_id(), CapabilityId::StudioInstall, error)
            })?;
            crate::winboat::install_studio(
                self.config,
                version,
                &windows_installer_path,
                expected_sha256,
                on_progress,
            )
            .await
            .map_err(|error| {
                BackendError::operation(self.backend_id(), CapabilityId::StudioInstall, error)
            })
        })
    }

    fn uninstall<'a>(&'a self, version: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            crate::winboat::launch_uninstaller(self.config, version)
                .await
                .map_err(|error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioUninstall, error)
                })
        })
    }

    fn start<'a>(
        &'a self,
        version: &'a str,
        project_mpr_path: Option<&'a str>,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            crate::winboat::launch_studio(self.config, version, project_mpr_path)
                .await
                .map_err(|error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioStart, error)
                })
        })
    }
}

#[cfg(target_os = "linux")]
impl RuntimeBackend for LinuxWinboatBackend<'_> {}
#[cfg(target_os = "linux")]
impl UiAutomationBackend for LinuxWinboatBackend<'_> {}
#[cfg(target_os = "linux")]
impl BrowserBackend for LinuxWinboatBackend<'_> {}
#[cfg(target_os = "linux")]
impl PlatformBackend for LinuxWinboatBackend<'_> {}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct WindowsNativeBackend<'a> {
    config: &'a AppConfig,
}

#[cfg(target_os = "windows")]
impl BackendIdentity for WindowsNativeBackend<'_> {
    fn backend_id(&self) -> BackendId {
        BackendId::WindowsNative
    }
}

#[cfg(target_os = "windows")]
impl StudioBackend for WindowsNativeBackend<'_> {
    fn detect(&self) -> BackendFuture<'_, Vec<StudioVersion>> {
        Box::pin(async move {
            super::windows_native::installed_versions(self.config).map_err(|error| {
                BackendError::operation(self.backend_id(), CapabilityId::StudioDetect, error)
            })
        })
    }

    fn install<'a>(
        &'a self,
        version: &'a str,
        installer_path: &'a Path,
        _expected_sha256: &'a str,
        on_progress: ProgressCallback<'a>,
    ) -> BackendFuture<'a, String> {
        Box::pin(async move {
            super::windows_native::install_studio(self.config, version, installer_path, on_progress)
                .await
                .map_err(|error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioInstall, error)
                })
        })
    }

    fn uninstall<'a>(&'a self, version: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            super::windows_native::uninstall_studio(self.config, version)
                .await
                .map_err(|error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioUninstall, error)
                })
        })
    }

    fn start<'a>(
        &'a self,
        version: &'a str,
        project_mpr_path: Option<&'a str>,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            super::windows_native::launch_studio(self.config, version, project_mpr_path).map_err(
                |error| {
                    BackendError::operation(self.backend_id(), CapabilityId::StudioStart, error)
                },
            )
        })
    }
}

#[cfg(target_os = "windows")]
impl RuntimeBackend for WindowsNativeBackend<'_> {}
#[cfg(target_os = "windows")]
impl UiAutomationBackend for WindowsNativeBackend<'_> {}
#[cfg(target_os = "windows")]
impl BrowserBackend for WindowsNativeBackend<'_> {}
#[cfg(target_os = "windows")]
impl PlatformBackend for WindowsNativeBackend<'_> {}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct MacNativeBackend;

#[cfg(target_os = "macos")]
impl BackendIdentity for MacNativeBackend {
    fn backend_id(&self) -> BackendId {
        BackendId::MacNative
    }
}

#[cfg(target_os = "macos")]
impl StudioBackend for MacNativeBackend {}
#[cfg(target_os = "macos")]
impl RuntimeBackend for MacNativeBackend {}
#[cfg(target_os = "macos")]
impl UiAutomationBackend for MacNativeBackend {}
#[cfg(target_os = "macos")]
impl BrowserBackend for MacNativeBackend {}
#[cfg(target_os = "macos")]
impl PlatformBackend for MacNativeBackend {}

pub(super) fn active_backend<'a>(
    config: &'a AppConfig,
    requested: Option<BackendId>,
) -> BackendResult<Box<dyn PlatformBackend + 'a>> {
    let selected = select_backend_id(PlatformId::current(), requested)?;
    match selected {
        BackendId::LinuxWinboat => {
            #[cfg(target_os = "linux")]
            {
                Ok(Box::new(LinuxWinboatBackend { config }))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(BackendError::backend_mismatch(
                    selected,
                    PlatformId::current(),
                    expected_backend(PlatformId::current()),
                ))
            }
        }
        BackendId::WindowsNative => {
            #[cfg(target_os = "windows")]
            {
                Ok(Box::new(WindowsNativeBackend { config }))
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(BackendError::backend_mismatch(
                    selected,
                    PlatformId::current(),
                    expected_backend(PlatformId::current()),
                ))
            }
        }
        BackendId::MacNative => {
            #[cfg(target_os = "macos")]
            {
                let _ = config;
                Ok(Box::new(MacNativeBackend))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(BackendError::backend_mismatch(
                    selected,
                    PlatformId::current(),
                    expected_backend(PlatformId::current()),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{BackendErrorCode, CapabilityStatus};
    use std::collections::BTreeSet;

    struct FakeBackend(BackendId);

    impl BackendIdentity for FakeBackend {
        fn backend_id(&self) -> BackendId {
            self.0
        }

        fn manifest(&self, architecture: &str) -> CapabilityManifest {
            let mut manifest = manifest_for(self.0, architecture);
            manifest.capabilities = CapabilityId::ALL
                .into_iter()
                .map(|id| Capability::unsupported(id, CapabilityLimitation::not_implemented(id)))
                .collect();
            manifest
        }
    }

    impl StudioBackend for FakeBackend {}
    impl RuntimeBackend for FakeBackend {}
    impl UiAutomationBackend for FakeBackend {}
    impl BrowserBackend for FakeBackend {}
    impl PlatformBackend for FakeBackend {}

    #[test]
    fn auto_selection_maps_each_host_to_exactly_one_backend() {
        assert_eq!(
            select_backend_id(PlatformId::Linux, None).expect("Linux selects"),
            BackendId::LinuxWinboat
        );
        assert_eq!(
            select_backend_id(PlatformId::Windows, None).expect("Windows selects"),
            BackendId::WindowsNative
        );
        assert_eq!(
            select_backend_id(PlatformId::Macos, None).expect("macOS selects"),
            BackendId::MacNative
        );
    }

    #[test]
    fn explicit_cross_platform_override_never_falls_back() {
        for host in [PlatformId::Linux, PlatformId::Windows, PlatformId::Macos] {
            let expected = expected_backend(host).expect("supported host has a backend");
            for requested in BackendId::ALL {
                let result = select_backend_id(host, Some(requested));
                if requested == expected {
                    assert_eq!(result.expect("matching backend selects"), expected);
                } else {
                    let error = result.expect_err("mismatched backend must fail");
                    assert_eq!(error.code, BackendErrorCode::BackendMismatch);
                    assert_eq!(error.backend, Some(requested));
                    assert!(!error.retryable);
                }
            }
        }
    }

    #[test]
    fn manifests_expose_every_action_and_no_implicit_fallback() {
        for backend in BackendId::ALL {
            let manifest = manifest_for(backend, "x86_64");
            let ids = manifest
                .capabilities
                .iter()
                .map(|entry| entry.id)
                .collect::<BTreeSet<_>>();
            assert_eq!(ids.len(), CapabilityId::ALL.len());
            assert!(CapabilityId::ALL.into_iter().all(|id| ids.contains(&id)));
            assert!(manifest
                .capabilities
                .iter()
                .all(|entry| !entry.fallback_allowed));
            assert!(manifest.capabilities.iter().all(|entry| {
                (entry.status == CapabilityStatus::Supported && entry.limitation.is_none())
                    || (entry.status == CapabilityStatus::Unsupported && entry.limitation.is_some())
            }));
        }
    }

    #[test]
    fn production_capabilities_match_the_implemented_studio_surface() {
        for backend in [BackendId::LinuxWinboat, BackendId::WindowsNative] {
            let manifest = manifest_for(backend, "x86_64");
            for capability in CapabilityId::ALL {
                let expected = matches!(
                    capability,
                    CapabilityId::StudioDetect
                        | CapabilityId::StudioInstall
                        | CapabilityId::StudioUninstall
                        | CapabilityId::StudioStart
                );
                assert_eq!(manifest.supports(capability), expected);
            }
        }
        let mac = manifest_for(BackendId::MacNative, "contract-test");
        assert!(CapabilityId::ALL
            .into_iter()
            .all(|capability| !mac.supports(capability)));

        let windows_32_bit = manifest_for(BackendId::WindowsNative, "x86");
        assert!(!windows_32_bit.supports(CapabilityId::StudioDetect));
        assert_eq!(
            windows_32_bit
                .capability(CapabilityId::StudioDetect)
                .and_then(|capability| capability.limitation.as_ref())
                .and_then(|limitation| limitation.required_version.as_deref()),
            Some("x86_64 or aarch64 Windows host")
        );
    }

    #[test]
    fn unsupported_invocation_reuses_discovered_limitation_and_permission() {
        let manifest = manifest_for(BackendId::MacNative, "aarch64");
        let capability = manifest
            .capability(CapabilityId::UiScreenshot)
            .expect("UI screenshot is declared");
        let error = tauri::async_runtime::block_on(unsupported::<()>(
            BackendId::MacNative,
            CapabilityId::UiScreenshot,
        ))
        .expect_err("UI screenshot is unsupported");
        assert_eq!(error.reason.as_deref(), capability.limitation.as_ref());
        assert_eq!(
            error
                .reason
                .as_ref()
                .and_then(|reason| reason.required_permission.as_deref()),
            Some("macos-accessibility")
        );
        assert_eq!(
            capability.required_permissions,
            ["macos-accessibility", "macos-screen-recording"]
        );
    }

    #[test]
    fn identical_contract_suite_runs_for_all_fake_platform_adapters() {
        for backend in BackendId::ALL {
            assert_fake_contract(&FakeBackend(backend));
        }
    }

    fn assert_fake_contract(backend: &dyn PlatformBackend) {
        let manifest = backend.manifest("fake");
        assert_eq!(manifest.schema_version, CONTRACT_SCHEMA_VERSION);
        assert_eq!(manifest.capabilities.len(), CapabilityId::ALL.len());
        let calls = [
            tauri::async_runtime::block_on(backend.status("session"))
                .expect_err("Studio status is unsupported"),
            tauri::async_runtime::block_on(backend.url("session"))
                .expect_err("Runtime URL is unsupported"),
            tauri::async_runtime::block_on(backend.capabilities("session"))
                .expect_err("UI capabilities are unsupported"),
            tauri::async_runtime::block_on(backend.artifacts("session"))
                .expect_err("Browser artifacts are unsupported"),
        ];
        let expected = [
            CapabilityId::StudioStatus,
            CapabilityId::RuntimeUrl,
            CapabilityId::UiCapabilities,
            CapabilityId::BrowserArtifacts,
        ];
        for (error, capability) in calls.into_iter().zip(expected) {
            assert_eq!(error.schema_version, CONTRACT_SCHEMA_VERSION);
            assert_eq!(error.code, BackendErrorCode::UnsupportedCapability);
            assert_eq!(error.backend, Some(backend.backend_id()));
            assert_eq!(error.capability, Some(capability));
            assert!(error.reason.is_some());
            assert!(!error.retryable);
        }
    }
}
