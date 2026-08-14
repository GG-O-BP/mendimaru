use flate2::read::GzDecoder;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_ARCHIVE_FILES: u64 = 250_000;
const MAX_EXTRACTED_BYTES: u64 = 16 * 1024 * 1024 * 1024;

pub(super) fn extract_toolchain(archive_path: &Path, destination: &Path) -> Result<(), String> {
    ensure_empty_destination(destination)?;
    let archive = File::open(archive_path)
        .map_err(|error| format!("could not open the MxBuild archive: {error}"))?;
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("could not read the MxBuild archive: {error}"))?;
    let mut file_count = 0_u64;
    let mut extracted_bytes = 0_u64;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| format!("could not read an MxBuild archive entry: {error}"))?;
        file_count = file_count.saturating_add(1);
        if file_count > MAX_ARCHIVE_FILES {
            return Err("the MxBuild archive contains too many entries".to_string());
        }
        let relative = entry
            .path()
            .map_err(|error| format!("could not decode an MxBuild archive path: {error}"))?;
        let relative = safe_relative_path(&relative)?;
        let destination_path = destination.join(&relative);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            create_direct_directory(&destination_path)?;
            continue;
        }
        if !entry_type.is_file() {
            return Err("the MxBuild archive contains a link or special file".to_string());
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .filter(|bytes| *bytes <= MAX_EXTRACTED_BYTES)
            .ok_or_else(|| "the MxBuild archive exceeds the extraction limit".to_string())?;
        if let Some(parent) = destination_path.parent() {
            create_direct_directory(parent)?;
        }
        let executable = entry.header().mode().unwrap_or(0) & 0o111 != 0;
        let size = entry.size();
        let mut output = create_new_file(&destination_path)?;
        copy_exact_bounded(&mut entry, &mut output, size)?;
        output
            .flush()
            .map_err(|error| format!("could not persist an MxBuild archive entry: {error}"))?;
        set_extracted_permissions(&destination_path, executable)?;
    }
    Ok(())
}

pub(super) fn extract_portable_package(
    archive_path: &Path,
    destination: &Path,
) -> Result<(), String> {
    ensure_empty_destination(destination)?;
    let archive = File::open(archive_path)
        .map_err(|error| format!("could not open the portable package: {error}"))?;
    let mut archive = zip::ZipArchive::new(archive)
        .map_err(|error| format!("could not read the portable package: {error}"))?;
    if archive.len() as u64 > MAX_ARCHIVE_FILES {
        return Err("the portable package contains too many entries".to_string());
    }
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("could not read a portable package entry: {error}"))?;
        if entry.encrypted() {
            return Err("encrypted portable package entries are not supported".to_string());
        }
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| "the portable package contains an unsafe path".to_string())?;
        let relative = safe_relative_path(&enclosed)?;
        let destination_path = destination.join(&relative);
        let unix_mode = entry.unix_mode().unwrap_or(0);
        if unix_mode & 0o170000 == 0o120000 {
            return Err("the portable package contains a symbolic link".to_string());
        }
        if entry.is_dir() {
            create_direct_directory(&destination_path)?;
            continue;
        }
        if !entry.is_file() {
            return Err("the portable package contains a special file".to_string());
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .filter(|bytes| *bytes <= MAX_EXTRACTED_BYTES)
            .ok_or_else(|| "the portable package exceeds the extraction limit".to_string())?;
        if let Some(parent) = destination_path.parent() {
            create_direct_directory(parent)?;
        }
        let executable = unix_mode & 0o111 != 0
            || relative == Path::new("bin/start")
            || relative == Path::new("bin/start.bat")
            || relative == Path::new("bin/start.ps1");
        let size = entry.size();
        let mut output = create_new_file(&destination_path)?;
        copy_exact_bounded(&mut entry, &mut output, size)?;
        output
            .flush()
            .map_err(|error| format!("could not persist a portable package entry: {error}"))?;
        set_extracted_permissions(&destination_path, executable)?;
    }
    for required in ["app", "bin", "etc", "lib"] {
        let path = destination.join(required);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| format!("the portable package is missing {required}/"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "the portable package has an invalid {required}/ entry"
            ));
        }
    }
    let launchers = if cfg!(windows) {
        vec![
            destination.join("bin/start.bat"),
            destination.join("bin/start.ps1"),
        ]
    } else {
        vec![destination.join("bin/start")]
    };
    for launcher in launchers {
        let metadata = fs::symlink_metadata(&launcher)
            .map_err(|_| "the portable package is missing a host start script".to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("a portable package start script is invalid".to_string());
        }
    }
    Ok(())
}

fn ensure_empty_destination(destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("the archive extraction destination already exists".to_string());
    }
    create_direct_directory(destination)
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err("the archive contains an unsafe path".to_string());
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => safe.push(value),
            _ => return Err("the archive contains an unsafe path".to_string()),
        }
    }
    if safe.as_os_str().is_empty() {
        return Err("the archive contains an empty path".to_string());
    }
    Ok(safe)
}

fn create_direct_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("could not create an archive directory: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect an archive directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("an archive directory is not direct".to_string());
    }
    set_directory_permissions(path)
}

fn create_new_file(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("could not create an archive file: {error}"))
}

fn copy_exact_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected: u64,
) -> Result<(), String> {
    let copied = io::copy(&mut reader.take(expected.saturating_add(1)), writer)
        .map_err(|error| format!("could not extract an archive file: {error}"))?;
    if copied != expected {
        return Err("an archive entry length did not match its header".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not protect an archive directory: {error}"))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_extracted_permissions(path: &Path, executable: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("could not protect an archive file: {error}"))
}

#[cfg(not(unix))]
fn set_extracted_permissions(_path: &Path, _executable: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_package_extraction_requires_the_documented_shape() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let archive_path = temporary.path().join("portable.zip");
        let file = File::create(&archive_path).expect("archive file");
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for directory in ["app/", "bin/", "etc/", "lib/"] {
            archive
                .add_directory(directory, options)
                .expect("directory");
        }
        archive
            .start_file("bin/start", options)
            .expect("start file");
        archive.write_all(b"#!/bin/sh\n").expect("start contents");
        archive
            .start_file("bin/start.bat", options)
            .expect("batch file");
        archive.write_all(b"@echo off\r\n").expect("batch contents");
        archive.finish().expect("finish archive");

        let destination = temporary.path().join("extracted");
        extract_portable_package(&archive_path, &destination).expect("extract package");
        assert!(destination.join("bin/start").is_file());
    }

    #[test]
    fn rejects_a_portable_package_without_a_host_launcher() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let archive_path = temporary.path().join("invalid.zip");
        let file = File::create(&archive_path).expect("archive file");
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for directory in ["app/", "bin/", "etc/", "lib/"] {
            archive
                .add_directory(directory, options)
                .expect("directory");
        }
        archive.finish().expect("finish archive");
        assert!(extract_portable_package(&archive_path, &temporary.path().join("bad")).is_err());
    }
}
