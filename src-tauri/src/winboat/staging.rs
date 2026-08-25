use crate::downloads::storage::{open_secure_file, SecureDirectory, SecureTemporaryFile};
use crate::downloads::MAX_INSTALLER_BYTES;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const STAGING_DIRECTORY: &str = ".mendimaru/installers";
const COPY_BUFFER_SIZE: usize = 128 * 1024;

pub(crate) struct StagedInstaller {
    temporary: SecureTemporaryFile,
}

impl StagedInstaller {
    pub(crate) fn path(&self) -> &Path {
        self.temporary.path()
    }
}

pub(crate) async fn stage_installer(
    shared_directory: &Path,
    installer_path: &Path,
    expected_sha256: &str,
) -> Result<StagedInstaller, String> {
    validate_sha256(expected_sha256)?;
    let mut source = open_secure_file(installer_path)?;
    let source_size = source
        .metadata()
        .await
        .map_err(|error| format!("could not inspect the private installer cache: {error}"))?
        .len();
    if source_size > MAX_INSTALLER_BYTES {
        return Err(format!(
            "the private installer exceeds the {MAX_INSTALLER_BYTES}-byte limit"
        ));
    }

    let staging_directory =
        SecureDirectory::open_or_create(&shared_directory.join(STAGING_DIRECTORY))?;
    let mut staged = staging_directory.create_random_file("mendimaru-installer-stage-", ".exe")?;
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read the private installer cache: {error}"))?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .filter(|size| *size <= MAX_INSTALLER_BYTES)
            .ok_or_else(|| {
                format!("the private installer exceeds the {MAX_INSTALLER_BYTES}-byte limit")
            })?;
        digest.update(&buffer[..read]);
        staged
            .file_mut()
            .write_all(&buffer[..read])
            .await
            .map_err(|error| {
                format!("could not write the shared installer staging file: {error}")
            })?;
    }
    if copied != source_size {
        return Err("the private installer changed size while it was staged".to_string());
    }
    let copied_sha256 = format!("{:x}", digest.finalize());
    if !copied_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err("the private installer SHA-256 changed before staging".to_string());
    }

    staged
        .file_mut()
        .flush()
        .await
        .map_err(|error| format!("could not flush the shared installer staging file: {error}"))?;
    staged
        .file_mut()
        .sync_all()
        .await
        .map_err(|error| format!("could not sync the shared installer staging file: {error}"))?;
    verify_staged_descriptor(&mut staged, copied, expected_sha256).await?;
    staged.sync_parent()?;
    Ok(StagedInstaller { temporary: staged })
}

async fn verify_staged_descriptor(
    staged: &mut SecureTemporaryFile,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), String> {
    staged
        .file_mut()
        .seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| format!("could not seek the shared installer staging file: {error}"))?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let read = staged.file_mut().read(&mut buffer).await.map_err(|error| {
            format!("could not verify the shared installer staging file: {error}")
        })?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .filter(|value| *value <= MAX_INSTALLER_BYTES)
            .ok_or_else(|| "the shared installer staging file exceeded its limit".to_string())?;
        digest.update(&buffer[..read]);
    }
    let sha256 = format!("{:x}", digest.finalize());
    if size != expected_size || !sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(
            "the shared installer staging file failed its host integrity check".to_string(),
        );
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("the expected installer SHA-256 is invalid".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write_source(root: &Path, content: &[u8]) -> (PathBuf, String) {
        let directory = root.join("private/cache/installers");
        fs::create_dir_all(&directory).expect("private cache directory");
        let path = directory.join("Mendix-11.12.2-Setup.exe");
        fs::write(&path, content).expect("private installer");
        (path, format!("{:x}", Sha256::digest(content)))
    }

    #[tokio::test]
    async fn stages_a_unique_verified_copy_and_cleans_it_on_drop() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let shared = temporary.path().join("shared");
        fs::create_dir(&shared).expect("shared directory");
        let content = vec![0x5a; 512 * 1024];
        let (source, sha256) = write_source(temporary.path(), &content);

        let first = stage_installer(&shared, &source, &sha256)
            .await
            .expect("first staged installer");
        let second = stage_installer(&shared, &source, &sha256)
            .await
            .expect("second staged installer");
        assert_ne!(first.path(), second.path());
        assert_eq!(fs::read(first.path()).expect("first staged bytes"), content);
        let first_path = first.path().to_path_buf();
        let second_path = second.path().to_path_buf();
        drop(first);
        drop(second);
        assert!(!first_path.exists());
        assert!(!second_path.exists());
    }

    #[tokio::test]
    async fn rejects_a_changed_source_digest_without_retaining_staging() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let shared = temporary.path().join("shared");
        fs::create_dir(&shared).expect("shared directory");
        let (source, _) = write_source(temporary.path(), b"changed source");
        let wrong_sha256 = format!("{:x}", Sha256::digest(b"expected source"));

        assert!(stage_installer(&shared, &source, &wrong_sha256)
            .await
            .is_err());
        assert_eq!(
            fs::read_dir(shared.join(STAGING_DIRECTORY))
                .expect("staging directory")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_linked_source_and_staging_ancestors_without_touching_sentinels() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let shared = temporary.path().join("shared");
        let outside = temporary.path().join("outside");
        let private = temporary.path().join("private");
        fs::create_dir(&shared).expect("shared directory");
        fs::create_dir(&outside).expect("outside directory");
        fs::create_dir(&private).expect("private directory");
        let sentinel = outside.join("sentinel");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        let linked_source = private.join("linked.exe");
        symlink(&sentinel, &linked_source).expect("source symlink");
        let sha256 = format!("{:x}", Sha256::digest(b"unchanged"));
        assert!(stage_installer(&shared, &linked_source, &sha256)
            .await
            .is_err());

        symlink(&outside, shared.join(".mendimaru")).expect("staging ancestor symlink");
        let source = private.join("installer.exe");
        fs::write(&source, b"installer").expect("source installer");
        let source_sha256 = format!("{:x}", Sha256::digest(b"installer"));
        assert!(stage_installer(&shared, &source, &source_sha256)
            .await
            .is_err());
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");
        assert!(!outside.join("installers").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_a_linked_installers_directory_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let shared = temporary.path().join("shared");
        let app_directory = shared.join(".mendimaru");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&app_directory).expect("shared app directory");
        fs::create_dir(&outside).expect("outside directory");
        symlink(&outside, app_directory.join("installers")).expect("installers symlink");
        let (source, sha256) = write_source(temporary.path(), b"installer");

        assert!(stage_installer(&shared, &source, &sha256).await.is_err());
        assert_eq!(
            fs::read_dir(&outside).expect("outside directory").count(),
            0
        );
    }
}
