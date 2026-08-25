use crate::app_paths::AppPaths;
use crate::contracts::CONTRACT_SCHEMA_VERSION;
use crate::models::{
    AppConfig, CommandError, OperationError, OperationKind, OperationRecord, OperationStage,
    OperationState,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

const HISTORY_FILE_NAME: &str = "operation-history.json";
const HISTORY_SCHEMA_VERSION: &str = "1.0.0";
const LEGACY_RECORD_SCHEMA_VERSIONS: &[&str] = &["1.0.0", "2.0.0", "3.0.0"];
const MAX_HISTORY_BYTES: u64 = 512 * 1024;
const MAX_RECORDS: usize = 250;
const MAX_IDENTIFIER_BYTES: usize = 160;
const MAX_REASON_BYTES: usize = 160;

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVE_OPERATIONS: OnceLock<Mutex<HashSet<ActiveOperation>>> = OnceLock::new();
static ACTIVE_SESSION_ACTIONS: OnceLock<Mutex<HashSet<ActiveSessionAction>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveOperation {
    history_path: PathBuf,
    id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveSessionAction {
    history_path: PathBuf,
    target_version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationHistory {
    schema_version: String,
    records: Vec<OperationRecord>,
    #[serde(default)]
    legacy_scan_complete: bool,
}

impl Default for OperationHistory {
    fn default() -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_VERSION.to_string(),
            records: Vec::new(),
            legacy_scan_complete: false,
        }
    }
}

pub(crate) struct OperationTracker {
    history_path: PathBuf,
    log_directory: PathBuf,
    id: String,
    finished: bool,
    last_stage: OperationStage,
    last_percentage: Option<f64>,
    last_estimated: bool,
}

pub(crate) struct SessionActionGuard {
    history_path: PathBuf,
    target_version: String,
}

impl OperationTracker {
    pub(crate) fn begin_with_paths(
        paths: &AppPaths,
        config: &AppConfig,
        kind: OperationKind,
        target_version: &str,
        protected_project: bool,
        stage: OperationStage,
        retry_of: Option<String>,
    ) -> Result<Self, String> {
        Self::begin_at(
            config,
            history_path(paths)?,
            kind,
            target_version,
            protected_project,
            stage,
            retry_of,
        )
    }

    fn begin_at(
        config: &AppConfig,
        history_path: PathBuf,
        kind: OperationKind,
        target_version: &str,
        protected_project: bool,
        stage: OperationStage,
        retry_of: Option<String>,
    ) -> Result<Self, String> {
        validate_target_version(target_version)?;
        let log_directory = operation_log_directory(config);
        let _store = lock_store()?;
        let mut history = load_history(&history_path)?;
        reconcile_and_import(config, &history_path, &mut history)?;
        if history.records.iter().any(|record| {
            record.state == OperationState::Running
                && record.target_version == target_version
                && operation_kinds_conflict(kind, record.kind)
        }) {
            return Err(format!(
                "another Studio Pro {target_version} operation is already running"
            ));
        }
        if is_mutation(kind)
            && active_session_actions()?.contains(&ActiveSessionAction {
                history_path: history_path.clone(),
                target_version: target_version.to_string(),
            })
        {
            return Err(format!(
                "a Studio Pro {target_version} session action is already running"
            ));
        }
        let id = operation_id(kind, target_version)?;
        let now = Utc::now();
        history.records.push(OperationRecord {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            id: id.clone(),
            kind,
            target_version: target_version.to_string(),
            protected_project,
            state: OperationState::Running,
            stage,
            percentage: None,
            estimated: false,
            started_at: now,
            updated_at: now,
            finished_at: None,
            error: None,
            retryable: false,
            log_available: is_direct_directory(&log_directory),
            retry_of,
        });
        trim_history(&mut history.records);
        save_history(&history_path, &history)?;
        active_operations()?.insert(ActiveOperation {
            history_path: history_path.clone(),
            id: id.clone(),
        });
        Ok(Self {
            history_path,
            log_directory,
            id,
            finished: false,
            last_stage: stage,
            last_percentage: None,
            last_estimated: false,
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn progress(
        &mut self,
        stage: OperationStage,
        percentage: Option<f64>,
        estimated: bool,
    ) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        let percentage = percentage.map(|value| value.clamp(0.0, 99.9));
        let significant_percentage = match (self.last_percentage, percentage) {
            (Some(previous), Some(current)) => (current - previous).abs() >= 1.0,
            (None, None) => false,
            _ => true,
        };
        if stage == self.last_stage && !significant_percentage && estimated == self.last_estimated {
            return Ok(());
        }
        self.update_record(|record| {
            record.stage = stage;
            record.percentage = percentage;
            record.estimated = estimated;
        })?;
        self.last_stage = stage;
        self.last_percentage = percentage;
        self.last_estimated = estimated;
        Ok(())
    }

    pub(crate) fn succeed(mut self) -> Result<(), String> {
        self.finish(
            OperationState::Succeeded,
            OperationStage::Completed,
            Some(100.0),
            None,
            false,
        )
    }

    pub(crate) fn cancel(self, reason: &str) -> Result<(), String> {
        self.cancel_with_code("download_cancelled", reason)
    }

    pub(crate) fn cancel_with_code(mut self, code: &str, reason: &str) -> Result<(), String> {
        self.finish(
            OperationState::Cancelled,
            self.last_stage,
            self.last_percentage,
            Some(OperationError {
                code: safe_reason(code),
                reason: safe_reason(reason),
                exit_code: None,
            }),
            true,
        )
    }

    pub(crate) fn fail(mut self, error: &CommandError) -> Result<(), String> {
        let (code, retryable) = failure_classification(error);
        let exit_code = error
            .details
            .as_ref()
            .and_then(|details| details.diagnostic_ref.as_deref())
            .and_then(|reference| reference.strip_prefix("windows-exit-code:"))
            .and_then(|code| code.parse::<i32>().ok());
        self.finish(
            OperationState::Failed,
            self.last_stage,
            self.last_percentage,
            Some(OperationError {
                code: code.to_string(),
                reason: safe_reason(code),
                exit_code,
            }),
            retryable,
        )
    }

    fn finish(
        &mut self,
        state: OperationState,
        stage: OperationStage,
        percentage: Option<f64>,
        error: Option<OperationError>,
        retryable: bool,
    ) -> Result<(), String> {
        let result = self.update_record(|record| {
            let now = Utc::now();
            record.state = state;
            record.stage = stage;
            record.percentage = percentage;
            record.estimated = false;
            record.updated_at = now;
            record.finished_at = Some(now);
            record.error = error;
            record.retryable = retryable && !record.protected_project;
        });
        self.finished = true;
        remove_active(&self.history_path, &self.id);
        result
    }

    fn update_record<F>(&self, update: F) -> Result<(), String>
    where
        F: FnOnce(&mut OperationRecord),
    {
        let _store = lock_store()?;
        let mut history = load_history(&self.history_path)?;
        let record = history
            .records
            .iter_mut()
            .find(|record| record.id == self.id)
            .ok_or_else(|| "the active operation record is missing".to_string())?;
        update(record);
        record.updated_at = Utc::now();
        record.log_available |= is_direct_directory(&self.log_directory);
        save_history(&self.history_path, &history)
    }
}

impl SessionActionGuard {
    pub(crate) fn begin_with_paths(
        paths: &AppPaths,
        config: &AppConfig,
        target_version: &str,
    ) -> Result<Self, String> {
        Self::begin_at(config, history_path(paths)?, target_version)
    }

    fn begin_at(
        config: &AppConfig,
        history_path: PathBuf,
        target_version: &str,
    ) -> Result<Self, String> {
        validate_target_version(target_version)?;
        let _store = lock_store()?;
        let mut history = load_history(&history_path)?;
        let changed = reconcile_and_import(config, &history_path, &mut history)?;
        if changed {
            trim_history(&mut history.records);
            save_history(&history_path, &history)?;
        }
        if history.records.iter().any(|record| {
            record.state == OperationState::Running
                && record.target_version == target_version
                && is_mutation(record.kind)
        }) {
            return Err(format!(
                "Studio Pro {target_version} is being installed or removed"
            ));
        }
        let action = ActiveSessionAction {
            history_path: history_path.clone(),
            target_version: target_version.to_string(),
        };
        if !active_session_actions()?.insert(action) {
            return Err(format!(
                "another Studio Pro {target_version} session action is already running"
            ));
        }
        Ok(Self {
            history_path,
            target_version: target_version.to_string(),
        })
    }
}

impl Drop for SessionActionGuard {
    fn drop(&mut self) {
        if let Ok(_store) = lock_store() {
            if let Ok(mut actions) = active_session_actions() {
                actions.remove(&ActiveSessionAction {
                    history_path: self.history_path.clone(),
                    target_version: self.target_version.clone(),
                });
            }
        }
    }
}

fn is_mutation(kind: OperationKind) -> bool {
    matches!(kind, OperationKind::Install | OperationKind::Uninstall)
}

fn operation_kinds_conflict(requested: OperationKind, active: OperationKind) -> bool {
    is_mutation(requested) || is_mutation(active)
}

impl Drop for OperationTracker {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.update_record(|record| {
            let now = Utc::now();
            record.state = OperationState::Interrupted;
            record.stage = OperationStage::Interrupted;
            record.finished_at = Some(now);
            record.updated_at = now;
            record.retryable = !record.protected_project;
            record.error = Some(OperationError {
                code: "operation_interrupted".to_string(),
                reason: "operation_interrupted".to_string(),
                exit_code: None,
            });
        });
        remove_active(&self.history_path, &self.id);
    }
}

pub(crate) fn list(app: &AppHandle, config: &AppConfig) -> Result<Vec<OperationRecord>, String> {
    list_with_paths(&AppPaths::from_app(app)?, config)
}

pub(crate) fn list_with_paths(
    paths: &AppPaths,
    config: &AppConfig,
) -> Result<Vec<OperationRecord>, String> {
    list_at(config, &history_path(paths)?)
}

fn list_at(config: &AppConfig, history_path: &Path) -> Result<Vec<OperationRecord>, String> {
    let _store = lock_store()?;
    let mut history = load_history(history_path)?;
    if reconcile_and_import(config, history_path, &mut history)? {
        trim_history(&mut history.records);
        save_history(history_path, &history)?;
    }
    history
        .records
        .sort_by_key(|record| std::cmp::Reverse(record.started_at));
    Ok(history.records)
}

pub(crate) fn retry_source_with_paths(
    paths: &AppPaths,
    config: &AppConfig,
    id: &str,
) -> Result<OperationRecord, String> {
    retry_source_at(config, &history_path(paths)?, id)
}

#[cfg(target_os = "linux")]
pub(crate) fn interrupt_completed_launch_with_paths(
    paths: &AppPaths,
    id: &str,
) -> Result<(), String> {
    interrupt_completed_launch_at(&history_path(paths)?, id)
}

#[cfg(any(target_os = "linux", test))]
fn interrupt_completed_launch_at(history_path: &Path, id: &str) -> Result<(), String> {
    validate_identifier(id)?;
    let _store = lock_store()?;
    let mut history = load_history(history_path)?;
    let record = history
        .records
        .iter_mut()
        .find(|record| record.id == id)
        .filter(|record| {
            record.kind == OperationKind::Launch && record.state == OperationState::Succeeded
        })
        .ok_or_else(|| "the completed launch operation could not be interrupted".to_string())?;
    let now = Utc::now();
    record.state = OperationState::Interrupted;
    record.stage = OperationStage::Interrupted;
    record.percentage = None;
    record.estimated = false;
    record.updated_at = now;
    record.finished_at = Some(now);
    record.error = Some(OperationError {
        code: "operation_interrupted".to_string(),
        reason: "operation_interrupted".to_string(),
        exit_code: None,
    });
    record.retryable = !record.protected_project;
    save_history(history_path, &history)
}

fn retry_source_at(
    config: &AppConfig,
    history_path: &Path,
    id: &str,
) -> Result<OperationRecord, String> {
    validate_identifier(id)?;
    list_at(config, history_path)?
        .into_iter()
        .find(|record| record.id == id)
        .filter(|record| record.state.is_terminal() && record.retryable)
        .ok_or_else(|| "the selected operation cannot be retried".to_string())
}

pub(crate) fn clear_completed(app: &AppHandle, config: &AppConfig) -> Result<usize, String> {
    clear_completed_with_paths(&AppPaths::from_app(app)?, config)
}

pub(crate) fn clear_completed_with_paths(
    paths: &AppPaths,
    config: &AppConfig,
) -> Result<usize, String> {
    clear_completed_at(config, &history_path(paths)?)
}

fn clear_completed_at(config: &AppConfig, history_path: &Path) -> Result<usize, String> {
    let _store = lock_store()?;
    let mut history = load_history(history_path)?;
    let reconciled = reconcile_and_import(config, history_path, &mut history)?;
    let previous = history.records.len();
    history.records.retain(|record| !record.state.is_terminal());
    let removed = previous - history.records.len();
    if removed > 0 || reconciled {
        save_history(history_path, &history)?;
    }
    Ok(removed)
}

pub(crate) fn log_directory(config: &AppConfig) -> Result<PathBuf, String> {
    existing_operation_log_directory(config)?
        .ok_or_else(|| "the operation report directory does not exist".to_string())
}

fn reconcile_and_import(
    config: &AppConfig,
    history_path: &Path,
    history: &mut OperationHistory,
) -> Result<bool, String> {
    let active = active_operations()?;
    let mut changed = false;
    let now = Utc::now();
    for record in &mut history.records {
        if record.state == OperationState::Running
            && !active.contains(&ActiveOperation {
                history_path: history_path.to_path_buf(),
                id: record.id.clone(),
            })
        {
            record.state = OperationState::Interrupted;
            record.stage = OperationStage::Interrupted;
            record.updated_at = now;
            record.finished_at = Some(now);
            record.retryable = !record.protected_project;
            record.error = Some(OperationError {
                code: "operation_interrupted".to_string(),
                reason: "operation_interrupted".to_string(),
                exit_code: None,
            });
            changed = true;
        }
    }
    drop(active);

    if history.legacy_scan_complete {
        return Ok(changed);
    }

    let known = history
        .records
        .iter()
        .map(|record| record.id.as_str())
        .collect::<HashSet<_>>();
    let Some(operation_directory) = existing_operation_log_directory(config)? else {
        return Ok(changed);
    };
    let entries = match fs::read_dir(&operation_directory) {
        Ok(entries) => entries,
        Err(error) => return Err(format!("could not inspect operation reports: {error}")),
    };
    let mut imported = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if known.contains(id) || !valid_legacy_id(id) {
            continue;
        }
        let Some((kind, version)) = legacy_target(id) else {
            continue;
        };
        let started_at = metadata
            .modified()
            .ok()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        imported.push(OperationRecord {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            id: id.to_string(),
            kind,
            target_version: version,
            protected_project: kind == OperationKind::Launch,
            state: OperationState::Interrupted,
            stage: OperationStage::Interrupted,
            percentage: None,
            estimated: false,
            started_at,
            updated_at: started_at,
            finished_at: Some(started_at),
            error: Some(OperationError {
                code: "legacy_report_untrusted".to_string(),
                reason: "legacy_report_untrusted".to_string(),
                exit_code: None,
            }),
            retryable: false,
            log_available: true,
            retry_of: None,
        });
    }
    history.legacy_scan_complete = true;
    changed = true;
    history.records.extend(imported);
    Ok(changed)
}

fn history_path(paths: &AppPaths) -> Result<PathBuf, String> {
    paths.ensure_config_directory()?;
    Ok(paths.config_directory().join(HISTORY_FILE_NAME))
}

fn operation_log_directory(config: &AppConfig) -> PathBuf {
    Path::new(&config.shared_directory).join(".mendimaru/operations")
}

fn existing_operation_log_directory(config: &AppConfig) -> Result<Option<PathBuf>, String> {
    let directory = operation_log_directory(config);
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect operation reports: {error}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the operation report directory must be a direct directory".to_string());
    }
    let shared = Path::new(&config.shared_directory)
        .canonicalize()
        .map_err(|error| format!("could not resolve the shared directory: {error}"))?;
    let canonical = directory
        .canonicalize()
        .map_err(|error| format!("could not resolve operation reports: {error}"))?;
    if !canonical.starts_with(shared) {
        return Err("the operation report directory escapes the shared root".to_string());
    }
    Ok(Some(directory))
}

