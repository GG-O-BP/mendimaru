//! Shared browser test session ownership for Linux WinBoat. See
//! docs/browser-testing.md. One owner prepares a session from a
//! readiness-verified Runtime and records its exact Runtime/Studio identity.
//! Test workers attach with that recorded identity instead of reopening
//! Studio metadata, and their exit only releases their own participation.
//!
//! Liveness is decided exclusively by kernel `flock` ownership on
//! per-participant lock files, mirroring `vm_use`: no PID records, no
//! reference counting, no RDP disconnect inference, and no eviction. Crashes
//! and signals close descriptors through the kernel. A participant count
//! never authorizes Studio termination or VM cleanup; only an explicit
//! finalize transition under the session's finalize lock applies the recorded
//! policy, and a Runtime stop additionally requires the recorded owner claim.
#![cfg(target_os = "linux")]
use crate::contracts::{RuntimeMode, CONTRACT_SCHEMA_VERSION};
use cap_std::fs::OpenOptionsExt as CapOpenOptionsExt;
use cap_std::fs::PermissionsExt as CapPermissionsExt;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::PathBuf;
use std::time::Duration;

pub(crate) const UNKNOWN: &str = "unknown shared browser session identifier";
pub(crate) const UNTRUSTED: &str =
    "the shared browser session registry could not be verified; check ownership and permissions";
pub(crate) const STILL_PREPARING: &str = "the shared browser session is still preparing";
pub(crate) const FINALIZING: &str =
    "the shared browser session is finalizing; retry after finalize completes";
pub(crate) const FINALIZED: &str = "the shared browser session is finalized";
pub(crate) const WRONG_VM: &str = "the shared browser session belongs to a different WinBoat VM";
pub(crate) const FINALIZE_BUSY: &str = "shared browser session finalize is already in progress";
pub(crate) const DRAINED_TIMEOUT: &str =
    "shared browser session participants are still active; retry finalize after they exit";

const MAX_RECORD_BYTES: u64 = 8 * 1024;
const MAX_URL_LENGTH: usize = 4096;
const MAX_MARKER_LENGTH: usize = 255;
const FINALIZE_WAIT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const RECORD_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    Preparing,
    Ready,
    Finalizing,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FinalizePolicy {
    Keep,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RuntimeOrigin {
    OwnerStarted,
    AttachedExisting,
}

/// The stabilized identity handed to participants. Recorded once at prepare
/// time; participants never re-derive it, so attaching opens no Runtime
/// metadata path and no RDP connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Identity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) studio_session_id: Option<String>,
    pub(crate) runtime_mode: RuntimeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) studio_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_version: Option<String>,
    pub(crate) base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) host_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) guest_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Descriptor {
    pub(crate) schema_version: String,
    pub(crate) session_id: String,
    pub(crate) state: State,
    pub(crate) vm_key: String,
    pub(crate) runtime_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) build_marker: Option<String>,
    pub(crate) finalize_policy: FinalizePolicy,
    pub(crate) runtime_origin: RuntimeOrigin,
    pub(crate) prepared_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) identity: Option<Identity>,
}

pub(crate) struct NewSession {
    pub(crate) vm_key: String,
    pub(crate) runtime_session_id: String,
    pub(crate) build_marker: Option<String>,
    pub(crate) finalize_policy: FinalizePolicy,
    pub(crate) runtime_origin: RuntimeOrigin,
}

/// Registers participation on construction and always releases it on drop.
/// A crash skips `Drop`, and the kernel then releases the lock; the leftover
/// file is inert and pruned by a later finalize or record GC.
#[derive(Debug)]
pub(crate) struct ParticipantGuard {
    directory: cap_std::fs::Dir,
    participant: String,
    file: File,
}

impl ParticipantGuard {
    /// Detach is idempotent at the kernel level; the unlink is best-effort
    /// and only touches this participant's own file.
    pub(crate) fn detach(mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        let _ = self.directory.remove_file(&self.participant);
        self.participant.clear();
    }
}

impl Drop for ParticipantGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        if !self.participant.is_empty() {
            let _ = self.directory.remove_file(&self.participant);
        }
    }
}

/// Held while a finalize transition is pending. Duplicate finalize callers
/// receive `FINALIZE_BUSY`; a crashed finalizer has it released by the kernel,
/// which is the defined recovery for an interrupted finalize.
#[derive(Debug)]
pub(crate) struct FinalizeGuard {
    file: File,
}

