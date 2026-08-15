mod archive;
mod process;
mod store;
mod supervisor;
mod toolchain;

use crate::contracts::{
    ArtifactDescriptor, ArtifactKind, BackendError, BackendErrorCode, BackendId, BackendResult,
    CapabilityId, RuntimeBuildRequest, RuntimeBuildResult, RuntimeLogBatch, RuntimeStartRequest,
    RuntimeStatus, CONTRACT_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use walkdir::{DirEntry, WalkDir};

const MAX_PROJECT_FILES: u64 = 250_000;
const MAX_PROJECT_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArtifactRecord {
    schema_version: String,
    descriptor: ArtifactDescriptor,
    project_key: String,
    build_key: String,
    role: ArtifactRole,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ArtifactRole {
    Package,
    Consistency,
    BuildLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildRecord {
    schema_version: String,
    project_key: String,
    build_key: String,
    project_digest: String,
    required_version: String,
    toolchain_version: String,
    toolchain_source: String,
    toolchain_sha256: String,
    java_home: String,
    java_executable: String,
    java_major: u32,
    created_at: DateTime<Utc>,
    success: bool,
    package_artifact: Option<ArtifactDescriptor>,
    consistency_artifact: ArtifactDescriptor,
    build_log_artifact: ArtifactDescriptor,
}

pub(crate) async fn build(
    request: &RuntimeBuildRequest,
    backend: BackendId,
) -> BackendResult<RuntimeBuildResult> {
    if !toolchain::portable_version_supported(&request.required_version) {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeVersionUnsupported,
            None,
            false,
        ));
    }
    let project_path = validate_project_path(&request.project_path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    let layout = store::RuntimeLayout::discover().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })?;
    let project_key = digest_bytes(project_path.to_string_lossy().as_bytes());
    let project_root = project_path.parent().ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    let project_root_for_digest = project_root.to_path_buf();
    let project_digest =
        tokio::task::spawn_blocking(move || project_tree_digest(&project_root_for_digest))
            .await
            .map_err(|_| {
                runtime_error(
                    backend,
                    CapabilityId::RuntimeBuild,
                    BackendErrorCode::OperationFailed,
                    None,
                    true,
                )
            })?
            .map_err(|_| {
                runtime_error(
                    backend,
                    CapabilityId::RuntimeBuild,
                    BackendErrorCode::PreconditionFailed,
                    None,
                    false,
                )
            })?;

    let toolchain = toolchain::ensure(&layout, &request.required_version, &project_path)
        .await
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::ToolchainUnavailable,
                None,
                true,
            )
        })?;
    let java = toolchain::resolve_java(&toolchain, &project_path)
        .await
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::ToolchainUnavailable,
                None,
                false,
            )
        })?;
    let build_key = digest_bytes(
        format!(
            "portable-app-package\0{}\0{}\0{}\0{}",
            request.required_version,
            project_digest,
            toolchain.archive_sha256,
            std::env::consts::OS
        )
        .as_bytes(),
    );
    let project_directory = layout.project_directory(&project_key).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    let lock =
        store::create_private_file(&project_directory.join("build.lock"), false).map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::OperationFailed,
                None,
                true,
            )
        })?;
    acquire_lock(&lock, backend).await?;
    let builds = layout.project_builds_directory(&project_key).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    if request.clean {
        clean_builds(&builds).map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::OperationFailed,
                None,
                true,
            )
        })?;
    }
    clean_interrupted_staging(&builds).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    let final_directory = builds.join(&build_key);
    if let Ok(record) = cached_build(
        &layout,
        &final_directory,
        &project_key,
        &build_key,
        &project_digest,
        &request.required_version,
        &toolchain.archive_sha256,
    ) {
        return build_result(request, &record, true);
    }
    remove_direct_directory_if_present(&final_directory).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;

    let staging = builds.join(format!(
        ".staging-{}",
        random_suffix().map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::OperationFailed,
                None,
                true,
            )
        })?
    ));
    store::ensure_private_directory(&staging).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    let package_path = staging.join("package.zip");
    let consistency_path = staging.join("consistency.json");
    let build_log_path = staging.join("build.log");
    let deno_directory = staging.join("deno");
    let temp_directory = staging.join("tmp");
    store::ensure_private_directory(&deno_directory).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    store::ensure_private_directory(&temp_directory).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    let exit_code = run_mxbuild(MxBuildInvocation {
        toolchain: &toolchain,
        java: &java,
        project_path: &project_path,
        package_path: &package_path,
        consistency_path: &consistency_path,
        log_path: &build_log_path,
        deno_directory: &deno_directory,
        temp_directory: &temp_directory,
    })
    .await
    .map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            true,
        )
    })?;
    ensure_consistency_report(&consistency_path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            false,
        )
    })?;
    let has_consistency_errors = consistency_has_errors(&consistency_path).unwrap_or(false);

    if exit_code == 0 && package_path.is_file() {
        let template = staging.join("deployment-template");
        let package_for_extract = package_path.clone();
        let template_for_extract = template.clone();
        tokio::task::spawn_blocking(move || {
            archive::extract_portable_package(&package_for_extract, &template_for_extract)
        })
        .await
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::RuntimeBuildFailed,
                None,
                false,
            )
        })?
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeBuild,
                BackendErrorCode::RuntimeBuildFailed,
                None,
                false,
            )
        })?;
    }

    let package_artifact = if package_path.is_file() {
        Some(create_artifact(
            &request.session_id,
            backend,
            ArtifactKind::RuntimePackage,
            &package_path,
            "portable-runtime-package",
        )?)
    } else {
        None
    };
    let consistency_artifact = create_artifact(
        &request.session_id,
        backend,
        ArtifactKind::ConsistencyReport,
        &consistency_path,
        "mxbuild-consistency",
    )?;
    let build_log_artifact = create_artifact(
        &request.session_id,
        backend,
        ArtifactKind::BuildLog,
        &build_log_path,
        "mxbuild-output",
    )?;
    let success = exit_code == 0 && package_artifact.is_some();
    let record = BuildRecord {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        project_key: project_key.clone(),
        build_key: build_key.clone(),
        project_digest,
        required_version: request.required_version.clone(),
        toolchain_version: toolchain.version.clone(),
        toolchain_source: toolchain.source_url.clone(),
        toolchain_sha256: toolchain.archive_sha256.clone(),
        java_home: java.home.to_string_lossy().to_string(),
        java_executable: java.executable.to_string_lossy().to_string(),
        java_major: java.major,
        created_at: Utc::now(),
        success,
        package_artifact: package_artifact.clone(),
        consistency_artifact: consistency_artifact.clone(),
        build_log_artifact: build_log_artifact.clone(),
    };
    store::write_json(&staging.join("build.json"), &record).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            Some(build_log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    fs::rename(&staging, &final_directory).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            Some(build_log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    register_artifact(
        &layout,
        &project_key,
        &build_key,
        &consistency_artifact,
        ArtifactRole::Consistency,
    )?;
    register_artifact(
        &layout,
        &project_key,
        &build_key,
        &build_log_artifact,
        ArtifactRole::BuildLog,
    )?;
    if let Some(package_artifact) = &package_artifact {
        register_artifact(
            &layout,
            &project_key,
            &build_key,
            package_artifact,
            ArtifactRole::Package,
        )?;
    }
    if !success {
        let (code, diagnostic) = if has_consistency_errors {
            (
                BackendErrorCode::ConsistencyFailed,
                consistency_artifact.artifact_id,
            )
        } else {
            (
                BackendErrorCode::RuntimeBuildFailed,
                build_log_artifact.artifact_id,
            )
        };
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            code,
            Some(diagnostic),
            false,
        ));
    }
    build_result(request, &record, false)
}

