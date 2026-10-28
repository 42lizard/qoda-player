//! Read-only folder libraries. Index snapshots never grant arbitrary path access.
use super::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
mod covers;
mod media;
mod probe;

/// A per-file sidecar avoids collisions between albums in flat import folders.
pub fn cover_sidecar(audio: &Path) -> PathBuf {
    let mut name = audio.as_os_str().to_os_string();
    name.push(".qoda-cover.jpg");
    PathBuf::from(name)
}

pub const DOWNLOAD_DIRECTORY: &str = "Qoda Downloads";
pub const DOWNLOAD_STAGING: &str = ".qoda-download-staging";
const PAGE: usize = 100;
const MAX_FILES: usize = 100_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Root {
    pub id: SourceId,
    pub path: PathBuf,
}
impl Root {
    pub fn new(path: PathBuf) -> Result<Self, ProviderError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ProviderError::LocalUnavailable);
        }
        // Normalize missing paths too, resolving their nearest existing ancestor.
        // A drive/folder appearing later must not change the persisted source ID.
        let path: PathBuf = path.components().collect();
        let mut ancestor = path.clone();
        let mut missing = Vec::new();
        let path = loop {
            if let Ok(mut resolved) = std::fs::canonicalize(&ancestor) {
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                break resolved;
            }
            if std::fs::symlink_metadata(&ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(ProviderError::LocalUnavailable);
            }
            let Some(name) = ancestor.file_name().map(std::ffi::OsStr::to_owned) else {
                break path;
            };
            missing.push(name);
            ancestor.pop();
        };
        if std::fs::metadata(&path).is_ok_and(|m| !m.is_dir()) {
            return Err(ProviderError::LocalUnavailable);
        }
        let id = Self::identity(&path)?;
        Ok(Self { id, path })
    }
    fn identity(path: &Path) -> Result<SourceId, ProviderError> {
        let hash = Sha256::digest(path.as_os_str().as_encoded_bytes());
        SourceId::try_from(uuid::Uuid::from_bytes(hash[..16].try_into().unwrap()))
            .map_err(|_| ProviderError::InvalidReply)
    }
    /// Validate persisted identity without mistaking an unavailable drive for
    /// damaged settings. File access still checks the live directory separately.
    pub fn has_valid_identity(&self) -> bool {
        self.path.is_absolute()
            && !self
                .path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            && self.path.components().collect::<PathBuf>().as_os_str() == self.path.as_os_str()
            && Self::identity(&self.path).is_ok_and(|id| id == self.id)
    }
}
#[derive(Clone)]
struct Entry {
    root: Root,
    relative: PathBuf,
    version: String,
    track: Track,
    disc: u32,
    number: u32,
    year: Option<u32>,
    album_artist: String,
}
impl Entry {
    fn album_key(&self) -> String {
        digest(format!("{}\0{}", self.album_artist, self.track.album).as_bytes())
    }
    fn artist_key(&self) -> String {
        digest(self.album_artist.as_bytes())
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FolderStatus {
    pub id: SourceId,
    pub path: PathBuf,
    pub tracks: Counter,
    pub message: &'static str,
}
#[derive(Default)]
struct Index {
    entries: HashMap<(SourceId, String), Arc<Entry>>,
    seen: HashMap<(SourceId, PathBuf), String>,
    folders: Vec<FolderStatus>,
}
struct ScanCancellation(Arc<AtomicBool>);
impl Drop for ScanCancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
pub struct Local {
    roots: Vec<Root>,
    index: RwLock<Index>,
    scan_lock: tokio::sync::Mutex<()>,
}
impl Local {
    pub fn new(roots: Vec<Root>) -> Self {
        Self {
            roots,
            index: RwLock::new(Index::default()),
            scan_lock: Default::default(),
        }
    }
    pub fn refresh_metadata(&self) {
        self.index
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .seen
            .clear();
    }
    pub fn roots(&self) -> &[Root] {
        &self.roots
    }
    pub fn status(&self) -> Vec<FolderStatus> {
        self.index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .folders
            .clone()
    }
    fn entries(&self, source: SourceId) -> Result<Vec<Arc<Entry>>, ProviderError> {
        if !self.roots.iter().any(|r| r.id == source) {
            return Err(ProviderError::UnknownSource);
        }
        Ok(self
            .index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .values()
            .filter(|e| e.root.id == source)
            .cloned()
            .collect())
    }
    /// Bounded polling scan; unchanged audio and rejected non-audio reuse their fingerprint.
    pub async fn scan(&self) -> Result<bool, ProviderError> {
        let _permit = self.scan_lock.lock().await;
        if !self.roots.is_empty() {
            probe::available().await?;
        }
        let roots = self.roots.clone();
        let cancellation = ScanCancellation(Arc::new(AtomicBool::new(false)));
        let cancelled = cancellation.0.clone();
        let (files, mut folders) = tokio::task::spawn_blocking(move || collect(&roots, &cancelled))
            .await
            .map_err(|_| ProviderError::LocalUnavailable)?;
        let (previous, seen) = {
            let index = self.index.read().unwrap_or_else(|e| e.into_inner());
            (index.entries.clone(), index.seen.clone())
        };
        let mut next = Index::default();
        let work =
            futures_util::stream::iter(files.into_iter().map(|(root, relative, version)| {
                let old = previous
                    .get(&(root.id, digest(relative.as_os_str().as_encoded_bytes())))
                    .cloned();
                let unchanged = seen.get(&(root.id, relative.clone())) == Some(&version);
                async move {
                    let entry = if unchanged {
                        old
                    } else {
                        probe::read(&root, &relative, &version)
                            .await
                            .map(|metadata| {
                                let title = metadata.title.unwrap_or_else(|| {
                                    relative
                                        .file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                });
                                let album = metadata.album.unwrap_or_else(|| {
                                    relative
                                        .parent()
                                        .filter(|p| !p.as_os_str().is_empty())
                                        .and_then(Path::file_name)
                                        .or_else(|| root.path.file_name())
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                });
                                let artist = metadata.artist.unwrap_or_default();
                                let album_artist =
                                    metadata.album_artist.unwrap_or_else(|| artist.clone());
                                Arc::new(Entry {
                                    track: Track {
                                        reference: TrackRef {
                                            source: root.id,
                                            key: digest(relative.as_os_str().as_encoded_bytes())
                                                .try_into()
                                                .unwrap(),
                                        },
                                        title,
                                        artist,
                                        album,
                                        duration_ms: metadata.duration.map(Counter::new),
                                    },
                                    root: root.clone(),
                                    relative: relative.clone(),
                                    version: version.clone(),
                                    disc: metadata.disc,
                                    number: metadata.number,
                                    year: metadata.year,
                                    album_artist,
                                })
                            })
                    };
                    (root.id, relative, version, entry)
                }
            }));
        use futures_util::StreamExt;
        let mut work = work.buffer_unordered(4);
        while let Some((source, relative, version, entry)) = work.next().await {
            next.seen.insert((source, relative), version);
            if let Some(entry) = entry {
                next.entries.insert(
                    (source, String::from(entry.track.reference.key.clone())),
                    entry,
                );
            }
        }
        for status in &mut folders {
            status.tracks = Counter::new(
                next.entries
                    .values()
                    .filter(|e| e.root.id == status.id)
                    .count() as u64,
            );
        }
        next.folders = folders;
        let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
        let changed = next.folders != index.folders || next.seen != index.seen;
        *index = next;
        Ok(changed)
    }
}
fn collect(
    roots: &[Root],
    cancelled: &AtomicBool,
) -> (Vec<(Root, PathBuf, String)>, Vec<FolderStatus>) {
    let mut visited = 0_usize;
    let mut files = Vec::new();
    let mut folders = Vec::new();
    for root in roots {
        let mut stack = vec![(PathBuf::new(), 0)];
        let mut message = "Local folder ready.";
        while let Some((relative, depth)) = stack.pop() {
            if cancelled.load(Ordering::Acquire) {
                return (files, folders);
            }
            if depth > 64 || files.len() >= MAX_FILES || visited >= 300_000 {
                message = "Local scan limit reached; narrow the configured folders.";
                break;
            }
            let dir = match media::directory(&root.path, &relative) {
                Ok(dir) => dir,
                Err(_) => {
                    message = "Local folder or subfolder is unavailable; check permissions or reconnect the drive.";
                    continue;
                }
            };
            let path = PathBuf::from(format!(
                "/proc/self/fd/{}",
                std::os::fd::AsRawFd::as_raw_fd(&dir)
            ));
            let Ok(read) = std::fs::read_dir(path) else {
                message = "Local folder or subfolder is unavailable; check permissions or reconnect the drive.";
                continue;
            };
            for child in read {
                if cancelled.load(Ordering::Acquire) {
                    return (files, folders);
                }
                visited += 1;
                if visited > 300_000 {
                    message = "Local scan limit reached; narrow the configured folders.";
                    break;
                }
                let Ok(child) = child else {
                    continue;
                };
                let Ok(kind) = child.file_type() else {
                    continue;
                };
                if relative == Path::new(DOWNLOAD_DIRECTORY)
                    && child.file_name() == DOWNLOAD_STAGING
                {
                    continue;
                }
                let relative = relative.join(child.file_name());
                if kind.is_dir() {
                    stack.push((relative, depth + 1));
                } else if kind.is_file() {
                    if files.len() >= MAX_FILES {
                        message = "Local scan limit reached; narrow the configured folders.";
                        break;
                    }
                    if let Ok(file) = media::open(&root.path, &relative)
                        && let Ok(meta) = file.metadata()
                    {
                        files.push((root.clone(), relative, media::version(&meta)));
                    }
                }
            }
        }
        folders.push(FolderStatus {
            id: root.id,
            path: root.path.clone(),
            tracks: Counter::ZERO,
            message,
        });
    }
    (files, folders)
}
fn page<T>(items: Vec<T>, offset: u64) -> (Vec<T>, bool) {
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    let more = items.len().saturating_sub(start) > PAGE;
    (items.into_iter().skip(start).take(PAGE).collect(), more)
}
fn matches(value: &str, query: &str) -> bool {
    value.to_lowercase().contains(&query.to_lowercase())
}
fn album_groups(entries: Vec<Arc<Entry>>, source: SourceId, query: &str) -> Vec<Album> {
    let mut groups = HashMap::<String, (Arc<Entry>, u64)>::new();
    for e in entries {
        let key = e.album_key();
        groups
            .entry(key)
            .and_modify(|(first, n)| {
                *n += 1;
                if e.relative < first.relative {
                    *first = e.clone();
                }
            })
            .or_insert((e, 1));
    }
    let mut albums: Vec<_> = groups
        .into_iter()
        .filter(|(_, (e, _))| matches(&e.track.album, query))
        .map(|(key, (e, n))| Album {
            reference: AlbumRef { source, key },
            artist_ref: (!e.album_artist.is_empty()).then(|| ArtistRef {
                source,
                key: e.artist_key(),
            }),
            title: e.track.album.clone(),
            artist: e.album_artist.clone(),
            year: e.year,
            track_count: Some(Counter::new(n)),
        })
        .collect();
    albums.sort_by(|a, b| {
        a.title
            .cmp(&b.title)
            .then_with(|| a.reference.key.cmp(&b.reference.key))
    });
    albums
}
impl MusicProvider for Local {
    fn cache_covers(&self, _: SourceId) -> bool {
        false
    }
    fn cover(&self, reference: AlbumRef) -> ProviderFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let mut entries: Vec<_> = self
                .entries(reference.source)?
                .into_iter()
                .filter(|entry| entry.album_key() == reference.key)
                .collect();
            entries.sort_by(|a, b| a.relative.cmp(&b.relative));
            tokio::time::timeout(std::time::Duration::from_secs(10), covers::read(entries))
                .await
                .map_err(|_| ProviderError::LocalUnavailable)?
        })
    }
    fn cache_catalog(&self, _: SourceId) -> bool {
        false
    }
    fn libraries(&self) -> Vec<Library> {
        self.roots
            .iter()
            .map(|r| Library {
                id: r.id,
                title: r
                    .path
                    .file_name()
                    .unwrap_or(r.path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                server: "Local files".into(),
            })
            .collect()
    }
    fn tracks(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, CatalogPage> {
        Box::pin(async move {
            let mut entries = self.entries(source)?;
            entries.retain(|e| {
                matches(
                    &format!("{} {} {}", e.track.title, e.track.artist, e.track.album),
                    &query,
                )
            });
            entries.sort_by(|a, b| {
                a.track
                    .title
                    .cmp(&b.track.title)
                    .then_with(|| a.relative.cmp(&b.relative))
            });
            let total = entries.len();
            let (entries, has_more) = page(entries, offset);
            let tracks = entries.into_iter().map(|e| e.track.clone()).collect();
            Ok(CatalogPage {
                source,
                offset: Counter::new(offset),
                total: Some(Counter::new(total as u64)),
                query,
                tracks,
                has_more,
            })
        })
    }
    fn prepare(
        &self,
        source: SourceId,
        key: ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async move {
            let root = self
                .roots
                .iter()
                .find(|r| r.id == source)
                .ok_or(ProviderError::UnknownSource)?;
            media::directory(&root.path, Path::new(""))?;
            let entry = self
                .index
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .entries
                .get(&(source, String::from(key)))
                .cloned()
                .ok_or(ProviderError::MissingTrack)?;
            let stream = media::Source::new(
                entry.root.path.clone(),
                entry.relative.clone(),
                entry.version.clone(),
            )?;
            Ok(PreparedMedia {
                track: entry.track.clone(),
                version: entry.version.clone(),
                stream: Box::new(stream),
            })
        })
    }
    fn albums(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async move {
            let albums = album_groups(self.entries(source)?, source, &query);
            let (albums, has_more) = page(albums, offset);
            Ok(AlbumPage {
                source,
                artist: None,
                offset: Counter::new(offset),
                query,
                albums,
                has_more,
            })
        })
    }
    fn artists(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, ArtistPage> {
        Box::pin(async move {
            let mut groups = HashMap::new();
            for e in self.entries(source)? {
                if !e.album_artist.is_empty() && matches(&e.album_artist, &query) {
                    groups.insert(e.artist_key(), e.album_artist.clone());
                }
            }
            let mut artists: Vec<_> = groups
                .into_iter()
                .map(|(key, title)| Artist {
                    reference: ArtistRef { source, key },
                    title,
                })
                .collect();
            artists.sort_by(|a, b| a.title.cmp(&b.title));
            let (artists, has_more) = page(artists, offset);
            Ok(ArtistPage {
                source,
                offset: Counter::new(offset),
                query,
                artists,
                has_more,
            })
        })
    }
    fn artist_albums(
        &self,
        reference: ArtistRef,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async move {
            let entries = self.entries(reference.source)?;
            let artist = entries
                .iter()
                .find(|entry| entry.artist_key() == reference.key)
                .ok_or(ProviderError::MissingTrack)?
                .album_artist
                .clone();
            let albums = album_groups(
                entries
                    .into_iter()
                    .filter(|entry| entry.artist_key() == reference.key)
                    .collect(),
                reference.source,
                &query,
            );
            let (albums, has_more) = page(albums, offset);
            Ok(AlbumPage {
                source: reference.source,
                artist: Some(Artist {
                    reference,
                    title: artist,
                }),
                offset: Counter::new(offset),
                query,
                albums,
                has_more,
            })
        })
    }
    fn album(&self, reference: AlbumRef) -> ProviderFuture<'_, AlbumSnapshot> {
        Box::pin(async move {
            let mut entries: Vec<_> = self
                .entries(reference.source)?
                .into_iter()
                .filter(|e| e.album_key() == reference.key)
                .collect();
            entries.sort_by(|a, b| {
                (a.disc, a.number, &a.relative).cmp(&(b.disc, b.number, &b.relative))
            });
            if entries.len() > 500 {
                return Err(ProviderError::LocalLimit);
            }
            let first = entries.first().ok_or(ProviderError::MissingTrack)?;
            let album = Album {
                reference: reference.clone(),
                artist_ref: (!first.album_artist.is_empty()).then(|| ArtistRef {
                    source: reference.source,
                    key: first.artist_key(),
                }),
                title: first.track.album.clone(),
                artist: first.album_artist.clone(),
                year: first.year,
                track_count: Some(Counter::new(entries.len() as u64)),
            };
            let revision = digest(
                entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{}:{};",
                            String::from(e.track.reference.key.clone()),
                            e.version
                        )
                    })
                    .collect::<String>()
                    .as_bytes(),
            );
            Ok(AlbumSnapshot {
                album,
                revision,
                tracks: entries
                    .into_iter()
                    .map(|e| AlbumTrack {
                        track: e.track.clone(),
                        disc: e.disc,
                        number: e.number,
                    })
                    .collect(),
                artwork: None,
                artwork_unavailable: true,
            })
        })
    }
}
#[cfg(test)]
mod tests;