fn is_direct_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn load_history(path: &Path) -> Result<OperationHistory, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperationHistory::default())
        }
        Err(error) => return Err(format!("could not inspect operation history: {error}")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("operation history must be a regular file".to_string());
    }
    if metadata.len() > MAX_HISTORY_BYTES {
        return Err("operation history exceeds the safe size limit".to_string());
    }
    let file =
        File::open(path).map_err(|error| format!("could not open operation history: {error}"))?;
    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_HISTORY_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|error| format!("could not read operation history: {error}"))?;
    if content.len() as u64 > MAX_HISTORY_BYTES {
        return Err("operation history exceeds the safe size limit".to_string());
    }
    let mut history: OperationHistory = serde_json::from_slice(&content)
        .map_err(|error| format!("operation history is invalid: {error}"))?;
    validate_history(&history)?;
    migrate_record_schema_versions(&mut history);
    Ok(history)
}

fn validate_history(history: &OperationHistory) -> Result<(), String> {
    if history.schema_version != HISTORY_SCHEMA_VERSION {
        return Err("operation history uses an unsupported schema".to_string());
    }
    if history.records.len() > MAX_RECORDS {
        return Err("operation history contains too many records".to_string());
    }
    let mut ids = HashSet::new();
    for record in &history.records {
        validate_identifier(&record.id)?;
        validate_target_version(&record.target_version)?;
        if !compatible_record_schema_version(&record.schema_version) || !ids.insert(&record.id) {
            return Err("operation history contains an invalid record".to_string());
        }
        if let Some(retry_of) = &record.retry_of {
            validate_identifier(retry_of)?;
            if retry_of == &record.id {
                return Err("operation history contains an invalid retry reference".to_string());
            }
        }
        if record
            .percentage
            .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
        {
            return Err("operation history contains invalid progress".to_string());
        }
        if let Some(error) = &record.error {
            validate_short_text(&error.code)?;
            validate_short_text(&error.reason)?;
        }
        validate_record_semantics(record)?;
    }
    Ok(())
}