pub(crate) async fn start(
    request: &RuntimeStartRequest,
    backend: BackendId,
) -> BackendResult<RuntimeStatus> {
    supervisor::start(request, backend).await
}

pub(crate) async fn wait(session_id: &str, backend: BackendId) -> BackendResult<RuntimeStatus> {
    supervisor::wait(session_id, backend).await
}

pub(crate) async fn status(session_id: &str, backend: BackendId) -> BackendResult<RuntimeStatus> {
    supervisor::status(session_id, backend).await
}

pub(crate) async fn url(session_id: &str, backend: BackendId) -> BackendResult<String> {
    supervisor::url(session_id, backend).await
}

pub(crate) async fn stop(session_id: &str, backend: BackendId) -> BackendResult<()> {
    supervisor::stop(session_id, backend).await
}

pub(crate) async fn logs(
    session_id: &str,
    cursor: Option<&str>,
    backend: BackendId,
) -> BackendResult<RuntimeLogBatch> {
    supervisor::logs(session_id, cursor, backend).await
}

pub(crate) fn supervisor_dispatch(arguments: &[std::ffi::OsString]) -> i32 {
    supervisor::dispatch(arguments)
}

fn build_result(
    request: &RuntimeBuildRequest,
    record: &BuildRecord,
    cache_hit: bool,
) -> BackendResult<RuntimeBuildResult> {
    let package_artifact = record.package_artifact.clone().ok_or_else(|| {
        BackendError::operation(
            record.consistency_artifact.backend,
            CapabilityId::RuntimeBuild,
            "the cached portable package is incomplete",
        )
    })?;
    Ok(RuntimeBuildResult {
        session_id: request.session_id.clone(),
        package_artifact,
        consistency_artifact: record.consistency_artifact.clone(),
        build_log_artifact: record.build_log_artifact.clone(),
        required_version: record.required_version.clone(),
        toolchain_version: record.toolchain_version.clone(),
        cache_hit,
        capability_basis: toolchain::capability_basis().to_string(),
    })
}

