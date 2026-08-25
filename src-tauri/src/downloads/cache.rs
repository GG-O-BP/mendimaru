use super::storage::{SecureDirectory, SecureTemporaryFile};
use crate::app_paths::AppPaths;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const CACHE_METADATA_SCHEMA_VERSION: u32 = 1;
const MIN_INSTALLER_SIZE: u64 = 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 128 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const INSTALLER_CACHE_DIRECTORY: &str = "installers";

pub(super) struct InstallerCache {
    directory: SecureDirectory,
    installer_name: String,
    metadata_name: String,
}

impl InstallerCache {
    pub(super) fn open(paths: &AppPaths, version: &str) -> Result<Self, String> {
        Self::open_in(
            paths.cache_directory().join(INSTALLER_CACHE_DIRECTORY),
            version,
        )
    }

    #[cfg(test)]
    pub(super) fn open_for_tests(directory: &Path, version: &str) -> Result<Self, String> {
        Self::open_in(directory.to_path_buf(), version)
    }

    fn open_in(directory: PathBuf, version: &str) -> Result<Self, String> {
        let installer_name = format!("Mendix-{version}-Setup.exe");
        let metadata_name = format!("{installer_name}.metadata.json");
        let directory = SecureDirectory::open_or_create(&directory)?;
        // Validate both names before retaining the cache so a caller can never
        // turn a version into a path traversal, even if it skipped the public
        // version validator.
        directory.path_for(&installer_name)?;
        directory.path_for(&metadata_name)?;
        Ok(Self {
            directory,
            installer_name,
            metadata_name,
        })
    }

    pub(super) fn installer_path(&self) -> Result<PathBuf, String> {
        self.directory.path_for(&self.installer_name)
    }

    pub(super) fn create_payload(&self) -> Result<SecureTemporaryFile, String> {
        self.directory
            .create_random_file("mendimaru-installer-", ".download")
    }

    #[cfg(test)]
    pub(super) fn metadata_path(&self) -> Result<PathBuf, String> {
        self.directory.path_for(&self.metadata_name)
    }
}

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

pub(super) async fn inspect(
    cache: &InstallerCache,
    version: &str,
    source_url: &str,
) -> CacheInspection {
    let installer_metadata = match cache.directory.symlink_metadata(&cache.installer_name) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => {
            return match cache.directory.symlink_metadata(&cache.metadata_name) {
                Ok(Some(_)) => CacheInspection::Invalid(CacheValidationError::InstallerNotFile),
                Ok(None) => CacheInspection::Missing,
                Err(error) => {
                    CacheInspection::Invalid(CacheValidationError::FileRead(error.to_string()))
                }
            }
        }
        Err(error) => {
            return CacheInspection::Invalid(CacheValidationError::FileRead(error.to_string()))
        }
    };
    if installer_metadata.file_type().is_symlink() || !installer_metadata.is_file() {
        return CacheInspection::Invalid(CacheValidationError::InstallerNotFile);
    }

    let serialized = match read_metadata(cache).await {
        Ok(serialized) => serialized,
        Err(error) => return CacheInspection::Invalid(error),
    };
    let metadata = match serde_json::from_slice::<InstallerCacheMetadata>(&serialized) {
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
    let mut installer = match cache.directory.open_regular_file(&cache.installer_name) {
        Ok(installer) => installer,
        Err(error) => {
            return CacheInspection::Invalid(CacheValidationError::FileRead(error.to_string()))
        }
    };
    match validate_open_payload(&mut installer, Some(expected_size), Some(&metadata.sha256)).await {
        Ok(validated) if validated.size == metadata.size => CacheInspection::Valid(metadata),
        Ok(validated) => CacheInspection::Invalid(CacheValidationError::SizeMismatch {
            expected: metadata.size,
            actual: validated.size,
        }),
        Err(error) => CacheInspection::Invalid(error),
    }
}

