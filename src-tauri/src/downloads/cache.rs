use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const CACHE_METADATA_SCHEMA_VERSION: u32 = 1;
const MIN_INSTALLER_SIZE: u64 = 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct InstallerCacheMetadata {
    schema_version: u32,
    version: String,
    source_url: String,
    pub(super) size: u64,
    remote_content_length: Option<u64>,
    pub(super) sha256: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CacheInspection {
    Missing,
    Valid(InstallerCacheMetadata),
    Invalid(CacheValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CacheValidationError {
    InstallerNotFile,
    MetadataMissing,
    MetadataRead(String),
    MetadataInvalid(String),
    MetadataSchema(u32),
    VersionMismatch,
    SourceMismatch,
    TooSmall(u64),
    SizeMismatch { expected: u64, actual: u64 },
    PeHeader(&'static str),
    FileRead(String),
    HashMismatch,
}

impl fmt::Display for CacheValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InstallerNotFile => formatter.write_str("the cache path is not a regular file"),
            Self::MetadataMissing => formatter.write_str("cache metadata is missing"),
            Self::MetadataRead(error) => {
                write!(formatter, "cache metadata could not be read: {error}")
            }
            Self::MetadataInvalid(error) => write!(formatter, "cache metadata is invalid: {error}"),
            Self::MetadataSchema(version) => {
                write!(formatter, "cache metadata schema {version} is unsupported")
            }
            Self::VersionMismatch => formatter.write_str("the cached version does not match"),
            Self::SourceMismatch => formatter.write_str("the cached source URL does not match"),
            Self::TooSmall(size) => write!(formatter, "the installer is too small ({size} bytes)"),
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "the installer size changed (expected {expected} bytes, found {actual})"
            ),
            Self::PeHeader(reason) => {
                write!(formatter, "the Windows PE header is invalid: {reason}")
            }
            Self::FileRead(error) => write!(formatter, "the installer could not be read: {error}"),
            Self::HashMismatch => formatter.write_str("the installer SHA-256 does not match"),
        }
    }
}

pub(super) struct DownloadedInstaller {
    pub(super) size: u64,
    pub(super) sha256: String,
}

pub(super) struct RemoteMetadata {
    pub(super) content_length: Option<u64>,
    pub(super) etag: Option<String>,
    pub(super) last_modified: Option<String>,
}

pub(super) fn metadata_path(installer: &Path) -> PathBuf {
    append_suffix(installer, ".metadata.json")
}

pub(super) fn partial_path(installer: &Path) -> PathBuf {
    append_suffix(installer, ".download")
}

pub(super) async fn inspect(installer: &Path, version: &str, source_url: &str) -> CacheInspection {
    let installer_metadata = match tokio::fs::symlink_metadata(installer).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if tokio::fs::try_exists(metadata_path(installer))
                .await
                .unwrap_or(false)
            {
                CacheInspection::Invalid(CacheValidationError::InstallerNotFile)
            } else {
                CacheInspection::Missing
            };
        }
        Err(error) => {
            return CacheInspection::Invalid(CacheValidationError::FileRead(error.to_string()));
        }
    };
    if !installer_metadata.file_type().is_file() {
        return CacheInspection::Invalid(CacheValidationError::InstallerNotFile);
    }

    let metadata_file = metadata_path(installer);
    let serialized = match tokio::fs::read_to_string(&metadata_file).await {
        Ok(serialized) => serialized,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CacheInspection::Invalid(CacheValidationError::MetadataMissing);
        }
        Err(error) => {
            return CacheInspection::Invalid(CacheValidationError::MetadataRead(error.to_string()));
        }
    };
    let metadata = match serde_json::from_str::<InstallerCacheMetadata>(&serialized) {
        Ok(metadata) => metadata,
        Err(error) => {
            return CacheInspection::Invalid(CacheValidationError::MetadataInvalid(
                error.to_string(),
            ));
        }
    };
    if metadata.schema_version != CACHE_METADATA_SCHEMA_VERSION {
        return CacheInspection::Invalid(CacheValidationError::MetadataSchema(
            metadata.schema_version,
        ));
    }
    if metadata.version != version {
        return CacheInspection::Invalid(CacheValidationError::VersionMismatch);
    }
    if metadata.source_url != source_url {
        return CacheInspection::Invalid(CacheValidationError::SourceMismatch);
    }
    let expected_size = metadata.remote_content_length.unwrap_or(metadata.size);
    match validate_payload(installer, Some(expected_size), Some(&metadata.sha256)).await {
        Ok(validated) if validated.size == metadata.size => CacheInspection::Valid(metadata),
        Ok(validated) => CacheInspection::Invalid(CacheValidationError::SizeMismatch {
            expected: metadata.size,
            actual: validated.size,
        }),
        Err(error) => CacheInspection::Invalid(error),
    }
}