impl Drop for FinalizeGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

pub(crate) fn valid_session_id(value: &str) -> bool {
    value.len() == 39
        && value.strip_prefix("shared_").is_some_and(|suffix| {
            suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn record_name(session_id: &str) -> String {
    format!("{session_id}.json")
}

fn finalize_lock_name(session_id: &str) -> String {
    format!("{session_id}.final.lock")
}

pub(crate) fn directory() -> Result<cap_std::fs::Dir, &'static str> {
    let uid = unsafe { libc::geteuid() };
    // A fixed local namespace like vm_use: TMPDIR/XDG/cache overrides cannot
    // split the registry across caches or worktrees.
    let path = PathBuf::from(format!("/tmp/mendimaru-test-sessions-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(UNTRUSTED),
    }
    let opened = super::nvram::open_directory(&path).map_err(|_| UNTRUSTED)?;
    if opened
        .dir_metadata()
        .map_err(|_| UNTRUSTED)?
        .permissions()
        .mode()
        & 0o077
        != 0
    {
        return Err(UNTRUSTED);
    }
    Ok(opened)
}

fn open_lock(directory: &cap_std::fs::Dir, name: &str) -> Result<File, &'static str> {
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| UNTRUSTED)?
        .into_std();
    let metadata = file.metadata().map_err(|_| UNTRUSTED)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(UNTRUSTED);
    }
    Ok(file)
}

fn open_existing(directory: &cap_std::fs::Dir, name: &str) -> Result<File, &'static str> {
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| UNTRUSTED)?
        .into_std();
    let metadata = file.metadata().map_err(|_| UNTRUSTED)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_RECORD_BYTES
    {
        return Err(UNTRUSTED);
    }
    Ok(file)
}

fn read_record(directory: &cap_std::fs::Dir, name: &str) -> Result<Descriptor, &'static str> {
    let file = match open_existing(directory, name) {
        Ok(file) => file,
        // Only a genuinely absent entry is unknown; a present but untrusted
        // entry (symlink, public file, non-file) fails closed.
        Err(UNTRUSTED) if directory.symlink_metadata(name).is_err() => return Err(UNKNOWN),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata().map_err(|_| UNTRUSTED)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNTRUSTED)?;
    let descriptor: Descriptor = serde_json::from_slice(&bytes).map_err(|_| UNTRUSTED)?;
    validate_descriptor(&descriptor)?;
    Ok(descriptor)
}

fn validate_descriptor(descriptor: &Descriptor) -> Result<(), &'static str> {
    if descriptor.schema_version != CONTRACT_SCHEMA_VERSION
        || !valid_session_id(&descriptor.session_id)
        || descriptor.vm_key.len() != 64
        || !descriptor.vm_key.bytes().all(|b| b.is_ascii_hexdigit())
        || !descriptor.runtime_session_id.starts_with("runtime_")
        || descriptor.runtime_session_id.len() != 40
        || descriptor
            .build_marker
            .as_deref()
            .is_some_and(|marker| marker.is_empty() || marker.len() > MAX_MARKER_LENGTH)
        || matches!(&descriptor.identity, Some(identity) if identity.base_url.len() > MAX_URL_LENGTH)
        || (descriptor.state == State::Ready && descriptor.identity.is_none())
    {
        return Err(UNTRUSTED);
    }
    Ok(())
}

fn write_record(directory: &cap_std::fs::Dir, descriptor: &Descriptor) -> Result<(), &'static str> {
    let nonce = crate::contracts::secure_identifier("tmp")
        .map_err(|_| UNTRUSTED)?
        .trim_start_matches("tmp_")
        .to_string();
    let temporary = format!(".{nonce}.tmp");
    let bytes = serde_json::to_vec(descriptor).map_err(|_| UNTRUSTED)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = directory
        .open_with(&temporary, &options)
        .map_err(|_| UNTRUSTED)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| UNTRUSTED)?;
    drop(file);
    directory
        .rename(&temporary, directory, record_name(&descriptor.session_id))
        .map_err(|_| UNTRUSTED)
}

