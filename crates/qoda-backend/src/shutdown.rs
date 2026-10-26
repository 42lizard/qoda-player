//! Ordered shutdown outcomes and identity-checked runtime socket cleanup.
use crate::storage;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

pub const CHECKPOINT_FAILED: u8 = 74;
pub const BACKUP_WARNING: u8 = 75;
pub const CLEANUP_FAILED: u8 = 76;

#[derive(Debug)]
pub enum Error {
    Checkpoint(storage::Error),
    Archive(&'static str),
    Backup(storage::Error),
    Cleanup(std::io::Error),
}
impl Error {
    pub fn exit_status(&self) -> u8 {
        match self {
            Self::Checkpoint(_) | Self::Archive(_) => CHECKPOINT_FAILED,
            Self::Backup(_) => BACKUP_WARNING,
            Self::Cleanup(_) => CLEANUP_FAILED,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Archive(error) => write!(f, "final archive checkpoint failed: {error}"),
            Self::Checkpoint(error) => write!(f, "final checkpoint failed: {error}"),
            Self::Backup(error) => write!(f, "session saved; final backup update failed: {error}"),
            Self::Cleanup(error) => write!(f, "runtime socket cleanup failed: {error}"),
        }
    }
}
impl std::error::Error for Error {}

pub fn checkpoint(result: Result<Option<storage::Error>, storage::Error>) -> Result<(), Error> {
    match result {
        Ok(None) => Ok(()),
        Ok(Some(warning)) => Err(Error::Backup(warning)),
        Err(error) => Err(Error::Checkpoint(error)),
    }
}

/// Remove only the captured socket inode; preserve path replacements.
pub struct SocketIdentity {
    device: u64,
    inode: u64,
}
impl SocketIdentity {
    pub fn capture(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other("Runtime socket path is not a socket"));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    pub async fn clear_stale(path: &Path) -> std::io::Result<()> {
        let identity = Self::capture(path)?;
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::net::UnixStream::connect(path),
        )
        .await
        {
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) =>
            {
                identity.remove(path)
            }
            _ => Err(std::io::Error::other(
                "Active or unresponsive runtime socket preserved",
            )),
        }
    }
    pub fn remove(self, path: &Path) -> std::io::Result<()> {
        let metadata = match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        };
        if !metadata.file_type().is_socket()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(std::io::Error::other(
                "Runtime socket path was replaced; conflicting artifact preserved",
            ));
        }
        std::fs::remove_file(path)
    }
}