fn compatible_record_schema_version(version: &str) -> bool {
    version == CONTRACT_SCHEMA_VERSION || LEGACY_RECORD_SCHEMA_VERSIONS.contains(&version)
}

fn migrate_record_schema_versions(history: &mut OperationHistory) {
    for record in &mut history.records {
        record.schema_version = CONTRACT_SCHEMA_VERSION.to_string();
    }
}

fn validate_record_semantics(record: &OperationRecord) -> Result<(), String> {
    if record.protected_project && record.kind != OperationKind::Launch {
        return Err("operation history contains an invalid protected operation".to_string());
    }
    if record.protected_project && record.retryable {
        return Err("operation history contains an unsafe retry marker".to_string());
    }
    match record.state {
        OperationState::Running => {
            if record.finished_at.is_some() || record.error.is_some() || record.retryable {
                return Err("operation history contains an invalid running record".to_string());
            }
        }
        OperationState::Succeeded => {
            if record.finished_at.is_none()
                || record.error.is_some()
                || record.retryable
                || record.stage != OperationStage::Completed
                || record.percentage != Some(100.0)
                || record.estimated
            {
                return Err("operation history contains an invalid successful record".to_string());
            }
        }
        OperationState::Failed | OperationState::Cancelled | OperationState::Interrupted => {
            if record.finished_at.is_none() || record.error.is_none() || record.estimated {
                return Err("operation history contains an invalid terminal record".to_string());
            }
        }
    }
    Ok(())
}