pub(super) async fn validate_download(
    installer: &Path,
    expected_size: Option<u64>,
) -> Result<DownloadedInstaller, CacheValidationError> {
    validate_payload(installer, expected_size, None).await
}

pub(super) fn metadata_for_download(
    version: &str,
    source_url: &str,
    downloaded: &DownloadedInstaller,
    remote: RemoteMetadata,
) -> InstallerCacheMetadata {
    InstallerCacheMetadata {
        schema_version: CACHE_METADATA_SCHEMA_VERSION,
        version: version.to_string(),
        source_url: source_url.to_string(),
        size: downloaded.size,
        remote_content_length: remote.content_length,
        sha256: downloaded.sha256.clone(),
        etag: remote.etag,
        last_modified: remote.last_modified,
    }
}

pub(super) async fn commit(
    partial: &Path,
    destination: &Path,
    metadata: &InstallerCacheMetadata,
) -> Result<(), String> {
    let destination_metadata = metadata_path(destination);
    let metadata_partial = append_suffix(&destination_metadata, ".download");
    let installer_backup = append_suffix(destination, ".previous");
    let metadata_backup = append_suffix(&destination_metadata, ".previous");

    remove_if_exists(&metadata_partial).await?;
    remove_if_exists(&installer_backup).await?;
    remove_if_exists(&metadata_backup).await?;

    let serialized = serde_json::to_vec_pretty(metadata).map_err(|error| error.to_string())?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&metadata_partial)
        .await
        .map_err(|error| error.to_string())?;
    file.write_all(&serialized)
        .await
        .map_err(|error| error.to_string())?;
    file.flush().await.map_err(|error| error.to_string())?;
    file.sync_all().await.map_err(|error| error.to_string())?;
    drop(file);

    let had_installer = move_to_backup(destination, &installer_backup).await?;
    if let Err(error) = tokio::fs::rename(partial, destination).await {
        if had_installer {
            let _ = tokio::fs::rename(&installer_backup, destination).await;
        }
        let _ = remove_if_exists(&metadata_partial).await;
        return Err(error.to_string());
    }

    let had_metadata = match move_to_backup(&destination_metadata, &metadata_backup).await {
        Ok(value) => value,
        Err(error) => {
            rollback_installer(destination, &installer_backup, had_installer).await;
            let _ = remove_if_exists(&metadata_partial).await;
            return Err(error);
        }
    };
    if let Err(error) = tokio::fs::rename(&metadata_partial, &destination_metadata).await {
        rollback_installer(destination, &installer_backup, had_installer).await;
        if had_metadata {
            let _ = tokio::fs::rename(&metadata_backup, &destination_metadata).await;
        }
        let _ = remove_if_exists(&metadata_partial).await;
        return Err(error.to_string());
    }

    remove_if_exists(&installer_backup).await?;
    remove_if_exists(&metadata_backup).await?;
    Ok(())
}

pub(super) async fn discard(installer: &Path) -> Result<(), String> {
    for path in [
        installer.to_path_buf(),
        metadata_path(installer),
        partial_path(installer),
        append_suffix(installer, ".previous"),
        append_suffix(&metadata_path(installer), ".download"),
        append_suffix(&metadata_path(installer), ".previous"),
    ] {
        remove_if_exists(&path).await?;
    }
    Ok(())
}

async fn validate_payload(
    installer: &Path,
    expected_size: Option<u64>,
    expected_hash: Option<&str>,
) -> Result<DownloadedInstaller, CacheValidationError> {
    let filesystem_metadata = tokio::fs::symlink_metadata(installer)
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    if !filesystem_metadata.file_type().is_file() {
        return Err(CacheValidationError::InstallerNotFile);
    }
    let size = filesystem_metadata.len();
    if size < MIN_INSTALLER_SIZE {
        return Err(CacheValidationError::TooSmall(size));
    }
    if let Some(expected) = expected_size {
        if size != expected {
            return Err(CacheValidationError::SizeMismatch {
                expected,
                actual: size,
            });
        }
    }
    validate_pe(installer, size).await?;
    let sha256 = sha256(installer).await?;
    if expected_hash.is_some_and(|expected| !sha256.eq_ignore_ascii_case(expected)) {
        return Err(CacheValidationError::HashMismatch);
    }
    Ok(DownloadedInstaller { size, sha256 })
}

