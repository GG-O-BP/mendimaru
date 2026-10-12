//! Serialize Runtime stops across processes sharing one WinBoat VM identity.
//! The lock lives in Mendimaru's private coordination namespace (#225).
use crate::models::AppConfig;
use crate::winboat::maintenance::{Lease, LockError};
use std::time::Duration;

const MAX_WAIT: Duration = Duration::from_secs(3_600);

pub(super) async fn acquire(config: &AppConfig) -> Result<Lease, LockError> {
    // Async polling keeps the CLI deadline/cancellation effective. Never
    // unlink the file: waiters must continue to lock the same inode. Closing
    // the descriptor releases ownership on success, error, or process exit.
    super::super::maintenance::runtime_stop(config, MAX_WAIT).await
}

#[cfg(test)]
mod tests {
    use crate::winboat::maintenance::linux::wait_exclusive;
    use crate::winboat::maintenance::LockError;
    use std::fs::File;
    use std::time::Duration;

    #[tokio::test]
    async fn stop_lock_wait_is_bounded_and_cancellation_releases_the_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stop.lock");
        let open = || {
            vec![File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap()]
        };
        let owner = wait_exclusive(open(), Duration::ZERO).await.unwrap();
        let started = tokio::time::Instant::now();
        assert_eq!(
            wait_exclusive(open(), Duration::from_millis(75))
                .await
                .unwrap_err(),
            LockError::Timeout
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            wait_exclusive(open(), Duration::from_secs(3_600))
        )
        .await
        .is_err());
        drop(owner);
        let next = wait_exclusive(open(), Duration::ZERO).await.unwrap();
        drop(next);
        assert!(
            path.exists(),
            "the shared lock inode must never be unlinked"
        );
    }
}
