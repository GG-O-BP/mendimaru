//! One readiness contract for the desktop command, CLI and Studio operations.
use crate::contracts::{BackendError, BackendErrorCode, BackendId, CapabilityId};
use crate::models::{AppConfig, ContainerStatus};
use crate::process::{self, CommandFailureKind, CommandPolicy};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupPhase {
    StartingContainer,
    WaitingForGuest,
    Online,
    StartupFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupAttempt {
    pub id: u64,
    pub started_at: String,
    pub phase: StartupPhase,
    pub error_code: Option<BackendErrorCode>,
    pub container_status: ContainerStatus,
}

static ATTEMPT: Mutex<Option<(String, StartupAttempt)>> = Mutex::new(None);
static START_GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn identity(config: &AppConfig) -> String {
    format!(
        "{}\0{}\0{}",
        config.container_runtime.as_str(),
        config.compose_file,
        config.container_name
    )
}

pub fn snapshot(config: &AppConfig) -> Option<StartupAttempt> {
    ATTEMPT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(key, _)| *key == identity(config))
        .map(|(_, attempt)| attempt.clone())
}

fn update(phase: StartupPhase, status: ContainerStatus, code: Option<BackendErrorCode>) {
    if let Some((_, attempt)) = ATTEMPT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        attempt.phase = phase;
        attempt.container_status = status;
        attempt.error_code = code;
    }
}

pub(crate) fn failure(code: BackendErrorCode) -> BackendError {
    let message = match code {
        BackendErrorCode::ContainerExitedDuringStartup => {
            crate::tr!("error-container-exited-during-startup")
        }
        BackendErrorCode::GuestStartupTimeout => crate::tr!("error-guest-startup-timeout"),
        _ => crate::tr!("error-startup-command-failed"),
    };
    let mut error =
        BackendError::operation(BackendId::LinuxWinboat, CapabilityId::StudioDetect, message);
    error.code = code;
    error.retryable = true;
    error
}

trait StartupDriver {
    fn inspect(
        &self,
        deadline: Instant,
    ) -> impl Future<Output = Result<ContainerStatus, BackendError>> + Send;
    fn start(&self, deadline: Instant) -> impl Future<Output = Result<(), BackendError>> + Send;
    fn health(&self, deadline: Instant) -> impl Future<Output = Result<bool, BackendError>> + Send;
    fn phase(&self, _phase: StartupPhase, _status: ContainerStatus) {}
}

fn remaining(deadline: Instant) -> Result<Duration, BackendError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| failure(BackendErrorCode::GuestStartupTimeout))
}

async fn readiness(driver: &impl StartupDriver, deadline: Instant) -> Result<(), BackendError> {
    remaining(deadline)?;
    let initial = driver.inspect(deadline).await?;
    if initial == ContainerStatus::Running
        && driver.health(deadline).await?
        && driver.inspect(deadline).await? == ContainerStatus::Running
    {
        driver.phase(StartupPhase::Online, initial);
        return Ok(());
    }
    if !matches!(
        initial,
        ContainerStatus::Running | ContainerStatus::Restarting
    ) {
        driver.phase(StartupPhase::StartingContainer, initial);
        driver.start(deadline).await?;
    }
    driver.phase(StartupPhase::WaitingForGuest, initial);
    loop {
        remaining(deadline)?;
        let status = driver.inspect(deadline).await?;
        driver.phase(StartupPhase::WaitingForGuest, status);
        if matches!(
            status,
            ContainerStatus::Exited | ContainerStatus::Dead | ContainerStatus::NotFound
        ) {
            return Err(failure(BackendErrorCode::ContainerExitedDuringStartup));
        }
        if status == ContainerStatus::Running && driver.health(deadline).await? {
            // Health can take seconds; an exit during that request must not report success.
            let latest = driver.inspect(deadline).await?;
            if latest == ContainerStatus::Running {
                driver.phase(StartupPhase::Online, latest);
                return Ok(());
            }
            if matches!(
                latest,
                ContainerStatus::Exited | ContainerStatus::Dead | ContainerStatus::NotFound
            ) {
                driver.phase(StartupPhase::WaitingForGuest, latest);
                return Err(failure(BackendErrorCode::ContainerExitedDuringStartup));
            }
        }
        tokio::time::sleep(remaining(deadline)?.min(Duration::from_secs(1))).await;
    }
}

struct ContainerDriver<'a> {
    config: &'a AppConfig,
}

