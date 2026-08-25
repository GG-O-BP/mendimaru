use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

pub(crate) struct SecureDirectory {
    path: PathBuf,
    directory: Dir,
}

impl SecureDirectory {
    pub(crate) fn open_or_create(path: &Path) -> Result<Self, String> {
        open_absolute_directory(path, true)
    }

    pub(crate) fn open_existing(path: &Path) -> Result<Self, String> {
        open_absolute_directory(path, false)
    }

    pub(crate) fn try_clone(&self) -> Result<Self, String> {
        Ok(Self {
            path: self.path.clone(),
            directory: self
                .directory
                .try_clone()
                .map_err(|error| format!("could not retain secure directory: {error}"))?,
        })
    }

    pub(crate) fn path_for(&self, name: &str) -> Result<PathBuf, String> {
        validate_direct_name(name)?;
        Ok(self.path.join(name))
    }

    pub(crate) fn symlink_metadata(
        &self,
        name: &str,
    ) -> Result<Option<cap_std::fs::Metadata>, String> {
        validate_direct_name(name)?;
        match self.directory.symlink_metadata(name) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not inspect secure file: {error}")),
        }
    }

    pub(crate) fn open_regular_file(&self, name: &str) -> Result<tokio::fs::File, String> {
        validate_direct_name(name)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = self
            .directory
            .open_with(name, &options)
            .map_err(|error| format!("could not open secure file: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("could not inspect secure file: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("the secure file must be a direct regular file".to_string());
        }
        Ok(tokio::fs::File::from_std(file.into_std()))
    }

    pub(crate) fn create_random_file(
        &self,
        prefix: &str,
        suffix: &str,
    ) -> Result<SecureTemporaryFile, String> {
        validate_name_fragment(prefix)?;
        validate_name_fragment(suffix)?;
        for _ in 0..16 {
            let name = random_name(prefix, suffix)?;
            match self.open_new_file(&name) {
                Ok(file) => return self.retain_temporary_file(name, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("could not create secure temporary file: {error}"))
                }
            }
        }
        Err("could not allocate a secure temporary file".to_string())
    }

    #[cfg(all(test, unix))]
    fn create_named_file_for_tests(&self, name: &str) -> Result<SecureTemporaryFile, String> {
        validate_direct_name(name)?;
        let file = self
            .open_new_file(name)
            .map_err(|error| format!("could not create secure temporary file: {error}"))?;
        self.retain_temporary_file(name.to_string(), file)
    }

    fn open_new_file(&self, name: &str) -> io::Result<cap_std::fs::File> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        self.directory.open_with(name, &options)
    }

    fn retain_temporary_file(
        &self,
        name: String,
        file: cap_std::fs::File,
    ) -> Result<SecureTemporaryFile, String> {
        Ok(SecureTemporaryFile {
            directory: self.try_clone()?,
            path: self.path.join(&name),
            name,
            file: Some(tokio::fs::File::from_std(file.into_std())),
            armed: true,
        })
    }

    pub(crate) fn remove_file(&self, name: &str) -> Result<(), String> {
        validate_direct_name(name)?;
        match self.directory.remove_file_or_symlink(name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove secure file: {error}")),
        }
    }

    pub(crate) fn rename(&self, source: &str, destination: &str) -> Result<(), String> {
        validate_direct_name(source)?;
        validate_direct_name(destination)?;
        self.directory
            .rename(source, &self.directory, destination)
            .map_err(|error| format!("could not rename secure file: {error}"))
    }

    pub(crate) fn random_unused_name(&self, prefix: &str, suffix: &str) -> Result<String, String> {
        validate_name_fragment(prefix)?;
        validate_name_fragment(suffix)?;
        for _ in 0..16 {
            let name = random_name(prefix, suffix)?;
            if self.symlink_metadata(&name)?.is_none() {
                return Ok(name);
            }
        }
        Err("could not allocate a secure backup name".to_string())
    }

    pub(crate) fn sync(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            self.directory
                .open(".")
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("could not sync secure directory: {error}"))?;
        }
        Ok(())
    }
}

pub(crate) struct SecureTemporaryFile {
    directory: SecureDirectory,
    path: PathBuf,
    name: String,
    file: Option<tokio::fs::File>,
    armed: bool,
}

impl SecureTemporaryFile {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn file_mut(&mut self) -> &mut tokio::fs::File {
        self.file.as_mut().expect("temporary file remains open")
    }

