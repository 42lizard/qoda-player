//! Descriptor-anchored, read-only access; neither traversal nor symlinks grant access.
use super::*;
use std::{
    fs::{File, Metadata, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Component,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub(super) fn version(m: &Metadata) -> String {
    digest(
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec()
        )
        .as_bytes(),
    )
}
fn descend(dir: File, relative: &Path, want_directory: bool) -> Result<File, ProviderError> {
    let components: Vec<_> = relative.components().collect();
    let mut current = dir;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(ProviderError::LocalUnavailable);
        };
        let is_directory = want_directory || index + 1 < components.len();
        let path = PathBuf::from(format!("/proc/self/fd/{}", current.as_raw_fd())).join(name);
        current = OpenOptions::new()
            .read(true)
            .custom_flags(
                libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | if is_directory { libc::O_DIRECTORY } else { 0 },
            )
            .open(path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ProviderError::MissingTrack
                } else {
                    ProviderError::LocalUnavailable
                }
            })?;
    }
    if want_directory
        || current
            .metadata()
            .map_err(|_| ProviderError::LocalUnavailable)?
            .is_file()
    {
        Ok(current)
    } else {
        Err(ProviderError::LocalUnavailable)
    }
}
pub(super) fn directory(root: &Path, relative: &Path) -> Result<File, ProviderError> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root)
        .map_err(|_| ProviderError::LocalUnavailable)?;
    descend(directory, relative, true)
}
pub(super) fn open(root: &Path, relative: &Path) -> Result<File, ProviderError> {
    if relative.as_os_str().is_empty() {
        return Err(ProviderError::LocalUnavailable);
    }
    descend(directory(root, Path::new(""))?, relative, false)
}
pub(super) struct Source {
    root: PathBuf,
    relative: PathBuf,
    version: String,
}
impl Source {
    pub fn new(root: PathBuf, relative: PathBuf, version: String) -> Result<Self, ProviderError> {
        let this = Self {
            root,
            relative,
            version,
        };
        this.checked()?;
        Ok(this)
    }
    fn checked(&self) -> Result<File, ProviderError> {
        let file = open(&self.root, &self.relative)?;
        if version(
            &file
                .metadata()
                .map_err(|_| ProviderError::LocalUnavailable)?,
        ) != self.version
        {
            return Err(ProviderError::LocalChanged);
        }
        Ok(file)
    }
}
impl MediaSource for Source {
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            let file = self.checked()?;
            let length = file
                .metadata()
                .map_err(|_| ProviderError::LocalUnavailable)?
                .len();
            let mut start = 0;
            let mut end = length.saturating_sub(1);
            let mut status = 200;
            if let Some(range) = range {
                let (a, b) = range
                    .header()
                    .strip_prefix("bytes=")
                    .unwrap()
                    .split_once('-')
                    .unwrap();
                if a.is_empty() {
                    start =
                        length.saturating_sub(b.parse().map_err(|_| ProviderError::InvalidRange)?);
                } else {
                    start = a.parse().map_err(|_| ProviderError::InvalidRange)?;
                    if !b.is_empty() {
                        end = b
                            .parse::<u64>()
                            .map_err(|_| ProviderError::InvalidRange)?
                            .min(end);
                    }
                }
                status = 206;
                if start >= length {
                    return Ok(MediaResponse {
                        status: 416,
                        content_type: None,
                        content_length: Some(0),
                        content_range: Some(format!("bytes */{length}")),
                        etag: None,
                        body: futures_util::stream::empty().boxed(),
                    });
                }
            }
            let count = if length == 0 { 0 } else { end - start + 1 };
            let mut file = tokio::fs::File::from_std(file);
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|_| ProviderError::LocalUnavailable)?;
            let expected = self.version.clone();
            let body = futures_util::stream::try_unfold(
                (file, if head { 0 } else { count }, expected),
                |(mut file, remaining, expected)| async move {
                    if remaining == 0 {
                        return Ok(None);
                    }
                    if version(
                        &file
                            .metadata()
                            .await
                            .map_err(|_| ProviderError::LocalUnavailable)?,
                    ) != expected
                    {
                        return Err(ProviderError::LocalChanged);
                    }
                    let mut bytes = vec![0; usize::try_from(remaining.min(65536)).unwrap()];
                    let n = file
                        .read(&mut bytes)
                        .await
                        .map_err(|_| ProviderError::LocalUnavailable)?;
                    if n == 0
                        || version(
                            &file
                                .metadata()
                                .await
                                .map_err(|_| ProviderError::LocalUnavailable)?,
                        ) != expected
                    {
                        return Err(ProviderError::LocalChanged);
                    }
                    bytes.truncate(n);
                    Ok(Some((
                        Bytes::from(bytes),
                        (file, remaining - n as u64, expected),
                    )))
                },
            )
            .boxed();
            Ok(MediaResponse {
                status,
                content_type: Some("application/octet-stream".into()),
                content_length: Some(count),
                content_range: (status == 206).then(|| format!("bytes {start}-{end}/{length}")),
                etag: Some(format!("\"{}\"", self.version)),
                body,
            })
        })
    }
}
use futures_util::StreamExt;