async fn validate_pe(installer: &Path, size: u64) -> Result<(), CacheValidationError> {
    let mut file = tokio::fs::File::open(installer)
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut dos_header = [0_u8; 64];
    file.read_exact(&mut dos_header)
        .await
        .map_err(|_| CacheValidationError::PeHeader("truncated DOS header"))?;
    if &dos_header[..2] != b"MZ" {
        return Err(CacheValidationError::PeHeader("missing MZ signature"));
    }
    let pe_offset = u32::from_le_bytes(
        dos_header[0x3c..0x40]
            .try_into()
            .expect("the DOS header offset is four bytes"),
    ) as u64;
    if pe_offset < dos_header.len() as u64 || pe_offset.saturating_add(6) > size {
        return Err(CacheValidationError::PeHeader("invalid PE offset"));
    }
    file.seek(std::io::SeekFrom::Start(pe_offset))
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut pe_header = [0_u8; 6];
    file.read_exact(&mut pe_header)
        .await
        .map_err(|_| CacheValidationError::PeHeader("truncated PE header"))?;
    if &pe_header[..4] != b"PE\0\0" {
        return Err(CacheValidationError::PeHeader("missing PE signature"));
    }
    let machine = u16::from_le_bytes([pe_header[4], pe_header[5]]);
    if !matches!(machine, 0x014c | 0x8664 | 0xaa64) {
        return Err(CacheValidationError::PeHeader("unsupported machine type"));
    }
    Ok(())
}

async fn sha256(path: &Path) -> Result<String, CacheValidationError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

