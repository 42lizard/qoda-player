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
