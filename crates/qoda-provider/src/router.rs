//! Source authority selects an adapter before any catalog, media or mutation call.
use super::*;
use std::sync::Arc;
pub struct Router {
    local: Arc<dyn MusicProvider>,
    remote: Option<Arc<dyn MusicProvider>>,
}
impl Router {
    pub fn new(local: Arc<dyn MusicProvider>, remote: Option<Arc<dyn MusicProvider>>) -> Self {
        Self { local, remote }
    }
    fn route(&self, source: SourceId) -> Result<&dyn MusicProvider, ProviderError> {
        if self.local.libraries().iter().any(|l| l.id == source) {
            return Ok(self.local.as_ref());
        }
        let remote = self.remote.as_ref().ok_or(ProviderError::SignInRequired)?;
        if remote.libraries().iter().any(|l| l.id == source) {
            Ok(remote.as_ref())
        } else {
            Err(ProviderError::UnknownSource)
        }
    }
}
impl MusicProvider for Router {
    fn downloads(&self, source: SourceId) -> bool {
        self.route(source).is_ok_and(|p| p.downloads(source))
    }
    fn download(&self, source: SourceId, key: ProviderTrackKey) -> ProviderFuture<'_, Download> {
        Box::pin(async move { self.route(source)?.download(source, key).await })
    }
    fn cache_catalog(&self, source: SourceId) -> bool {
        self.route(source).is_ok_and(|p| p.cache_catalog(source))
    }
    fn libraries(&self) -> Vec<Library> {
        let mut libraries = self.local.libraries();
        if let Some(remote) = &self.remote {
            let ids: std::collections::HashSet<_> = libraries.iter().map(|l| l.id).collect();
            libraries.extend(
                remote
                    .libraries()
                    .into_iter()
                    .filter(|l| !ids.contains(&l.id)),
            );
        }
        libraries
    }
    fn tracks(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, CatalogPage> {
        Box::pin(async move { self.route(source)?.tracks(source, offset, query).await })
    }
    fn prepare(
        &self,
        source: SourceId,
        key: ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async move { self.route(source)?.prepare(source, key).await })
    }
    fn albums(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async move { self.route(source)?.albums(source, offset, query).await })
    }
    fn artists(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, ArtistPage> {
        Box::pin(async move { self.route(source)?.artists(source, offset, query).await })
    }
    fn album(&self, reference: AlbumRef) -> ProviderFuture<'_, AlbumSnapshot> {
        Box::pin(async move { self.route(reference.source)?.album(reference).await })
    }
    fn artist_albums(
        &self,
        reference: ArtistRef,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async move {
            self.route(reference.source)?
                .artist_albums(reference, offset, query)
                .await
        })
    }
    fn cover(&self, reference: AlbumRef) -> ProviderFuture<'_, Vec<u8>> {
        Box::pin(async move { self.route(reference.source)?.cover(reference).await })
    }
    fn cache_covers(&self, source: SourceId) -> bool {
        self.route(source)
            .is_ok_and(|provider| provider.cache_covers(source))
    }
    fn playlists(&self, source: SourceId, offset: u64) -> ProviderFuture<'_, PlaylistPage> {
        Box::pin(async move { self.route(source)?.playlists(source, offset).await })
    }
    fn playlist(&self, reference: PlaylistRef) -> ProviderFuture<'_, PlaylistSnapshot> {
        Box::pin(async move { self.route(reference.source)?.playlist(reference).await })
    }
    fn edit_playlist(&self, change: PlaylistChange) -> ProviderFuture<'_, PlaylistOutcome> {
        Box::pin(async move {
            let source = match &change {
                PlaylistChange::Create { source, .. } => *source,
                PlaylistChange::Update { reference, .. } => reference.source,
            };
            self.route(source)?.edit_playlist(change).await
        })
    }
    fn prepare_transcoded(&self, request: TranscodeRequest) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async move {
            self.route(request.reference.source)?
                .prepare_transcoded(request)
                .await
        })
    }
    fn report_playback(&self, report: playback::Report) -> ProviderFuture<'_, playback::Outcome> {
        Box::pin(async move {
            self.route(report.track.source)?
                .report_playback(report)
                .await
        })
    }
}
