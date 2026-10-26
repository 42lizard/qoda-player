//! Private Linux cache files, anchored to open directory identities.
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, &'static str>;
// Anchor all operations to an opened private directory, including after path replacement.
// This app targets Linux; /proc/self/fd avoids following a replacement root symlink.
pub(crate) struct Directory(pub File);
impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_private(path, false)
    }
    // Earlier source-identity persistence created the Qoda config directory
    // with the process umask (usually 0755). Tighten only owned directories
    // that nobody else can write, using the opened descriptor, not the path.
    pub fn open_settings(path: &Path) -> Result<Self> {
        Self::open_private(path, true)
    }
    fn open_private(path: &Path, migrate_readable_settings: bool) -> Result<Self> {
        match std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
        {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Cache directories could not be opened."),
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| "Cache directory is unsafe or unavailable.")?;
        let m = file
            .metadata()
            .map_err(|_| "Cache directory could not be inspected.")?;
        if migrate_readable_settings
            && m.is_dir()
            && m.uid() == unsafe { libc::getuid() }
            && m.permissions().mode() & 0o022 == 0
            && m.permissions().mode() & 0o077 != 0
        {
            file.set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "Settings directory could not be made private.")?;
        }
        let m = file
            .metadata()
            .map_err(|_| "Directory could not be inspected.")?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::getuid() }
            || m.permissions().mode() & 0o077 != 0
        {
            return Err("Cache directories must be private and owned by this user.");
        }
        Ok(Self(file))
    }
    pub fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.0.as_raw_fd()))
    }
    pub fn resolved_path(&self) -> Result<PathBuf> {
        let path =
            std::fs::read_link(self.path()).map_err(|_| "Cache directory identity unavailable.")?;
        let current = std::fs::symlink_metadata(&path)
            .map_err(|_| "Cache directory moved or is unavailable.")?;
        let opened = self
            .0
            .metadata()
            .map_err(|_| "Cache directory identity unavailable.")?;
        if !current.is_dir() || current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err("Cache directory identity changed.");
        }
        Ok(path)
    }
    pub fn file(&self, name: &str) -> Result<Option<File>> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path().join(name))
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("An cache file is unsafe or unavailable."),
        };
        let m = file
            .metadata()
            .map_err(|_| "An cache file could not be inspected.")?;
        if !private_file(&m) {
            return Err("Cache files must be private regular files owned by this user.");
        }
        Ok(Some(file))
    }
    pub fn atomic(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let temp = format!(".tmp-{}", uuid::Uuid::new_v4());
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(self.path().join(&temp))
                .map_err(|_| "Cache storage is not writable.")?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Cache data could not be saved.")?;
            std::fs::rename(self.path().join(&temp), self.path().join(name))
                .map_err(|_| "Cache data could not be replaced.")?;
            self.0
                .sync_all()
                .map_err(|_| "Cache changes could not be synced.")
        })();
        let _ = std::fs::remove_file(self.path().join(temp));
        result
    }
}
use std::os::unix::fs::DirBuilderExt;
pub(crate) fn private_file(m: &Metadata) -> bool {
    m.is_file()
        && m.uid() == unsafe { libc::getuid() }
        && m.permissions().mode() & 0o077 == 0
        && m.nlink() == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_migration_does_not_relax_cache_or_unsafe_directory_checks() {
        let root = std::env::temp_dir().join(format!("qoda-directory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for mode in [0o755, 0o750] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(Directory::open(&root).is_err());
            assert!(Directory::open_settings(&root).is_ok());
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for mode in [0o775, 0o777] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(Directory::open_settings(&root).is_err());
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.with_extension("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(Directory::open_settings(&link).is_err());
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