fn cached_build(
    layout: &store::RuntimeLayout,
    directory: &Path,
    project_key: &str,
    build_key: &str,
    project_digest: &str,
    version: &str,
    toolchain_sha256: &str,
) -> Result<BuildRecord, String> {
    ensure_direct_directory(directory)?;
    let record: BuildRecord = store::read_json(&directory.join("build.json"))?;
    if record.schema_version != CONTRACT_SCHEMA_VERSION
        || !record.success
        || record.project_key != project_key
        || record.build_key != build_key
        || record.project_digest != project_digest
        || record.required_version != version
        || record.toolchain_version != version
        || record.toolchain_sha256 != toolchain_sha256
    {
        return Err("the cached portable build does not match the request".to_string());
    }
    let package = record
        .package_artifact
        .as_ref()
        .ok_or_else(|| "the cached portable build has no package".to_string())?;
    let package_path = directory.join("package.zip");
    verify_artifact_file(package, &package_path)?;
    verify_artifact_file(
        &record.consistency_artifact,
        &directory.join("consistency.json"),
    )?;
    verify_artifact_file(&record.build_log_artifact, &directory.join("build.log"))?;
    ensure_direct_directory(&directory.join("deployment-template"))?;
    let artifact: ArtifactRecord =
        store::read_json(&layout.artifact_record(&package.artifact_id)?)?;
    if artifact.schema_version != CONTRACT_SCHEMA_VERSION
        || artifact.descriptor.schema_version != CONTRACT_SCHEMA_VERSION
        || record.consistency_artifact.schema_version != CONTRACT_SCHEMA_VERSION
        || record.build_log_artifact.schema_version != CONTRACT_SCHEMA_VERSION
        || artifact.role != ArtifactRole::Package
        || artifact.project_key != project_key
        || artifact.build_key != build_key
    {
        return Err("the cached package artifact registry entry is invalid".to_string());
    }
    Ok(record)
}

fn register_artifact(
    layout: &store::RuntimeLayout,
    project_key: &str,
    build_key: &str,
    descriptor: &ArtifactDescriptor,
    role: ArtifactRole,
) -> BackendResult<()> {
    let record = ArtifactRecord {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        descriptor: descriptor.clone(),
        project_key: project_key.to_string(),
        build_key: build_key.to_string(),
        role,
    };
    store::write_json(
        &layout
            .artifact_record(&descriptor.artifact_id)
            .map_err(|_| {
                runtime_error(
                    descriptor.backend,
                    CapabilityId::RuntimeBuild,
                    BackendErrorCode::RuntimeBuildFailed,
                    None,
                    true,
                )
            })?,
        &record,
    )
    .map_err(|_| {
        runtime_error(
            descriptor.backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            true,
        )
    })
}