async fn move_to_backup(path: &Path, backup: &Path) -> Result<bool, String> {
    match tokio::fs::rename(path, backup).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

async fn rollback_installer(destination: &Path, backup: &Path, had_installer: bool) {
    let _ = remove_if_exists(destination).await;
    if had_installer {
        let _ = tokio::fs::rename(backup, destination).await;
    }
}

async fn remove_if_exists(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Seek, SeekFrom, Write};

    fn write_pe(path: &Path, size: u64, machine: u16) {
        let mut file = fs::File::create(path).expect("create PE fixture");
        file.set_len(size).expect("size PE fixture");
        file.seek(SeekFrom::Start(0)).expect("seek DOS header");
        file.write_all(b"MZ").expect("DOS signature");
        file.seek(SeekFrom::Start(0x3c)).expect("seek PE offset");
        file.write_all(&0x80_u32.to_le_bytes()).expect("PE offset");
        file.seek(SeekFrom::Start(0x80)).expect("seek PE header");
        file.write_all(b"PE\0\0").expect("PE signature");
        file.write_all(&machine.to_le_bytes())
            .expect("machine type");
        file.flush().expect("flush fixture");
    }

    async fn create_cache(
        directory: &Path,
        version: &str,
        source_url: &str,
    ) -> (PathBuf, InstallerCacheMetadata) {
        let installer = directory.join(format!("Mendix-{version}-Setup.exe"));
        write_pe(&installer, MIN_INSTALLER_SIZE + 4096, 0x8664);
        let downloaded = validate_download(&installer, None)
            .await
            .expect("valid fixture");
        let metadata = metadata_for_download(
            version,
            source_url,
            &downloaded,
            RemoteMetadata {
                content_length: Some(downloaded.size),
                etag: Some("fixture-etag".to_string()),
                last_modified: None,
            },
        );
        fs::write(
            metadata_path(&installer),
            serde_json::to_vec_pretty(&metadata).expect("serialize metadata"),
        )
        .expect("write metadata");
        (installer, metadata)
    }

    #[tokio::test]
    async fn reuses_only_a_complete_pe_with_matching_metadata_and_hash() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (installer, metadata) = create_cache(
            temporary.path(),
            "11.12.2",
            "https://example.test/installer",
        )
        .await;

        assert_eq!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Valid(metadata)
        );
    }

    #[tokio::test]
    async fn rejects_an_arbitrary_file_larger_than_one_mebibyte() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        fs::write(&installer, vec![b'X'; MIN_INSTALLER_SIZE as usize + 1])
            .expect("write arbitrary payload");

        assert!(matches!(
            validate_download(&installer, None).await,
            Err(CacheValidationError::PeHeader("missing MZ signature"))
        ));
    }

    #[tokio::test]
    async fn rejects_truncated_and_unsupported_pe_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let truncated = temporary.path().join("truncated.exe");
        fs::write(&truncated, b"MZ").expect("write truncated fixture");
        assert!(matches!(
            validate_download(&truncated, None).await,
            Err(CacheValidationError::TooSmall(2))
        ));

        let unsupported = temporary.path().join("unsupported.exe");
        write_pe(&unsupported, MIN_INSTALLER_SIZE + 1, 0xffff);
        assert!(matches!(
            validate_download(&unsupported, None).await,
            Err(CacheValidationError::PeHeader("unsupported machine type"))
        ));
    }

    #[tokio::test]
    async fn rejects_missing_or_corrupt_cache_metadata() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        write_pe(&installer, MIN_INSTALLER_SIZE + 1, 0x014c);
        assert_eq!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::MetadataMissing)
        );

        fs::write(metadata_path(&installer), b"not json").expect("write bad metadata");
        assert!(matches!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::MetadataInvalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_version_source_size_and_hash_changes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (installer, _) = create_cache(
            temporary.path(),
            "11.12.2",
            "https://example.test/installer",
        )
        .await;

        assert_eq!(
            inspect(&installer, "11.13.0", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::VersionMismatch)
        );
        assert_eq!(
            inspect(&installer, "11.12.2", "https://example.test/other").await,
            CacheInspection::Invalid(CacheValidationError::SourceMismatch)
        );

        let original_size = fs::metadata(&installer).expect("fixture metadata").len();
        fs::OpenOptions::new()
            .append(true)
            .open(&installer)
            .expect("open fixture")
            .write_all(b"changed size")
            .expect("append fixture");
        assert!(matches!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::SizeMismatch {
                expected,
                actual
            }) if expected == original_size && actual > expected
        ));

        let (installer, _) = create_cache(
            temporary.path(),
            "11.12.2",
            "https://example.test/installer",
        )
        .await;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&installer)
            .expect("open fixture");
        file.seek(SeekFrom::Start(4096)).expect("seek payload");
        file.write_all(b"same length mutation")
            .expect("mutate payload");
        file.flush().expect("flush mutation");
        assert_eq!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::HashMismatch)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symbolic_link_cache_entries() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let target = temporary.path().join("target.exe");
        write_pe(&target, MIN_INSTALLER_SIZE + 1, 0x8664);
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        symlink(&target, &installer).expect("create symlink");

        assert_eq!(
            inspect(&installer, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::InstallerNotFile)
        );
    }

    #[tokio::test]
    async fn commit_replaces_installer_and_metadata_as_one_valid_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let partial = partial_path(&destination);
        write_pe(&destination, MIN_INSTALLER_SIZE + 1, 0x014c);
        fs::write(metadata_path(&destination), b"old metadata").expect("old metadata");
        write_pe(&partial, MIN_INSTALLER_SIZE + 8192, 0x8664);
        let downloaded = validate_download(&partial, None)
            .await
            .expect("valid partial");
        let metadata = metadata_for_download(
            "11.12.2",
            "https://example.test/installer",
            &downloaded,
            RemoteMetadata {
                content_length: Some(downloaded.size),
                etag: None,
                last_modified: None,
            },
        );

        commit(&partial, &destination, &metadata)
            .await
            .expect("commit cache");

        assert!(!partial.exists());
        assert_eq!(
            inspect(&destination, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Valid(metadata)
        );
    }

    #[tokio::test]
    async fn discard_removes_payload_metadata_and_interrupted_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        for path in [
            installer.clone(),
            metadata_path(&installer),
            partial_path(&installer),
            append_suffix(&installer, ".previous"),
            append_suffix(&metadata_path(&installer), ".download"),
            append_suffix(&metadata_path(&installer), ".previous"),
        ] {
            fs::write(path, b"fixture").expect("write cache artifact");
        }

        discard(&installer).await.expect("discard cache");

        assert!(!installer.exists());
        assert!(!metadata_path(&installer).exists());
        assert!(!partial_path(&installer).exists());
    }
}