fn validate_new_session(session: &NewSession) -> Result<(), &'static str> {
    if session.vm_key.len() != 64
        || !session.vm_key.bytes().all(|b| b.is_ascii_hexdigit())
        || !session.runtime_session_id.starts_with("runtime_")
        || session.runtime_session_id.len() != 40
        || session
            .build_marker
            .as_deref()
            .is_some_and(|marker| marker.is_empty() || marker.len() > MAX_MARKER_LENGTH)
    {
        return Err(UNTRUSTED);
    }
    Ok(())
}

/// Creates the session record in `Preparing`. The record exists before the
/// owner verifies readiness, so a concurrent attach observes an explicit
/// `STILL_PREPARING` refusal instead of racing the verification.
pub(crate) fn create(session: &NewSession) -> Result<String, &'static str> {
    validate_new_session(session)?;
    let directory = directory()?;
    discard_expired(&directory);
    let session_id = crate::contracts::secure_identifier("shared").map_err(|_| UNTRUSTED)?;
    let descriptor = Descriptor {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id: session_id.clone(),
        state: State::Preparing,
        vm_key: session.vm_key.clone(),
        runtime_session_id: session.runtime_session_id.clone(),
        build_marker: session.build_marker.clone(),
        finalize_policy: session.finalize_policy,
        runtime_origin: session.runtime_origin,
        prepared_at: chrono::Utc::now(),
        identity: None,
    };
    write_record(&directory, &descriptor)?;
    Ok(session_id)
}

/// Publishes the stabilized identity. Only the preparing owner calls this
/// before the session identifier has been returned to any participant.
pub(crate) fn mark_ready(session_id: &str, identity: Identity) -> Result<Descriptor, &'static str> {
    if !valid_session_id(session_id) || identity.base_url.len() > MAX_URL_LENGTH {
        return Err(UNTRUSTED);
    }
    let directory = directory()?;
    let mut descriptor = read_record(&directory, &record_name(session_id))?;
    if descriptor.state != State::Preparing {
        return Err(UNTRUSTED);
    }
    descriptor.identity = Some(identity);
    descriptor.state = State::Ready;
    write_record(&directory, &descriptor)?;
    Ok(descriptor)
}

/// Removes a session that never became ready. Legal only from `Preparing`:
/// participants cannot attach to a non-ready session, so no participation can
/// be lost.
pub(crate) fn discard(session_id: &str) -> Result<(), &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    let descriptor = read_record(&directory, &record_name(session_id))?;
    if descriptor.state != State::Preparing {
        return Err(UNTRUSTED);
    }
    let _ = directory.remove_file(record_name(session_id));
    let _ = directory.remove_file(finalize_lock_name(session_id));
    Ok(())
}

pub(crate) fn load(session_id: &str) -> Result<Descriptor, &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    read_record(&directory()?, &record_name(session_id))
}

fn attach_refusal(state: State) -> &'static str {
    match state {
        State::Preparing => STILL_PREPARING,
        State::Finalizing => FINALIZING,
        State::Finalized => FINALIZED,
        State::Ready => UNTRUSTED,
    }
}

/// Registers this process as a live participant. The lock file is created and
/// locked first, then the state is re-read: a finalize transition that started
/// before registration is observed here, and one that starts after sees the
/// held lock while draining participants.
pub(crate) fn register_participant(
    session_id: &str,
    vm_key: &str,
) -> Result<ParticipantGuard, &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    let descriptor = load(session_id)?;
    if descriptor.state != State::Ready {
        return Err(attach_refusal(descriptor.state));
    }
    if descriptor.vm_key != vm_key {
        return Err(WRONG_VM);
    }
    let nonce = crate::contracts::secure_identifier("p")
        .map_err(|_| UNTRUSTED)?
        .trim_start_matches("p_")
        .to_string();
    let participant = format!("{session_id}.p-{nonce}.lock");
    let file = open_lock(&directory, &participant)?;
    let registration = fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| UNTRUSTED)
        .and_then(|()| {
            load(session_id).and_then(|descriptor| {
                if descriptor.state == State::Ready {
                    Ok(())
                } else {
                    Err(attach_refusal(descriptor.state))
                }
            })
        });
    if let Err(error) = registration {
        let _ = fs2::FileExt::unlock(&file);
        let _ = directory.remove_file(&participant);
        return Err(error);
    }
    Ok(ParticipantGuard {
        directory,
        participant,
        file,
    })
}