fn create_artifact(
    session_id: &str,
    backend: BackendId,
    kind: ArtifactKind,
    path: &Path,
    diagnostic: &str,
) -> BackendResult<ArtifactDescriptor> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            false,
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_DIAGNOSTIC_BYTES && kind != ArtifactKind::RuntimePackage
    {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            false,
        ));
    }
    let mut descriptor = ArtifactDescriptor::create(session_id, backend, kind)?;
    descriptor.media_type = Some(
        match kind {
            ArtifactKind::RuntimePackage => "application/zip",
            ArtifactKind::ConsistencyReport => "application/json",
            _ => "text/plain; charset=utf-8",
        }
        .to_string(),
    );
    descriptor.sha256 = Some(sha256_file(path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::RuntimeBuildFailed,
            None,
            true,
        )
    })?);
    descriptor.size_bytes = Some(metadata.len());
    descriptor.location = Some(format!("mendimaru-cache://{}", descriptor.artifact_id));
    descriptor.backend_diagnostic_ref = Some(diagnostic.to_string());
    Ok(descriptor)
}

struct MxBuildInvocation<'a> {
    toolchain: &'a toolchain::Toolchain,
    java: &'a toolchain::JavaRuntime,
    project_path: &'a Path,
    package_path: &'a Path,
    consistency_path: &'a Path,
    log_path: &'a Path,
    deno_directory: &'a Path,
    temp_directory: &'a Path,
}

async fn run_mxbuild(invocation: MxBuildInvocation<'_>) -> Result<i32, String> {
    let MxBuildInvocation {
        toolchain,
        java,
        project_path,
        package_path,
        consistency_path,
        log_path,
        deno_directory,
        temp_directory,
    } = invocation;
    let mut log = store::create_private_file(log_path, true)?;
    writeln!(
        log,
        "mendimaru portable build: version={}, java={}, target=portable-app-package",
        toolchain.version, java.major
    )
    .map_err(|error| format!("could not initialize the build log: {error}"))?;
    let stdout = log
        .try_clone()
        .map_err(|error| format!("could not clone the build log: {error}"))?;
    let stderr = log
        .try_clone()
        .map_err(|error| format!("could not clone the build log: {error}"))?;
    let mut command = tokio::process::Command::new(&toolchain.mxbuild);
    let gradle_directory = temp_directory.join("gradle-home");
    store::ensure_private_directory(&gradle_directory)?;
    command
        .arg(format!("--java-home={}", java.home.display()))
        .arg(format!("--java-exe-path={}", java.executable.display()))
        .arg("--target=portable-app-package")
        .arg(format!("--write-errors={}", consistency_path.display()))
        .arg(format!("--output={}", package_path.display()))
        .arg(project_path)
        .current_dir(
            project_path
                .parent()
                .ok_or_else(|| "the project path has no parent".to_string())?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true)
        .env_clear()
        .env("JAVA_HOME", &java.home)
        .env("DENO_DIR", deno_directory)
        .env("GRADLE_USER_HOME", &gradle_directory)
        .env(
            "GRADLE_OPTS",
            "-Dorg.gradle.daemon=false -Dorg.gradle.vfs.watch=false",
        )
        .env("TMPDIR", temp_directory)
        .env("TMP", temp_directory)
        .env("TEMP", temp_directory);
    for name in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "COMSPEC",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    process::configure_runtime_child(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not execute exact-version MxBuild: {error}"))?;
    let child_pid = child
        .id()
        .ok_or_else(|| "the exact-version MxBuild process has no identifier".to_string())?;
    let _containment = process::RuntimeContainment::attach(child_pid)?;
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not wait for exact-version MxBuild: {error}"))?;
    log.sync_all()
        .map_err(|error| format!("could not persist the build log: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

fn ensure_consistency_report(path: &Path) -> Result<(), String> {
    if !path.exists() {
        store::write_json(path, &serde_json::json!({ "problems": [] }))?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect the consistency report: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_DIAGNOSTIC_BYTES
    {
        return Err("the consistency report is unsafe".to_string());
    }
    let value: serde_json::Value = store::read_json_bounded(path, MAX_DIAGNOSTIC_BYTES)?;
    if !value
        .get("problems")
        .is_some_and(serde_json::Value::is_array)
    {
        return Err("MxBuild returned an invalid consistency report".to_string());
    }
    Ok(())
}

fn consistency_has_errors(path: &Path) -> Result<bool, String> {
    let value: serde_json::Value = store::read_json_bounded(path, MAX_DIAGNOSTIC_BYTES)?;
    Ok(value["problems"].as_array().is_some_and(|problems| {
        problems.iter().any(|problem| {
            problem["severity"]
                .as_str()
                .is_some_and(|severity| severity.eq_ignore_ascii_case("error"))
        })
    }))
}

fn validate_project_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("mpr"))
    {
        return Err("the project path is invalid".to_string());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect the project: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the project must be a direct regular file".to_string());
    }
    fs::canonicalize(path).map_err(|error| format!("could not resolve the project: {error}"))
}

fn project_tree_digest(root: &Path) -> Result<String, String> {
    let mut entries = WalkDir::new(root)
        .max_depth(32)
        .follow_links(false)
        .into_iter()
        .filter_entry(project_entry_allowed)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not scan the project: {error}"))?;
    entries.retain(|entry| entry.file_type().is_file());
    entries.sort_by(|left, right| left.path().cmp(right.path()));
    if entries.len() as u64 > MAX_PROJECT_FILES {
        return Err("the project contains too many files".to_string());
    }
    let mut total = 0_u64;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    for entry in entries {
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| "a project file escaped its root".to_string())?;
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("a project file has an unsafe path".to_string());
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect a project file: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("the project contains an unsafe file".to_string());
        }
        total = total
            .checked_add(metadata.len())
            .filter(|bytes| *bytes <= MAX_PROJECT_BYTES)
            .ok_or_else(|| "the project exceeds the hashing limit".to_string())?;
        digest.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        digest.update(metadata.len().to_le_bytes());
        let mut file = File::open(entry.path())
            .map_err(|error| format!("could not read a project file: {error}"))?;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| format!("could not hash a project file: {error}"))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn project_entry_allowed(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git"
            | ".hg"
            | ".svn"
            | ".mendimaru"
            | ".mendix-cache"
            | "deployment"
            | "node_modules"
            | "releases"
            | "target"
            | "theme-cache"
    ) && !entry.file_type().is_symlink()
}

fn clean_builds(builds: &Path) -> Result<(), String> {
    ensure_direct_directory(builds)?;
    for entry in fs::read_dir(builds)
        .map_err(|error| format!("could not inspect portable builds: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect a portable build: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect a portable build: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("the portable build cache contains a symbolic link".to_string());
        }
        if metadata.is_dir() {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("could not clean a portable build: {error}"))?;
        } else if metadata.is_file() {
            fs::remove_file(entry.path())
                .map_err(|error| format!("could not clean a portable build file: {error}"))?;
        } else {
            return Err("the portable build cache contains a special file".to_string());
        }
    }
    Ok(())
}