fn save_history(path: &Path, history: &OperationHistory) -> Result<(), String> {
    validate_history(history)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("operation history must be a regular file".to_string());
        }
    }
    let content = serde_json::to_vec_pretty(history)
        .map_err(|error| format!("could not serialize operation history: {error}"))?;
    if content.len() as u64 > MAX_HISTORY_BYTES {
        return Err("operation history exceeds the safe size limit".to_string());
    }
    let temporary = temporary_path(path)?;
    let write_result =
        write_private_file(&temporary, &content).and_then(|()| replace_file(&temporary, path));
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    sync_parent(path);
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "operation history has no parent directory".to_string())?;
    for _ in 0..8 {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)
            .map_err(|error| format!("could not create an operation identifier: {error}"))?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let candidate = parent.join(format!(".{HISTORY_FILE_NAME}.{suffix}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("could not allocate a temporary operation history file".to_string())
}

fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not create operation history: {error}"))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not save operation history: {error}"))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("could not replace operation history: {error}"))
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(format!(
            "could not replace operation history: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) {}

fn trim_history(records: &mut Vec<OperationRecord>) {
    while records.len() > MAX_RECORDS {
        if let Some(index) = records.iter().position(|record| record.state.is_terminal()) {
            records.remove(index);
        } else {
            break;
        }
    }
}

fn failure_classification(error: &CommandError) -> (&'static str, bool) {
    use crate::models::CommandErrorCode;
    match error.code {
        CommandErrorCode::ConfigLoadFailed => ("config_load_failed", false),
        CommandErrorCode::DownloadCancelled => ("download_cancelled", true),
        CommandErrorCode::InstallFailed => ("install_failed", true),
        CommandErrorCode::UnsupportedCapability => ("unsupported_capability", false),
        CommandErrorCode::BackendMismatch => ("backend_mismatch", false),
        CommandErrorCode::InvalidRequest => ("invalid_request", false),
        CommandErrorCode::PreconditionFailed => (
            "precondition_failed",
            error
                .details
                .as_ref()
                .is_some_and(|details| details.retryable),
        ),
        CommandErrorCode::OperationFailed => (
            "operation_failed",
            error
                .details
                .as_ref()
                .is_some_and(|details| details.retryable),
        ),
        CommandErrorCode::ExternalProcessTimeout => ("external_process_timeout", true),
        CommandErrorCode::ExternalProcessCancelled => ("external_process_cancelled", true),
        CommandErrorCode::ExternalProcessInterrupted => ("external_process_interrupted", true),
        CommandErrorCode::ToolchainUnavailable => ("toolchain_unavailable", false),
        CommandErrorCode::RuntimeVersionUnsupported => ("runtime_version_unsupported", false),
        CommandErrorCode::ConsistencyFailed => ("consistency_failed", false),
        CommandErrorCode::RuntimeBuildFailed => ("runtime_build_failed", false),
        CommandErrorCode::RuntimeInitializationFailed => ("runtime_initialization_failed", false),
        CommandErrorCode::RuntimeReadinessTimeout => ("runtime_readiness_timeout", true),
        CommandErrorCode::RuntimeSessionNotFound => ("runtime_session_not_found", false),
        CommandErrorCode::RuntimeExited => ("runtime_exited", false),
        CommandErrorCode::RuntimeGuestOffline => ("runtime_guest_offline", true),
        CommandErrorCode::RuntimePortConflict => ("runtime_port_conflict", true),
        CommandErrorCode::RuntimePortForwardingInvalid => ("runtime_port_forwarding_invalid", true),
        CommandErrorCode::RuntimeFirewallBlocked => ("runtime_firewall_blocked", true),
        CommandErrorCode::RuntimeNotListening => ("runtime_not_listening", true),
        CommandErrorCode::RuntimeComposeRecoveryFailed => {
            ("runtime_compose_recovery_failed", false)
        }
    }
}

fn safe_reason(reason: &str) -> String {
    if reason
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        && reason.len() <= MAX_REASON_BYTES
    {
        reason.to_string()
    } else {
        "operation_failed".to_string()
    }
}

fn operation_id(kind: OperationKind, version: &str) -> Result<String, String> {
    let prefix = match kind {
        OperationKind::Install => "install",
        OperationKind::Uninstall => "uninstall",
        OperationKind::Launch => "launch",
    };
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not create an operation identifier: {error}"))?;
    let random = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}-{version}-{random}"))
}

fn validate_identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err("operation history contains an invalid identifier".to_string());
    }
    Ok(())
}