    pub(crate) fn sync_parent(&self) -> Result<(), String> {
        self.directory.sync()
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SecureTemporaryFile {
    fn drop(&mut self) {
        // Close the handle first so cleanup is reliable on Windows as well as
        // Unix. Until this point the descriptor remains the identity used by
        // every write, validation, and commit operation.
        drop(self.file.take());
        if self.armed {
            let _ = self.directory.remove_file(&self.name);
        }
    }
}

pub(crate) fn open_secure_file(path: &Path) -> Result<tokio::fs::File, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the secure file has no parent directory".to_string())?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "the secure file name is not valid UTF-8".to_string())?;
    SecureDirectory::open_existing(parent)?.open_regular_file(name)
}

fn open_absolute_directory(path: &Path, create: bool) -> Result<SecureDirectory, String> {
    if !path.is_absolute() {
        return Err("a secure directory path must be absolute".to_string());
    }
    let (root, components) = split_absolute_path(path)?;
    let mut directory = Dir::open_ambient_dir(&root, ambient_authority())
        .map_err(|error| format!("could not open filesystem root securely: {error}"))?;
    let mut opened_path = root;
    for component in components {
        let next = match directory.open_dir_nofollow(&component) {
            Ok(next) => next,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                #[cfg(unix)]
                let mut builder = DirBuilder::new();
                #[cfg(not(unix))]
                let builder = DirBuilder::new();
                #[cfg(unix)]
                {
                    use cap_std::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match directory.create_dir_with(&component, &builder) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(format!("could not create secure directory: {error}"))
                    }
                }
                directory.open_dir_nofollow(&component).map_err(|error| {
                    format!("could not open newly created directory securely: {error}")
                })?
            }
            Err(error) => {
                return Err(format!(
                    "a secure directory component is missing, linked, or invalid: {error}"
                ))
            }
        };
        opened_path.push(&component);
        directory = next;
    }
    Ok(SecureDirectory {
        path: opened_path,
        directory,
    })
}

fn split_absolute_path(path: &Path) -> Result<(PathBuf, Vec<OsString>), String> {
    let mut root = PathBuf::new();
    let mut normal = Vec::new();
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) if root.as_os_str().is_empty() => {
                root.push(prefix.as_os_str());
            }
            Component::RootDir if !rooted => {
                root.push(std::path::MAIN_SEPARATOR.to_string());
                rooted = true;
            }
            Component::Normal(value) if rooted => normal.push(value.to_os_string()),
            Component::CurDir if rooted => {}
            Component::ParentDir => {
                return Err("a secure directory path must not contain parent traversal".to_string())
            }
            _ => return Err("a secure directory path is malformed".to_string()),
        }
    }
    if !rooted {
        return Err("a secure directory path has no filesystem root".to_string());
    }
    Ok((root, normal))
}

fn random_name(prefix: &str, suffix: &str) -> Result<String, String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not generate secure file name: {error}"))?;
    let encoded = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}{encoded}{suffix}"))
}

fn validate_direct_name(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("a secure file name must be one direct component".to_string());
    }
    Ok(())
}

fn validate_name_fragment(value: &str) -> Result<(), String> {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("a secure file name fragment contains an unsafe character".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::SecureDirectory;
    use std::fs;

    #[test]
    fn creates_private_directories_and_unique_direct_files() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let path = temporary.path().join("private/cache/installers");
        let directory = SecureDirectory::open_or_create(&path).expect("secure directory");
        let first = directory
            .create_random_file("payload-", ".download")
            .expect("first temporary file");
        let second = directory
            .create_random_file("payload-", ".download")
            .expect("second temporary file");
        assert_ne!(first.name(), second.name());
        assert!(first.path().is_file());
        assert!(second.path().is_file());
        drop(first);
        drop(second);
        assert_eq!(fs::read_dir(path).expect("read cache").count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_ancestors_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary root");
        let outside = temporary.path().join("outside");
        let sentinel = outside.join("sentinel");
        fs::create_dir(&outside).expect("outside directory");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        let linked = temporary.path().join("linked");
        symlink(&outside, &linked).expect("ancestor symlink");

        assert!(SecureDirectory::open_or_create(&linked.join("installers")).is_err());
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");
        assert!(!outside.join("installers").exists());
    }

    #[cfg(unix)]
    #[test]
    fn retained_directory_handle_cannot_be_retargeted_by_path_replacement() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary root");
        let original = temporary.path().join("cache");
        let detached = temporary.path().join("detached");
        let outside = temporary.path().join("outside");
        fs::create_dir(&original).expect("original directory");
        fs::create_dir(&outside).expect("outside directory");
        let directory = SecureDirectory::open_existing(&original).expect("secure directory");
        fs::rename(&original, &detached).expect("detach directory name");
        symlink(&outside, &original).expect("replace path with symlink");

        let file = directory
            .create_random_file("race-", ".tmp")
            .expect("descriptor-relative file");
        assert!(detached.join(file.name()).is_file());
        assert!(!outside.join(file.name()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn create_new_partial_never_follows_an_existing_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary root");
        let cache = temporary.path().join("cache");
        let sentinel = temporary.path().join("sentinel");
        fs::create_dir(&cache).expect("cache directory");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        symlink(&sentinel, cache.join("known.download")).expect("partial symlink");
        let directory = SecureDirectory::open_existing(&cache).expect("secure directory");

        assert!(directory
            .create_named_file_for_tests("known.download")
            .is_err());
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_windows_junction_ancestor_without_touching_its_target() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let outside = temporary.path().join("outside");
        let linked = temporary.path().join("linked");
        let sentinel = outside.join("sentinel");
        fs::create_dir(&outside).expect("outside directory");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside)
            .status()
            .expect("create junction fixture");
        assert!(status.success(), "create junction fixture");

        assert!(SecureDirectory::open_or_create(&linked.join("installers")).is_err());
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");
        assert!(!outside.join("installers").exists());
        fs::remove_dir(&linked).expect("remove junction fixture");
    }
}