fn clean_interrupted_staging(builds: &Path) -> Result<(), String> {
    ensure_direct_directory(builds)?;
    for entry in fs::read_dir(builds)
        .map_err(|error| format!("could not inspect interrupted portable builds: {error}"))?
    {
        let entry = entry
            .map_err(|error| format!("could not inspect interrupted portable build: {error}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(".staging-") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect interrupted portable build: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("an interrupted portable build is not a direct directory".to_string());
        }
        fs::remove_dir_all(entry.path())
            .map_err(|error| format!("could not clean an interrupted portable build: {error}"))?;
    }
    Ok(())
}

fn remove_direct_directory_if_present(path: &Path) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the portable build target is not a direct directory".to_string());
    }
    fs::remove_dir_all(path)
        .map_err(|error| format!("could not remove an incomplete portable build: {error}"))
}

fn verify_artifact_file(descriptor: &ArtifactDescriptor, path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect a cached artifact: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || descriptor.size_bytes != Some(metadata.len())
        || descriptor.sha256.as_deref() != Some(&sha256_file(path)?)
    {
        return Err("a cached artifact failed integrity validation".to_string());
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("could not open an artifact for hashing: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash an artifact: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn acquire_lock(file: &File, backend: BackendId) -> BackendResult<()> {
    use fs2::FileExt;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(_) => {
                return Err(runtime_error(
                    backend,
                    CapabilityId::RuntimeBuild,
                    BackendErrorCode::OperationFailed,
                    None,
                    true,
                ))
            }
        }
    }
}