impl ContainerDriver<'_> {
    async fn command(
        &self,
        args: &[&str],
        deadline: Instant,
    ) -> Result<process::CommandOutput, BackendError> {
        // Reserve the bounded process termination and pipe-drain allowance.
        let budget = remaining(deadline)?
            .checked_sub(Duration::from_millis(2250))
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failure(BackendErrorCode::GuestStartupTimeout))?;
        let mut command = Command::new(self.config.container_runtime.as_str());
        command.args(args);
        process::output(
            command,
            CommandPolicy::new(
                if matches!(args.first(), Some(&"start" | &"compose")) {
                    budget
                } else {
                    budget.min(Duration::from_secs(5))
                },
                64 * 1024,
            ),
            None,
            "Windows startup",
        )
        .await
        .map_err(|error| {
            failure(match error.kind() {
                CommandFailureKind::Timeout => BackendErrorCode::ExternalProcessTimeout,
                CommandFailureKind::Cancelled => BackendErrorCode::ExternalProcessCancelled,
                _ => BackendErrorCode::ExternalProcessInterrupted,
            })
        })
    }

    async fn inspection(
        &self,
        deadline: Instant,
    ) -> Result<Option<serde_json::Value>, BackendError> {
        let output = self
            .command(&["inspect", &self.config.container_name], deadline)
            .await?;
        if !output.status.success() {
            // Distinguish a missing container from an unavailable daemon without trusting stderr.
            let listed = self
                .command(
                    &["container", "ls", "--all", "--format", "{{.Names}}"],
                    deadline,
                )
                .await?;
            if listed.status.success()
                && !listed.stdout_truncated
                && !String::from_utf8_lossy(&listed.stdout)
                    .lines()
                    .any(|name| name == self.config.container_name)
            {
                return Ok(None);
            }
            return Err(failure(BackendErrorCode::OperationFailed));
        }
        let values: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|_| failure(BackendErrorCode::OperationFailed))?;
        if output.stdout_truncated || values.len() != 1 {
            return Err(failure(BackendErrorCode::OperationFailed));
        }
        Ok(values.into_iter().next())
    }
}

impl StartupDriver for ContainerDriver<'_> {
    async fn inspect(&self, deadline: Instant) -> Result<ContainerStatus, BackendError> {
        let Some(value) = self.inspection(deadline).await? else {
            return Ok(ContainerStatus::NotFound);
        };
        let status = value
            .pointer("/State/Status")
            .and_then(|v| v.as_str())
            .map(ContainerStatus::from_runtime)
            .filter(|status| *status != ContainerStatus::Unknown)
            .ok_or_else(|| failure(BackendErrorCode::OperationFailed))?;
        Ok(status)
    }

    async fn start(&self, deadline: Instant) -> Result<(), BackendError> {
        let output = if self.inspection(deadline).await?.is_some() {
            self.command(&["start", &self.config.container_name], deadline)
                .await?
        } else {
            let service = crate::config::winboat_compose_service_name(std::path::Path::new(
                &self.config.compose_file,
            ))
            .map_err(|_| failure(BackendErrorCode::PreconditionFailed))?;
            self.command(
                &[
                    "compose",
                    "-f",
                    &self.config.compose_file,
                    "up",
                    "-d",
                    &service,
                ],
                deadline,
            )
            .await?
        };
        if output.status.success() {
            Ok(())
        } else {
            Err(failure(BackendErrorCode::OperationFailed))
        }
    }

    async fn health(&self, deadline: Instant) -> Result<bool, BackendError> {
        let Some(value) = self.inspection(deadline).await? else {
            return Ok(false);
        };
        let bindings = value
            .pointer("/NetworkSettings/Ports/7148~1tcp")
            .and_then(|v| v.as_array());
        let api_url = match bindings {
            Some(bindings) if !bindings.is_empty() => {
                let binding = &bindings[0];
                let host = binding.get("HostIp").and_then(|v| v.as_str()).unwrap_or("");
                if bindings.len() != 1 || !matches!(host, "127.0.0.1" | "::1") {
                    return Err(failure(BackendErrorCode::PreconditionFailed));
                }
                let port = binding
                    .get("HostPort")
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<u16>().ok())
                    .filter(|port| *port != 0)
                    .ok_or_else(|| failure(BackendErrorCode::PreconditionFailed))?;
                format!(
                    "http://{}:{port}",
                    if host == "::1" { "[::1]" } else { host }
                )
            }
            _ => self.config.api_url.clone(),
        };
        let budget = remaining(deadline)?.min(Duration::from_secs(2));
        let client = super::container::http_client(budget)
            .map_err(|_| failure(BackendErrorCode::OperationFailed))?;
        Ok(client
            .get(format!("{api_url}/health"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success()))
    }

    fn phase(&self, phase: StartupPhase, status: ContainerStatus) {
        update(phase, status, None);
    }
}