fn participant_files(directory: &cap_std::fs::Dir, session_id: &str) -> Vec<String> {
    let prefix = format!("{session_id}.p-");
    let mut names = Vec::new();
    let Ok(entries) = directory.entries() else {
        return names;
    };
    for entry in entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            if name.starts_with(&prefix) && name.ends_with(".lock") {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

/// Counts participants whose kernel lock is still held. This is live
/// participation, not a historical reference count, and it alone never
/// triggers cleanup.
pub(crate) fn live_participants(session_id: &str) -> Result<usize, &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    let mut live = 0;
    for name in participant_files(&directory, session_id) {
        let file = match open_lock(&directory, &name) {
            Ok(file) => file,
            // An entry that vanished while listing is not live
            // participation; a present but untrusted one fails closed.
            Err(UNTRUSTED) if directory.symlink_metadata(&name).is_err() => continue,
            Err(error) => return Err(error),
        };
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => live += 1,
            Err(_) => return Err(UNTRUSTED),
        }
    }
    Ok(live)
}

/// Removes participant files that hold no lock. Called only after a session
/// reaches `Finalized`, where new participation is impossible.
pub(crate) fn prune_participants(session_id: &str) -> Result<(), &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    for name in participant_files(&directory, session_id) {
        if let Ok(file) = open_lock(&directory, &name) {
            if fs2::FileExt::try_lock_exclusive(&file).is_ok() {
                let _ = fs2::FileExt::unlock(&file);
                let _ = directory.remove_file(&name);
            }
        }
    }
    Ok(())
}

