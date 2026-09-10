//! Serialize Runtime stops across processes sharing a Compose directory.
use crate::models::AppConfig;
use std::fs::File;
use std::time::Duration;

const LOCK_NAME: &str = ".mendimaru-runtime-stop.lock";
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_WAIT: Duration = Duration::from_secs(3_600);

pub(super) async fn acquire(config: &AppConfig) -> Result<File, ()> {
    let file = super::super::maintenance::open_lock(config, LOCK_NAME).map_err(|_| ())?;
    wait(file, MAX_WAIT).await
}

async fn wait(file: File, timeout: Duration) -> Result<File, ()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(()),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(());
        }
        // Async polling keeps the CLI deadline/cancellation effective. Never
        // unlink the file: waiters must continue to lock the same inode. Closing
        // the descriptor releases ownership on success, error, or process exit.
        tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + POLL_INTERVAL)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_lock_wait_is_bounded_and_cancellation_releases_the_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stop.lock");
        let open = || {
            File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap()
        };
        let owner = wait(open(), Duration::ZERO).await.unwrap();
        let started = tokio::time::Instant::now();
        assert!(wait(open(), Duration::from_millis(75)).await.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), wait(open(), MAX_WAIT))
                .await
                .is_err()
        );
        drop(owner);
        let next = wait(open(), Duration::ZERO).await.unwrap();
        drop(next);
        assert!(
            path.exists(),
            "the shared lock inode must never be unlinked"
        );
    }
}