async fn read_metadata(cache: &InstallerCache) -> Result<Vec<u8>, CacheValidationError> {
    let metadata = match cache.directory.symlink_metadata(&cache.metadata_name) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return Err(CacheValidationError::MetadataMissing),
        Err(error) => return Err(CacheValidationError::MetadataRead(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CacheValidationError::MetadataRead(
            "the metadata path is not a direct regular file".to_string(),
        ));
    }
    if metadata.len() > MAX_METADATA_BYTES {
        return Err(CacheValidationError::MetadataRead(
            "the metadata exceeds its safe size limit".to_string(),
        ));
    }
    let file = cache
        .directory
        .open_regular_file(&cache.metadata_name)
        .map_err(CacheValidationError::MetadataRead)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| CacheValidationError::MetadataRead(error.to_string()))?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(CacheValidationError::MetadataRead(
            "the metadata exceeds its safe size limit".to_string(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
pub(super) async fn validate_download(
    installer: &Path,
    expected_size: Option<u64>,
) -> Result<DownloadedInstaller, CacheValidationError> {
    let parent = installer.parent().ok_or_else(|| {
        CacheValidationError::FileRead("the installer has no parent directory".to_string())
    })?;
    let name = installer
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CacheValidationError::FileRead("the installer name is not valid UTF-8".to_string())
        })?;
    let directory =
        SecureDirectory::open_existing(parent).map_err(CacheValidationError::FileRead)?;
    let metadata = directory
        .symlink_metadata(name)
        .map_err(CacheValidationError::FileRead)?
        .ok_or_else(|| CacheValidationError::FileRead("the installer is missing".to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CacheValidationError::InstallerNotFile);
    }
    let mut file = directory
        .open_regular_file(name)
        .map_err(CacheValidationError::FileRead)?;
    validate_open_payload(&mut file, expected_size, None).await
}

pub(super) async fn validate_temporary_download(
    partial: &mut SecureTemporaryFile,
    expected_size: Option<u64>,
) -> Result<DownloadedInstaller, CacheValidationError> {
    validate_open_payload(partial.file_mut(), expected_size, None).await
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
    cache: &InstallerCache,
    partial: &mut SecureTemporaryFile,
    metadata: &InstallerCacheMetadata,
) -> Result<(), String> {
    let serialized = serde_json::to_vec_pretty(metadata).map_err(|error| error.to_string())?;
    if serialized.len() as u64 > MAX_METADATA_BYTES {
        return Err("installer cache metadata exceeds its safe size limit".to_string());
    }
    let mut metadata_partial = cache
        .directory
        .create_random_file("mendimaru-metadata-", ".tmp")?;
    metadata_partial
        .file_mut()
        .write_all(&serialized)
        .await
        .map_err(|error| error.to_string())?;
    metadata_partial
        .file_mut()
        .flush()
        .await
        .map_err(|error| error.to_string())?;
    metadata_partial
        .file_mut()
        .sync_all()
        .await
        .map_err(|error| error.to_string())?;

    ensure_optional_regular(&cache.directory, &cache.installer_name)?;
    ensure_optional_regular(&cache.directory, &cache.metadata_name)?;
    let installer_backup = cache
        .directory
        .random_unused_name("mendimaru-installer-", ".backup")?;
    let metadata_backup = cache
        .directory
        .random_unused_name("mendimaru-metadata-", ".backup")?;
    let had_installer = move_to_backup(&cache.directory, &cache.installer_name, &installer_backup)?;
    if let Err(error) = cache
        .directory
        .rename(partial.name(), &cache.installer_name)
    {
        if had_installer {
            let _ = cache
                .directory
                .rename(&installer_backup, &cache.installer_name);
        }
        return Err(error);
    }
    partial.disarm();

    let had_metadata =
        match move_to_backup(&cache.directory, &cache.metadata_name, &metadata_backup) {
            Ok(value) => value,
            Err(error) => {
                rollback_installer(cache, &installer_backup, had_installer);
                return Err(error);
            }
        };
    if let Err(error) = cache
        .directory
        .rename(metadata_partial.name(), &cache.metadata_name)
    {
        rollback_installer(cache, &installer_backup, had_installer);
        if had_metadata {
            let _ = cache
                .directory
                .rename(&metadata_backup, &cache.metadata_name);
        }
        return Err(error);
    }
    metadata_partial.disarm();

    cache.directory.remove_file(&installer_backup)?;
    cache.directory.remove_file(&metadata_backup)?;
    cache.directory.sync()?;
    Ok(())
}

pub(super) async fn discard(cache: &InstallerCache) -> Result<(), String> {
    for name in [&cache.installer_name, &cache.metadata_name] {
        cache.directory.remove_file(name)?;
    }
    cache.directory.sync()?;
    Ok(())
}

async fn validate_open_payload(
    installer: &mut tokio::fs::File,
    expected_size: Option<u64>,
    expected_hash: Option<&str>,
) -> Result<DownloadedInstaller, CacheValidationError> {
    let filesystem_metadata = installer
        .metadata()
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    if !filesystem_metadata.is_file() {
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

async fn validate_pe(
    installer: &mut tokio::fs::File,
    size: u64,
) -> Result<(), CacheValidationError> {
    installer
        .seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut dos_header = [0_u8; 64];
    installer
        .read_exact(&mut dos_header)
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
    installer
        .seek(std::io::SeekFrom::Start(pe_offset))
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut pe_header = [0_u8; 6];
    installer
        .read_exact(&mut pe_header)
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

async fn sha256(installer: &mut tokio::fs::File) -> Result<String, CacheValidationError> {
    installer
        .seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| CacheValidationError::FileRead(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = installer
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

fn ensure_optional_regular(directory: &SecureDirectory, name: &str) -> Result<bool, String> {
    match directory.symlink_metadata(name)? {
        None => Ok(false),
        Some(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Some(_) => Err("an installer cache entry is not a direct regular file".to_string()),
    }
}

fn move_to_backup(directory: &SecureDirectory, source: &str, backup: &str) -> Result<bool, String> {
    if !ensure_optional_regular(directory, source)? {
        return Ok(false);
    }
    directory.rename(source, backup)?;
    Ok(true)
}

fn rollback_installer(cache: &InstallerCache, backup: &str, had_installer: bool) {
    let _ = cache.directory.remove_file(&cache.installer_name);
    if had_installer {
        let _ = cache.directory.rename(backup, &cache.installer_name);
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

    async fn write_temporary_pe(temporary: &mut SecureTemporaryFile, size: u64, machine: u16) {
        temporary
            .file_mut()
            .set_len(size)
            .await
            .expect("size PE fixture");
        temporary
            .file_mut()
            .seek(SeekFrom::Start(0))
            .await
            .expect("seek DOS header");
        temporary
            .file_mut()
            .write_all(b"MZ")
            .await
            .expect("DOS signature");
        temporary
            .file_mut()
            .seek(SeekFrom::Start(0x3c))
            .await
            .expect("seek PE offset");
        temporary
            .file_mut()
            .write_all(&0x80_u32.to_le_bytes())
            .await
            .expect("PE offset");
        temporary
            .file_mut()
            .seek(SeekFrom::Start(0x80))
            .await
            .expect("seek PE header");
        temporary
            .file_mut()
            .write_all(b"PE\0\0")
            .await
            .expect("PE signature");
        temporary
            .file_mut()
            .write_all(&machine.to_le_bytes())
            .await
            .expect("machine type");
        temporary.file_mut().sync_all().await.expect("sync fixture");
    }

    async fn create_cache(
        directory: &Path,
        version: &str,
        source_url: &str,
    ) -> (InstallerCache, PathBuf, InstallerCacheMetadata) {
        let cache = InstallerCache::open_for_tests(directory, version).expect("installer cache");
        let mut partial = cache.create_payload().expect("temporary installer");
        write_temporary_pe(&mut partial, MIN_INSTALLER_SIZE + 4096, 0x8664).await;
        let downloaded = validate_temporary_download(&mut partial, None)
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
        commit(&cache, &mut partial, &metadata)
            .await
            .expect("commit fixture cache");
        let installer = cache.installer_path().expect("installer path");
        (cache, installer, metadata)
    }

    #[tokio::test]
    async fn reuses_only_a_complete_pe_with_matching_metadata_and_hash() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (cache, _, metadata) = create_cache(
            temporary.path(),
            "11.12.2",
            "https://example.test/installer",
        )
        .await;

        assert_eq!(
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
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
        let cache =
            InstallerCache::open_for_tests(temporary.path(), "11.12.2").expect("installer cache");
        let installer = cache.installer_path().expect("installer path");
        write_pe(&installer, MIN_INSTALLER_SIZE + 1, 0x014c);
        assert_eq!(
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::MetadataMissing)
        );

        fs::write(cache.metadata_path().expect("metadata path"), b"not json")
            .expect("write bad metadata");
        assert!(matches!(
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::MetadataInvalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_version_source_size_and_hash_changes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (cache, installer, _) = create_cache(
            temporary.path(),
            "11.12.2",
            "https://example.test/installer",
        )
        .await;

        assert_eq!(
            inspect(&cache, "11.13.0", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::VersionMismatch)
        );
        assert_eq!(
            inspect(&cache, "11.12.2", "https://example.test/other").await,
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
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::SizeMismatch {
                expected,
                actual
            }) if expected == original_size && actual > expected
        ));

        let (cache, installer, _) = create_cache(
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
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
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
        let cache =
            InstallerCache::open_for_tests(temporary.path(), "11.12.2").expect("installer cache");
        let installer = cache.installer_path().expect("installer path");
        symlink(&target, &installer).expect("create symlink");

        assert_eq!(
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Invalid(CacheValidationError::InstallerNotFile)
        );
    }

    #[tokio::test]
    async fn commit_replaces_installer_and_metadata_as_one_valid_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let cache =
            InstallerCache::open_for_tests(temporary.path(), "11.12.2").expect("installer cache");
        let destination = cache.installer_path().expect("installer path");
        write_pe(&destination, MIN_INSTALLER_SIZE + 1, 0x014c);
        fs::write(
            cache.metadata_path().expect("metadata path"),
            b"old metadata",
        )
        .expect("old metadata");
        let mut partial = cache.create_payload().expect("temporary installer");
        write_temporary_pe(&mut partial, MIN_INSTALLER_SIZE + 8192, 0x8664).await;
        let downloaded = validate_temporary_download(&mut partial, None)
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

        let partial_path = partial.path().to_path_buf();
        commit(&cache, &mut partial, &metadata)
            .await
            .expect("commit cache");

        assert!(!partial_path.exists());
        assert_eq!(
            inspect(&cache, "11.12.2", "https://example.test/installer").await,
            CacheInspection::Valid(metadata)
        );
    }

    #[tokio::test]
    async fn discard_removes_payload_metadata_and_interrupted_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let cache =
            InstallerCache::open_for_tests(temporary.path(), "11.12.2").expect("installer cache");
        let installer = cache.installer_path().expect("installer path");
        let metadata = cache.metadata_path().expect("metadata path");
        for path in [&installer, &metadata] {
            fs::write(path, b"fixture").expect("write cache artifact");
        }

        discard(&cache).await.expect("discard cache");

        assert!(!installer.exists());
        assert!(!metadata.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commit_rejects_a_symlinked_destination_without_changing_its_target() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let cache =
            InstallerCache::open_for_tests(temporary.path(), "11.12.2").expect("installer cache");
        let sentinel = temporary.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        symlink(&sentinel, cache.installer_path().expect("installer path")).expect("cache symlink");
        let mut partial = cache.create_payload().expect("temporary installer");
        write_temporary_pe(&mut partial, MIN_INSTALLER_SIZE + 4096, 0x8664).await;
        let downloaded = validate_temporary_download(&mut partial, None)
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

        assert!(commit(&cache, &mut partial, &metadata).await.is_err());
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");
    }
}