/// Serializes finalize attempts for one session. Waiting is bounded; a
/// duplicate concurrent finalize receives `FINALIZE_BUSY`.
pub(crate) fn acquire_finalize(
    session_id: &str,
    wait: Duration,
) -> Result<FinalizeGuard, &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    let file = open_lock(&directory, &finalize_lock_name(session_id))?;
    let deadline = std::time::Instant::now() + wait.min(FINALIZE_WAIT);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(FinalizeGuard { file }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(UNTRUSTED),
        }
        if std::time::Instant::now() >= deadline {
            return Err(FINALIZE_BUSY);
        }
        std::thread::sleep(
            POLL_INTERVAL.min(deadline.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

/// Transitions state under the caller's serialization (finalize lock, or the
/// unpublished identifier during prepare). Fails if the record no longer
/// matches `from`, which is the duplicate-transition guard.
pub(crate) fn transition(
    session_id: &str,
    from: State,
    to: State,
) -> Result<Descriptor, &'static str> {
    if !valid_session_id(session_id) {
        return Err(UNKNOWN);
    }
    let directory = directory()?;
    let mut descriptor = read_record(&directory, &record_name(session_id))?;
    if descriptor.state != from {
        return Err(UNTRUSTED);
    }
    descriptor.state = to;
    write_record(&directory, &descriptor)?;
    Ok(descriptor)
}

fn discard_expired(directory: &cap_std::fs::Dir) {
    let now = cap_std::time::SystemTime::from_std(std::time::SystemTime::now());
    let Ok(entries) = directory.entries() else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(session_id) = name.strip_suffix(".json") else {
            continue;
        };
        if !valid_session_id(session_id) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(age) = now.duration_since(metadata.modified().unwrap_or(now)) else {
            continue;
        };
        if age < RECORD_RETENTION {
            continue;
        }
        if let Ok(descriptor) = read_record(directory, &name) {
            if descriptor.state == State::Finalized {
                let _ = directory.remove_file(&name);
                let _ = directory.remove_file(finalize_lock_name(session_id));
                for participant in participant_files(directory, session_id) {
                    let _ = directory.remove_file(&participant);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn hex64(seed: u64) -> String {
        format!("{seed:064x}")
    }

    fn runtime_id(seed: u64) -> String {
        format!("runtime_{seed:032x}")
    }

    fn new_session(seed: u64, policy: FinalizePolicy, origin: RuntimeOrigin) -> NewSession {
        NewSession {
            vm_key: hex64(seed),
            runtime_session_id: runtime_id(seed),
            build_marker: None,
            finalize_policy: policy,
            runtime_origin: origin,
        }
    }

    fn identity() -> Identity {
        Identity {
            studio_session_id: Some("studio-4242-638908128000000000".into()),
            runtime_mode: RuntimeMode::StudioRunLocally,
            studio_version: Some("10.12.0".into()),
            runtime_version: Some("10.12.0".into()),
            base_url: "http://127.0.0.1:8080/".into(),
            host_port: Some(8080),
            guest_port: Some(8080),
        }
    }

    fn prepared(seed: u64) -> String {
        let session_id = create(&new_session(
            seed,
            FinalizePolicy::Keep,
            RuntimeOrigin::AttachedExisting,
        ))
        .unwrap();
        mark_ready(&session_id, identity()).unwrap();
        session_id
    }

    fn cleanup(session_id: &str) {
        let Ok(directory) = directory() else {
            return;
        };
        let _ = directory.remove_file(record_name(session_id));
        let _ = directory.remove_file(finalize_lock_name(session_id));
        for name in participant_files(&directory, session_id) {
            let _ = directory.remove_file(&name);
        }
    }

    #[test]
    fn transitions_are_exactly_once_and_duplicate_guarded() {
        let session_id = prepared(1);
        assert_eq!(load(&session_id).unwrap().state, State::Ready);
        transition(&session_id, State::Ready, State::Finalizing).unwrap();
        assert!(transition(&session_id, State::Ready, State::Finalizing).is_err());
        transition(&session_id, State::Finalizing, State::Finalized).unwrap();
        assert_eq!(load(&session_id).unwrap().state, State::Finalized);
        assert!(transition(&session_id, State::Finalizing, State::Finalized).is_err());
        cleanup(&session_id);
    }

    #[test]
    fn attach_is_refused_outside_ready() {
        let session_id = create(&new_session(
            2,
            FinalizePolicy::Keep,
            RuntimeOrigin::AttachedExisting,
        ))
        .unwrap();
        assert_eq!(
            register_participant(&session_id, &hex64(2)).unwrap_err(),
            STILL_PREPARING
        );
        mark_ready(&session_id, identity()).unwrap();
        transition(&session_id, State::Ready, State::Finalizing).unwrap();
        assert_eq!(
            register_participant(&session_id, &hex64(2)).unwrap_err(),
            FINALIZING
        );
        transition(&session_id, State::Finalizing, State::Finalized).unwrap();
        assert_eq!(
            register_participant(&session_id, &hex64(2)).unwrap_err(),
            FINALIZED
        );
        cleanup(&session_id);
    }

    #[test]
    fn attach_rejects_a_foreign_vm() {
        let session_id = prepared(3);
        assert_eq!(
            register_participant(&session_id, &hex64(99)).unwrap_err(),
            WRONG_VM
        );
        cleanup(&session_id);
    }

    #[test]
    fn participation_liveness_follows_kernel_ownership() {
        let session_id = prepared(4);
        assert_eq!(live_participants(&session_id).unwrap(), 0);
        let guard = register_participant(&session_id, &hex64(4)).unwrap();
        assert_eq!(live_participants(&session_id).unwrap(), 1);
        // A leaked descriptor is an open kernel lock: still live, never a
        // reference count, and no eviction happens.
        std::mem::forget(guard);
        assert_eq!(live_participants(&session_id).unwrap(), 1);
        let second = register_participant(&session_id, &hex64(4)).unwrap();
        assert_eq!(live_participants(&session_id).unwrap(), 2);
        second.detach();
        assert_eq!(live_participants(&session_id).unwrap(), 1);
        // Detach is idempotent at the file level: no second participant file.
        assert_eq!(
            participant_files(&directory().unwrap(), &session_id).len(),
            1
        );
        cleanup(&session_id);
    }

    #[test]
    fn finalize_lock_serializes_and_kernel_recovers() {
        let session_id = prepared(5);
        let guard = acquire_finalize(&session_id, Duration::from_millis(0)).unwrap();
        assert_eq!(
            acquire_finalize(&session_id, Duration::from_millis(50)).unwrap_err(),
            FINALIZE_BUSY
        );
        drop(guard);
        // The dropped descriptor models a crashed finalizer: the kernel
        // released the lock and a retry proceeds.
        acquire_finalize(&session_id, Duration::from_millis(50)).unwrap();
        cleanup(&session_id);
    }

    #[test]
    fn discard_only_removes_unpublished_sessions() {
        let preparing = create(&new_session(
            6,
            FinalizePolicy::Keep,
            RuntimeOrigin::AttachedExisting,
        ))
        .unwrap();
        discard(&preparing).unwrap();
        assert_eq!(load(&preparing).unwrap_err(), UNKNOWN);
        let session_id = prepared(7);
        assert!(discard(&session_id).is_err());
        assert_eq!(load(&session_id).unwrap().state, State::Ready);
        cleanup(&session_id);
    }

    #[test]
    fn registry_trust_boundaries_are_enforced() {
        let session_id = prepared(8);
        let directory = directory().unwrap();
        let record = record_name(&session_id);
        let original = fs::read(format!(
            "/tmp/mendimaru-test-sessions-{}/{}",
            unsafe { libc::geteuid() },
            record
        ))
        .unwrap();

        // Unknown fields, drifted schema, and oversized records are refused.
        let path = format!(
            "/tmp/mendimaru-test-sessions-{}/{}",
            unsafe { libc::geteuid() },
            record
        );
        let rewrite = |bytes: &[u8]| {
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        };
        let mutated: serde_json::Value = serde_json::from_slice(&original).unwrap_or_default();
        let mut extra = mutated.clone();
        extra["unexpected"] = serde_json::json!("field");
        directory
            .rename(&record, &directory, format!("{session_id}.keep"))
            .unwrap();
        rewrite(&serde_json::to_vec(&extra).unwrap());
        assert_eq!(load(&session_id).unwrap_err(), UNTRUSTED);

        let mut drifted = mutated.clone();
        drifted["schemaVersion"] = serde_json::json!("4.0.0");
        rewrite(&serde_json::to_vec(&drifted).unwrap());
        assert_eq!(load(&session_id).unwrap_err(), UNTRUSTED);

        rewrite(&vec![b'{'; MAX_RECORD_BYTES as usize + 1]);
        assert_eq!(load(&session_id).unwrap_err(), UNTRUSTED);

        // A record symlink is refused before any read.
        fs::remove_file(format!(
            "/tmp/mendimaru-test-sessions-{}/{}",
            unsafe { libc::geteuid() },
            record
        ))
        .unwrap();
        std::os::unix::fs::symlink(
            format!(
                "/tmp/mendimaru-test-sessions-{}/{}",
                unsafe { libc::geteuid() },
                session_id
            ),
            format!(
                "/tmp/mendimaru-test-sessions-{}/{}",
                unsafe { libc::geteuid() },
                record
            ),
        )
        .unwrap();
        assert_eq!(load(&session_id).unwrap_err(), UNTRUSTED);
        fs::remove_file(&path).unwrap();
        rewrite(&original);
        assert_eq!(load(&session_id).unwrap().state, State::Ready);

        // A public participant entry fails closed for liveness probing.
        let guard = register_participant(&session_id, &hex64(8)).unwrap();
        let participants = participant_files(&directory, &session_id);
        assert_eq!(participants.len(), 1);
        fs::set_permissions(
            format!(
                "/tmp/mendimaru-test-sessions-{}/{}",
                unsafe { libc::geteuid() },
                participants[0]
            ),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(live_participants(&session_id).unwrap_err(), UNTRUSTED);
        drop(guard);
        let _ = directory.remove_file(&participants[0]);
        let _ = directory.remove_file(format!("{session_id}.keep"));
        cleanup(&session_id);
    }

    #[test]
    fn prune_removes_only_unlocked_participant_files() {
        let session_id = prepared(9);
        let guard = register_participant(&session_id, &hex64(9)).unwrap();
        prune_participants(&session_id).unwrap();
        assert_eq!(
            participant_files(&directory().unwrap(), &session_id).len(),
            1
        );
        guard.detach();
        prune_participants(&session_id).unwrap();
        assert_eq!(
            participant_files(&directory().unwrap(), &session_id).len(),
            0
        );
        cleanup(&session_id);
    }

    #[test]
    fn unknown_identifiers_fail_closed() {
        assert_eq!(
            load(&format!("shared_{}", "0".repeat(32))).unwrap_err(),
            UNKNOWN
        );
        assert!(register_participant(&format!("shared_{}", "0".repeat(32)), &hex64(10)).is_err());
        assert_eq!(load("shared_shorty").unwrap_err(), UNKNOWN);
    }
}