fn validate_target_version(value: &str) -> Result<(), String> {
    validate_identifier(value)?;
    crate::platform::validate_version(value)
}

fn validate_short_text(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_REASON_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("operation history contains unsafe text".to_string());
    }
    Ok(())
}

fn valid_legacy_id(id: &str) -> bool {
    validate_identifier(id).is_ok()
        && id.rsplit_once('-').is_some_and(|(_, suffix)| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn legacy_target(id: &str) -> Option<(OperationKind, String)> {
    let (kind, remainder) = if let Some(value) = id.strip_prefix("install-") {
        (OperationKind::Install, value)
    } else if let Some(value) = id.strip_prefix("uninstall-") {
        (OperationKind::Uninstall, value)
    } else {
        (OperationKind::Launch, id.strip_prefix("launch-")?)
    };
    let (version, _) = remainder.rsplit_once('-')?;
    validate_target_version(version).ok()?;
    Some((kind, version.to_string()))
}

fn lock_store() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "operation history lock is poisoned".to_string())
}

fn active_operations() -> Result<std::sync::MutexGuard<'static, HashSet<ActiveOperation>>, String> {
    ACTIVE_OPERATIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map_err(|_| "active operation lock is poisoned".to_string())
}

fn active_session_actions(
) -> Result<std::sync::MutexGuard<'static, HashSet<ActiveSessionAction>>, String> {
    ACTIVE_SESSION_ACTIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map_err(|_| "active session action lock is poisoned".to_string())
}