fn ensure_direct_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect a runtime directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the runtime directory is not direct".to_string());
    }
    Ok(())
}

fn random_suffix() -> Result<String, String> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not generate a build staging nonce: {error}"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn runtime_error(
    backend: BackendId,
    capability: CapabilityId,
    code: BackendErrorCode,
    diagnostic_ref: Option<String>,
    retryable: bool,
) -> BackendError {
    BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code,
        message: safe_runtime_error_message(code).to_string(),
        backend: Some(backend),
        capability: Some(capability),
        reason: None,
        retryable,
        diagnostic_ref,
    }
}

fn safe_runtime_error_message(code: BackendErrorCode) -> &'static str {
    match code {
        BackendErrorCode::ToolchainUnavailable => {
            "the exact-version MxBuild or required Java toolchain is unavailable"
        }
        BackendErrorCode::RuntimeVersionUnsupported => {
            "the exact project version does not support Portable Runtime on this host; Windows Studio Pro Run Locally is required"
        }
        BackendErrorCode::ConsistencyFailed => "the Mendix project has consistency errors",
        BackendErrorCode::RuntimeBuildFailed => "the Portable Runtime package build failed",
        BackendErrorCode::RuntimeInitializationFailed => {
            "the Portable Runtime process failed during initialization"
        }
        BackendErrorCode::RuntimeReadinessTimeout => {
            "the Portable Runtime did not become HTTP-ready before the timeout"
        }
        BackendErrorCode::RuntimeSessionNotFound => "the Portable Runtime session was not found",
        BackendErrorCode::RuntimeExited => "the Portable Runtime process exited unexpectedly",
        BackendErrorCode::RuntimeGuestOffline => "the WinBoat guest is offline",
        BackendErrorCode::RuntimePortConflict => "the WinBoat Runtime host port conflicts",
        BackendErrorCode::RuntimePortForwardingInvalid => {
            "the WinBoat Runtime port forwarding is invalid"
        }
        BackendErrorCode::RuntimeFirewallBlocked => {
            "the Windows firewall blocks the WinBoat Runtime port"
        }
        BackendErrorCode::RuntimeNotListening => {
            "the Mendix Runtime is not listening inside the WinBoat guest"
        }
        BackendErrorCode::RuntimeComposeRecoveryFailed => {
            "the original WinBoat Compose configuration could not be recovered"
        }
        BackendErrorCode::UnsupportedCapability => {
            "the selected backend does not support this runtime operation"
        }
        BackendErrorCode::BackendMismatch => "the selected backend does not match this host",
        BackendErrorCode::InvalidRequest => "the runtime request is invalid",
        BackendErrorCode::PreconditionFailed => "a runtime precondition was not satisfied",
        BackendErrorCode::OperationFailed => "the runtime operation could not be completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_digest_is_stable_and_ignores_generated_directories() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        fs::write(temporary.path().join("App.mpr"), b"model").expect("model");
        fs::create_dir(temporary.path().join("deployment")).expect("deployment");
        fs::write(temporary.path().join("deployment/generated"), b"one").expect("generated");
        let first = project_tree_digest(temporary.path()).expect("first digest");
        fs::write(temporary.path().join("deployment/generated"), b"two").expect("generated");
        let second = project_tree_digest(temporary.path()).expect("second digest");
        assert_eq!(first, second);
        fs::write(temporary.path().join("App.mpr"), b"changed").expect("model");
        assert_ne!(
            first,
            project_tree_digest(temporary.path()).expect("changed digest")
        );
    }

    #[test]
    fn runtime_errors_use_distinct_stable_codes_without_paths() {
        let error = runtime_error(
            BackendId::LinuxWinboat,
            CapabilityId::RuntimeBuild,
            BackendErrorCode::ConsistencyFailed,
            Some(format!("artifact_{}", "ab".repeat(16))),
            false,
        );
        assert_eq!(error.code, BackendErrorCode::ConsistencyFailed);
        assert!(!error.message.contains('/'));
        assert!(error
            .diagnostic_ref
            .as_deref()
            .is_some_and(|value| value.starts_with("artifact_")));
    }
}