pub async fn ensure_guest_online(config: &AppConfig) -> Result<(), BackendError> {
    let deadline =
        Instant::now() + Duration::from_secs(config.startup_timeout_seconds.clamp(1, 900));
    let _guard = tokio::time::timeout_at(
        deadline,
        START_GATE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock(),
    )
    .await
    .map_err(|_| failure(BackendErrorCode::GuestStartupTimeout))?;
    {
        let mut current = ATTEMPT.lock().unwrap_or_else(|e| e.into_inner());
        let id = current.as_ref().map_or(1, |(_, a)| a.id.saturating_add(1));
        *current = Some((
            identity(config),
            StartupAttempt {
                id,
                started_at: chrono::Utc::now().to_rfc3339(),
                phase: StartupPhase::StartingContainer,
                error_code: None,
                container_status: ContainerStatus::Unknown,
            },
        ));
    }
    let result = readiness(&ContainerDriver { config }, deadline).await;
    if let Err(error) = &result {
        let status = snapshot(config).map_or(ContainerStatus::Unknown, |a| a.container_status);
        update(StartupPhase::StartupFailed, status, Some(error.code));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture {
        states: Vec<ContainerStatus>,
        health_after: usize,
        inspections: AtomicUsize,
        starts: AtomicUsize,
        health_calls: AtomicUsize,
    }
    impl StartupDriver for Fixture {
        async fn inspect(&self, _: Instant) -> Result<ContainerStatus, BackendError> {
            let index = self.inspections.fetch_add(1, Ordering::SeqCst);
            Ok(self.states[index.min(self.states.len() - 1)])
        }
        async fn start(&self, _: Instant) -> Result<(), BackendError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn health(&self, _: Instant) -> Result<bool, BackendError> {
            Ok(self.health_calls.fetch_add(1, Ordering::SeqCst) >= self.health_after)
        }
    }
    fn fixture(states: Vec<ContainerStatus>, health_after: usize) -> Fixture {
        Fixture {
            states,
            health_after,
            inspections: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            health_calls: AtomicUsize::new(0),
        }
    }
    #[tokio::test]
    async fn delayed_health_is_the_only_success_condition() {
        let f = fixture(vec![ContainerStatus::Exited, ContainerStatus::Running], 1);
        readiness(&f, Instant::now() + Duration::from_secs(4))
            .await
            .unwrap();
        assert_eq!(f.starts.load(Ordering::SeqCst), 1);
        assert_eq!(f.health_calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn exited_and_dead_fail_without_waiting_for_timeout() {
        for terminal in [ContainerStatus::Exited, ContainerStatus::Dead] {
            let f = fixture(vec![ContainerStatus::Exited, terminal], usize::MAX);
            let started = Instant::now();
            let error = readiness(&f, started + Duration::from_secs(10))
                .await
                .unwrap_err();
            assert_eq!(error.code, BackendErrorCode::ContainerExitedDuringStartup);
            assert!(started.elapsed() < Duration::from_millis(100));
        }
    }
    #[tokio::test]
    async fn running_without_health_has_a_distinct_timeout() {
        let f = fixture(vec![ContainerStatus::Running], usize::MAX);
        let error = readiness(&f, Instant::now() + Duration::from_millis(30))
            .await
            .unwrap_err();
        assert_eq!(error.code, BackendErrorCode::GuestStartupTimeout);
        assert_eq!(f.starts.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn online_is_idempotent_and_exit_during_health_is_rejected() {
        let f = fixture(vec![ContainerStatus::Running], 0);
        readiness(&f, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(f.starts.load(Ordering::SeqCst), 0);
        let f = fixture(
            vec![
                ContainerStatus::Exited,
                ContainerStatus::Running,
                ContainerStatus::Dead,
            ],
            0,
        );
        assert_eq!(
            readiness(&f, Instant::now() + Duration::from_secs(1))
                .await
                .unwrap_err()
                .code,
            BackendErrorCode::ContainerExitedDuringStartup
        );
    }
}