fn remove_active(history_path: &Path, id: &str) {
    if let Ok(mut active) = active_operations() {
        active.remove(&ActiveOperation {
            history_path: history_path.to_path_buf(),
            id: id.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{OperationTracker, SessionActionGuard};
    use crate::contracts::{BackendError, BackendId, CapabilityId};
    use crate::models::{
        AppConfig, CommandError, CommandErrorCode, ContainerRuntime, OperationKind, OperationStage,
        OperationState,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    fn config(path: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47271".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47273,
            shared_directory: path.to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn history_path(config: &AppConfig) -> PathBuf {
        let directory = Path::new(&config.shared_directory).join("trusted-config");
        fs::create_dir_all(&directory).expect("trusted history directory");
        directory.join(super::HISTORY_FILE_NAME)
    }

    fn begin(
        config: &AppConfig,
        kind: OperationKind,
        target_version: &str,
        protected_project: bool,
        stage: OperationStage,
        retry_of: Option<String>,
    ) -> Result<OperationTracker, String> {
        OperationTracker::begin_at(
            config,
            history_path(config),
            kind,
            target_version,
            protected_project,
            stage,
            retry_of,
        )
    }

    fn list(config: &AppConfig) -> Result<Vec<crate::models::OperationRecord>, String> {
        super::list_at(config, &history_path(config))
    }

    fn retry_source(
        config: &AppConfig,
        id: &str,
    ) -> Result<crate::models::OperationRecord, String> {
        super::retry_source_at(config, &history_path(config), id)
    }

    fn clear_completed(config: &AppConfig) -> Result<usize, String> {
        super::clear_completed_at(config, &history_path(config))
    }

    fn begin_session_action(
        config: &AppConfig,
        target_version: &str,
    ) -> Result<SessionActionGuard, String> {
        SessionActionGuard::begin_at(config, history_path(config), target_version)
    }

    #[test]
    fn serializes_mutations_against_launches_for_only_the_same_version() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let launch = begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            false,
            OperationStage::Launching,
            None,
        )
        .expect("first launch");
        let parallel_launch = begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            false,
            OperationStage::Launching,
            None,
        )
        .expect("parallel launches remain allowed");
        assert!(begin(
            &config,
            OperationKind::Uninstall,
            "11.12.2",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .is_err());
        let other_version = begin(
            &config,
            OperationKind::Uninstall,
            "11.13.0",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .expect("other versions remain independent");
        drop(other_version);
        drop(parallel_launch);
        drop(launch);

        let install = begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("install after launches end");
        assert!(begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            false,
            OperationStage::Launching,
            None,
        )
        .is_err());
        assert!(begin(
            &config,
            OperationKind::Uninstall,
            "11.12.2",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .is_err());
        drop(install);
    }

    #[test]
    fn serializes_session_actions_against_install_and_uninstall() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let action = begin_session_action(&config, "11.12.2").expect("session action");
        assert!(begin_session_action(&config, "11.12.2").is_err());
        assert!(begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .is_err());
        drop(action);

        let uninstall = begin(
            &config,
            OperationKind::Uninstall,
            "11.12.2",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .expect("uninstall after session action ends");
        assert!(begin_session_action(&config, "11.12.2").is_err());
        drop(uninstall);
        assert!(begin_session_action(&config, "11.12.2").is_ok());
    }

    #[test]
    fn persists_progress_and_terminal_history_across_reloads() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let mut tracker = begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Starting,
            None,
        )
        .expect("begin operation");
        tracker
            .progress(OperationStage::Downloading, Some(42.5), false)
            .expect("persist progress");
        tracker.succeed().expect("finish operation");

        let records = list(&config).expect("reload operation history");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, OperationState::Succeeded);
        assert_eq!(records[0].stage, OperationStage::Completed);
        assert_eq!(records[0].percentage, Some(100.0));
        let serialized = fs::read_to_string(history_path(&config)).expect("read history");
        assert!(!serialized.contains("host.lan"));
        assert!(!serialized.contains("Program Files"));
    }

    #[test]
    fn external_process_deadlines_and_cancellation_are_exact_terminal_states() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let timeout = begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin timed operation");
        timeout
            .fail(&CommandError::new(
                CommandErrorCode::ExternalProcessTimeout,
                "the installer reached its deadline".into(),
            ))
            .expect("persist timeout");

        let cancelled = begin(
            &config,
            OperationKind::Install,
            "11.13.0",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin cancelled operation");
        cancelled
            .cancel_with_code("external_process_cancelled", "external_process_cancelled")
            .expect("persist cancellation");

        let records = list(&config).expect("reload exact terminal states");
        let timeout = records
            .iter()
            .find(|record| record.target_version == "11.12.2")
            .expect("timeout record");
        assert_eq!(timeout.state, OperationState::Failed);
        assert_eq!(
            timeout.error.as_ref().expect("timeout error").code,
            "external_process_timeout"
        );
        assert_ne!(timeout.state, OperationState::Interrupted);

        let cancelled = records
            .iter()
            .find(|record| record.target_version == "11.13.0")
            .expect("cancelled record");
        assert_eq!(cancelled.state, OperationState::Cancelled);
        assert_eq!(
            cancelled.error.as_ref().expect("cancellation error").code,
            "external_process_cancelled"
        );
    }

    #[test]
    fn migrates_compatible_record_contract_versions_on_the_next_write() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin operation")
        .succeed()
        .expect("finish operation");

        let history = history_path(&config);
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&history).expect("read history"))
                .expect("parse history");
        legacy["records"][0]["schemaVersion"] = serde_json::json!("3.0.0");
        fs::write(
            &history,
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let records = list(&config).expect("load compatible legacy history");
        assert_eq!(
            records[0].schema_version,
            crate::contracts::CONTRACT_SCHEMA_VERSION
        );

        begin(
            &config,
            OperationKind::Uninstall,
            "11.13.0",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .expect("begin operation after migration")
        .succeed()
        .expect("finish operation after migration");
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&history).expect("read migrated history"))
                .expect("parse migrated history");
        assert!(persisted["records"]
            .as_array()
            .expect("records")
            .iter()
            .all(|record| record["schemaVersion"] == crate::contracts::CONTRACT_SCHEMA_VERSION));
    }

    #[test]
    fn rejects_unknown_future_record_contract_versions_without_overwriting_evidence() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin operation")
        .succeed()
        .expect("finish operation");

        let history = history_path(&config);
        let mut future: serde_json::Value =
            serde_json::from_slice(&fs::read(&history).expect("read history"))
                .expect("parse history");
        future["records"][0]["schemaVersion"] = serde_json::json!("99.0.0");
        let future = serde_json::to_vec_pretty(&future).expect("serialize future history");
        fs::write(&history, &future).expect("write future history");

        assert!(list(&config).is_err());
        assert_eq!(fs::read(&history).expect("future evidence remains"), future);
    }

    #[test]
    fn dropped_operations_are_interrupted_and_retryable_when_safe() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let tracker = begin(
            &config,
            OperationKind::Uninstall,
            "11.12.2",
            false,
            OperationStage::Uninstalling,
            None,
        )
        .expect("begin operation");
        drop(tracker);
        let records = list(&config).expect("list operations");
        assert_eq!(records[0].state, OperationState::Interrupted);
        assert!(records[0].retryable);
    }

    #[test]
    fn an_unaccepted_completed_launch_is_reclassified_as_interrupted() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let tracker = begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            true,
            OperationStage::Launching,
            None,
        )
        .expect("begin launch");
        let id = tracker.id().to_string();
        tracker.succeed().expect("complete backend launch");

        super::interrupt_completed_launch_at(&history_path(&config), &id)
            .expect("interrupt unaccepted launch");
        let record = list(&config).expect("list interrupted launch").remove(0);
        assert_eq!(record.state, OperationState::Interrupted);
        assert_eq!(record.stage, OperationStage::Interrupted);
        assert_eq!(record.percentage, None);
        assert_eq!(
            record.error.expect("interruption error").code,
            "operation_interrupted"
        );
        assert!(!record.retryable);
        assert!(super::interrupt_completed_launch_at(&history_path(&config), &id).is_err());
    }

    #[test]
    fn keeps_live_in_process_work_running_and_reconciles_it_after_a_restart_boundary() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let tracker = begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin operation");
        let id = tracker.id().to_string();
        assert_eq!(
            list(&config).expect("list live operation")[0].state,
            OperationState::Running
        );

        let history = history_path(&config);
        std::mem::forget(tracker);
        super::remove_active(&history, &id);

        let records = list(&config).expect("reconcile after restart boundary");
        assert_eq!(records[0].state, OperationState::Interrupted);
        assert_eq!(records[0].stage, OperationStage::Interrupted);
        assert!(records[0].finished_at.is_some());
        assert!(records[0].retryable);
    }

    #[test]
    fn never_retries_a_protected_project_launch_without_its_unpersisted_path() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let tracker = begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            true,
            OperationStage::Launching,
            None,
        )
        .expect("begin protected launch");
        let id = tracker.id().to_string();
        let mut backend_error = BackendError::operation(
            BackendId::LinuxWinboat,
            CapabilityId::StudioStart,
            "project launch failed",
        );
        backend_error.retryable = true;
        tracker
            .fail(&CommandError::from(backend_error))
            .expect("persist protected failure");

        let record = list(&config).expect("list protected failure").remove(0);
        assert!(record.protected_project);
        assert!(!record.retryable);
        assert!(retry_source(&config, &id).is_err());
    }

    #[test]
    fn clear_removes_only_terminal_records_and_never_operation_artifacts() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let logs = temporary.path().join(".mendimaru/operations");
        fs::create_dir_all(&logs).expect("log directory");
        let report = logs.join("keep.json");
        fs::write(&report, b"report").expect("report fixture");
        let active = begin(
            &config,
            OperationKind::Install,
            "11.13.0",
            false,
            OperationStage::Downloading,
            None,
        )
        .expect("active operation");
        let failed = begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("failed operation");
        let mut backend_error = BackendError::operation(
            BackendId::LinuxWinboat,
            CapabilityId::StudioInstall,
            r"secret C:\Users\dev\token",
        );
        backend_error.diagnostic_ref = Some("windows-exit-code:1603".into());
        backend_error.retryable = true;
        failed
            .fail(&CommandError::from(backend_error))
            .expect("persist failure");

        let failed_record = list(&config)
            .expect("list failure")
            .into_iter()
            .find(|record| record.state == OperationState::Failed)
            .expect("failed record");
        assert_eq!(
            failed_record.error.expect("failure details").exit_code,
            Some(1603)
        );
        assert!(failed_record.retryable);
        let serialized = fs::read_to_string(history_path(&config)).expect("read failure history");
        assert!(!serialized.contains("Users"));
        assert!(!serialized.contains("token"));

        assert_eq!(clear_completed(&config).expect("clear completed"), 1);
        let records = list(&config).expect("list remaining");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, OperationState::Running);
        assert!(report.exists());
        drop(active);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_and_oversized_history_without_overwriting_it() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let history = history_path(&config);
        let outside = temporary.path().join("outside.json");
        fs::write(&outside, b"sensitive").expect("outside fixture");
        symlink(&outside, &history).expect("history symlink");
        assert!(list(&config).is_err());
        assert_eq!(fs::read(&outside).expect("outside remains"), b"sensitive");

        fs::remove_file(&history).expect("remove link");
        let oversized = fs::File::create(&history).expect("oversized history");
        oversized
            .set_len(super::MAX_HISTORY_BYTES + 1)
            .expect("extend history");
        assert!(list(&config).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_scan_a_symlinked_legacy_report_directory() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let app_directory = temporary.path().join(".mendimaru");
        let outside = temporary.path().join("outside-reports");
        fs::create_dir_all(&app_directory).expect("app directory");
        fs::create_dir_all(&outside).expect("outside report directory");
        fs::write(
            outside.join("install-11.12.2-0123456789abcdef0123456789abcdef.json"),
            b"private payload",
        )
        .expect("outside report");
        symlink(&outside, app_directory.join("operations")).expect("operation directory link");

        assert!(list(&config).is_err());
        assert!(super::log_directory(&config).is_err());
        assert!(!history_path(&config).exists());
    }

    #[test]
    fn opens_only_an_existing_operation_log_directory_without_creating_one() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let app_directory = temporary.path().join(".mendimaru");
        let logs = app_directory.join("operations");

        assert!(super::log_directory(&config).is_err());
        assert!(!app_directory.exists());

        fs::create_dir_all(&logs).expect("operation log directory");
        assert_eq!(super::log_directory(&config).expect("validated logs"), logs);
    }

    #[test]
    fn rejects_corrupt_or_extended_schemas_without_replacing_evidence() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let history = history_path(&config);
        let corrupt = br#"{"schemaVersion":"1.0.0","records":[],"unexpected":true}"#;
        fs::write(&history, corrupt).expect("corrupt history fixture");

        assert!(list(&config).is_err());
        assert_eq!(
            fs::read(&history).expect("corrupt evidence remains"),
            corrupt
        );
    }

    #[test]
    fn rejects_semantically_forged_retry_flags() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        begin(
            &config,
            OperationKind::Install,
            "11.12.2",
            false,
            OperationStage::Installing,
            None,
        )
        .expect("begin operation")
        .succeed()
        .expect("finish operation");

        let history = history_path(&config);
        let mut forged: serde_json::Value =
            serde_json::from_slice(&fs::read(&history).expect("read valid history"))
                .expect("parse valid history");
        forged["records"][0]["retryable"] = serde_json::json!(true);
        let forged = serde_json::to_vec_pretty(&forged).expect("serialize forged history");
        fs::write(&history, &forged).expect("write forged history");

        assert!(list(&config).is_err());
        assert_eq!(fs::read(&history).expect("forged evidence remains"), forged);
    }

    #[cfg(unix)]
    #[test]
    fn writes_private_history_and_leaves_no_temporary_files() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        begin(
            &config,
            OperationKind::Launch,
            "11.12.2",
            false,
            OperationStage::Launching,
            None,
        )
        .expect("begin operation")
        .succeed()
        .expect("finish operation");

        let history = history_path(&config);
        let history_directory = history.parent().expect("history directory");
        assert_eq!(
            fs::metadata(&history)
                .expect("history metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(fs::read_dir(history_directory)
            .expect("list app directory")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn imports_only_safe_legacy_report_names_without_trusting_payloads() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let logs = temporary.path().join(".mendimaru/operations");
        fs::create_dir_all(&logs).expect("log directory");
        fs::write(
            logs.join("install-11.12.2-0123456789abcdef0123456789abcdef.json"),
            br#"{"state":"succeeded","secret":"must-not-be-read"}"#,
        )
        .expect("legacy report");

        let records = list(&config).expect("import legacy report name");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, OperationState::Interrupted);
        assert_eq!(
            records[0].error.as_ref().expect("legacy error").code,
            "legacy_report_untrusted"
        );
        let serialized = fs::read_to_string(history_path(&config)).expect("history");
        assert!(!serialized.contains("must-not-be-read"));

        assert_eq!(clear_completed(&config).expect("clear legacy record"), 1);
        assert!(list(&config)
            .expect("legacy scan stays complete")
            .is_empty());
        assert!(logs
            .join("install-11.12.2-0123456789abcdef0123456789abcdef.json")
            .exists());
    }
}
