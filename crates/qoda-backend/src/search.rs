//! Bounded, source-labelled catalog search. Result actions resolve current metadata.
use crate::metadata_cache::{self, Cache, Info};
use futures_util::StreamExt;
use qoda_core::{SourceId, TrackRef};
use qoda_provider::{CatalogPage, Library, ProviderError, Track};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};
const SOURCES: usize = 32;
const RESULTS: usize = 500;
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key {
    account: u64,
    serial: u64,
}
pub enum Reply {
    Page(Key, SourceId, Result<Box<CatalogPage>, ProviderError>, Info),
    Timeout(Key, SourceId),
    Done(Key),
}
struct Source {
    library: Library,
    phase: &'static str,
    tracks: Vec<Track>,
    cache: Info,
    more: bool,
    error: Option<String>,
}
impl Source {
    fn actionable(&self) -> bool {
        self.phase == "ready" && !self.cache.stale && !self.cache.incomplete
    }
}
#[derive(Serialize)]
struct Row {
    track: Track,
    library: String,
    server: String,
    actionable: bool,
}
#[derive(Default)]
pub struct Controller {
    key: Option<Key>,
    serial: u64,
    revision: u64,
    query: String,
    sources: Vec<Source>,
    omitted: usize,
    busy: bool,
    task: Option<JoinHandle<()>>,
}
impl Controller {
    fn bump(&mut self) -> Result<(), &'static str> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("Search revision exhausted")?;
        Ok(())
    }
    pub fn invalidate(&mut self) -> Result<(), &'static str> {
        let serial = self
            .serial
            .checked_add(1)
            .ok_or("Search generation exhausted")?;
        self.bump()?;
        self.serial = serial;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.key = None;
        self.sources.clear();
        self.busy = false;
        self.omitted = 0;
        self.query.clear();
        Ok(())
    }
    pub fn begin(
        &mut self,
        query: &str,
        account: u64,
        access: impl Into<crate::offline::Access>,
        cache: Cache,
        tx: mpsc::Sender<crate::Event>,
    ) -> Result<(), &'static str> {
        let access = access.into();
        if query.chars().take(513).count() > 512 {
            return Err("Search supports at most 512 characters.");
        }
        self.invalidate()?;
        self.query = query.trim().into();
        if self.query.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::new();
        let libraries: Vec<_> = access
            .libraries()
            .into_iter()
            .filter(|library| ids.insert(library.id))
            .collect();
        self.omitted = libraries.len().saturating_sub(SOURCES);
        self.sources = libraries
            .into_iter()
            .take(SOURCES)
            .map(|library| Source {
                library,
                phase: "pending",
                tracks: vec![],
                cache: Info::default(),
                more: false,
                error: None,
            })
            .collect();
        let libraries: Vec<_> = self
            .sources
            .iter()
            .map(|source| source.library.clone())
            .collect();
        let key = Key {
            account,
            serial: self.serial,
        };
        self.key = Some(key);
        self.busy = !libraries.is_empty();
        let query = self.query.clone();
        self.task = Some(tokio::spawn(async move {
            let jobs = futures_util::stream::iter(libraries)
                .map(|library| {
                    let (access, cache, query, tx) =
                        (access.clone(), cache.clone(), query.clone(), tx.clone());
                    async move {
                        let source = library.id;
                        let output = tx.clone();
                        let result = tokio::time::timeout(
                            Duration::from_secs(6),
                            access.read(
                                &cache,
                                metadata_cache::Query::Tracks {
                                    source,
                                    offset: 0,
                                    query,
                                },
                                move |page, info| {
                                    let tx = output.clone();
                                    async move {
                                        let page = page.and_then(|page| match page {
                                            metadata_cache::Page::Tracks(page) => {
                                                Ok(Box::new(page))
                                            }
                                            _ => Err(ProviderError::InvalidReply),
                                        });
                                        let _ = tx
                                            .send(crate::Event::Search(Reply::Page(
                                                key, source, page, info,
                                            )))
                                            .await;
                                    }
                                },
                            ),
                        )
                        .await;
                        if result.is_err() {
                            let _ = tx
                                .send(crate::Event::Search(Reply::Timeout(key, source)))
                                .await;
                        }
                    }
                })
                .buffer_unordered(4)
                .collect::<Vec<_>>();
            let _ = tokio::time::timeout(Duration::from_secs(15), jobs).await;
            let _ = tx.send(crate::Event::Search(Reply::Done(key))).await;
        }));
        Ok(())
    }
    pub fn accept(&mut self, reply: Reply, account: u64) -> Result<(), &'static str> {
        let key = match &reply {
            Reply::Page(key, ..) | Reply::Timeout(key, ..) | Reply::Done(key) => *key,
        };
        if self.key != Some(key) || key.account != account || !self.busy {
            return Ok(());
        }
        self.bump()?;
        match reply {
            Reply::Done(_) => {
                self.busy = false;
                self.task = None;
                for source in &mut self.sources {
                    if matches!(source.phase, "pending" | "refreshing") {
                        source.phase = "timeout";
                        source.error =
                            Some("Search deadline reached; refine or retry the query.".into());
                    }
                }
            }
            Reply::Timeout(_, id) => {
                if let Some(source) = self
                    .sources
                    .iter_mut()
                    .find(|source| source.library.id == id)
                {
                    source.phase = "timeout";
                    source.error =
                        Some("Library search timed out; cached matches may be incomplete.".into());
                }
            }
            Reply::Page(_, id, page, cache) => {
                let Some(source) = self
                    .sources
                    .iter_mut()
                    .find(|source| source.library.id == id)
                else {
                    return Ok(());
                };
                let page = page.and_then(|page| {
                    if page.source != id
                        || page.query != self.query
                        || page.offset.get() != 0
                        || page.tracks.len() > 100
                        || page.tracks.iter().any(|track| track.reference.source != id)
                    {
                        Err(ProviderError::InvalidReply)
                    } else {
                        Ok(page)
                    }
                });
                match page {
                    Ok(page) => {
                        let mut seen = HashSet::new();
                        source.tracks = page
                            .tracks
                            .into_iter()
                            .filter(|track| seen.insert(String::from(track.reference.key.clone())))
                            .collect();
                        source.more = page.has_more;
                        source.phase = if cache.refreshing {
                            "refreshing"
                        } else if cache.stale || cache.incomplete {
                            "cached"
                        } else {
                            "ready"
                        };
                        source.error = None;
                        source.cache = cache;
                    }
                    Err(error) => {
                        source.phase = "failed";
                        source.error = Some(
                            if error == ProviderError::MissingTrack && cache.origin == "offline" {
                                cache.message.into()
                            } else {
                                error.to_string()
                            },
                        );
                        if !matches!(
                            error,
                            ProviderError::Network | ProviderError::SignInRequired
                        ) {
                            source.tracks.clear();
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn rows(&self) -> Vec<Row> {
        self.sources
            .iter()
            .flat_map(|source| {
                source.tracks.iter().map(|track| Row {
                    track: track.clone(),
                    library: source.library.title.clone(),
                    server: source.library.server.clone(),
                    actionable: source.actionable(),
                })
            })
            .take(RESULTS)
            .collect()
    }
    pub fn resolve(&self, request: &Value, account: u64) -> Result<Track, &'static str> {
        if self.key.is_none_or(|key| key.account != account)
            || request["account_epoch"].as_str() != Some(account.to_string().as_str())
            || request["search_revision"].as_str() != Some(self.revision.to_string().as_str())
        {
            return Err("Search results changed; choose a current result.");
        }
        let reference: TrackRef =
            serde_json::from_value(json!({"source": request["source"], "key": request["key"]}))
                .map_err(|_| "Choose a source-labelled search result.")?;
        self.rows()
            .into_iter()
            .find(|row| row.actionable && row.track.reference == reference)
            .map(|row| row.track)
            .ok_or("This result is unavailable or awaiting a fresh library response.")
    }
    pub fn view(&self) -> Value {
        let rows = self.rows();
        let truncated = self.omitted > 0
            || self.sources.iter().any(|source| source.more)
            || self
                .sources
                .iter()
                .map(|source| source.tracks.len())
                .sum::<usize>()
                > RESULTS;
        let partial = truncated || self.sources.iter().any(|source| !source.actionable());
        json!({"revision":self.revision.to_string(),"query":self.query,"busy":self.busy,"partial":partial,
            "truncated":truncated,"omitted_sources":self.omitted,"results":rows,
            "sources":self.sources.iter().map(|source|json!({"id":source.library.id,"library":source.library.title,
                "server":source.library.server,"phase":source.phase,"count":source.tracks.len(),"has_more":source.more,
                "cache":source.cache,"error":source.error})).collect::<Vec<_>>(),
            "message":if self.query.is_empty(){"Search tracks across your music libraries.".into()}else{format!("{} matches · {}/{} libraries ready{}{}", rows.len(),self.sources.iter().filter(|source|source.actionable()).count(),self.sources.len()+self.omitted,if partial{" · partial results"}else{""},if truncated{" · limited scope; refine query"}else{""})}})
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_core::{Counter, ProviderTrackKey};
    use qoda_provider::{MusicProvider, PreparedMedia, ProviderFuture};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Provider {
        libraries: Vec<Library>,
        active: AtomicUsize,
        maximum: AtomicUsize,
        calls: AtomicUsize,
        slow: bool,
        slow_source: Option<SourceId>,
        denied: Option<SourceId>,
        count: usize,
    }
    struct Active<'a>(&'a AtomicUsize);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn track(source: SourceId, key: &str) -> Track {
        Track {
            reference: TrackRef {
                source,
                key: key.to_owned().try_into().unwrap(),
            },
            title: "Same title".into(),
            artist: "Fixture".into(),
            album: "Synthetic".into(),
            duration_ms: None,
        }
    }
    fn provider(count: usize, slow: bool) -> Arc<Provider> {
        Arc::new(Provider {
            libraries: (0..count)
                .map(|index| Library {
                    id: SourceId::new(),
                    title: format!("Music {index}"),
                    server: "Fixture server".into(),
                })
                .collect(),
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            slow,
            slow_source: None,
            denied: None,
            count: 1,
        })
    }
    impl MusicProvider for Provider {
        fn libraries(&self) -> Vec<Library> {
            self.libraries.clone()
        }
        fn tracks(
            &self,
            source: SourceId,
            _: u64,
            query: String,
        ) -> ProviderFuture<'_, CatalogPage> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                let _active = Active(&self.active);
                if self.slow || self.slow_source == Some(source) {
                    std::future::pending::<()>().await;
                }
                if self.denied == Some(source) {
                    return Err(ProviderError::AccessDenied);
                }
                Ok(CatalogPage {
                    source,
                    query,
                    offset: Counter::ZERO,
                    total: None,
                    tracks: (0..self.count)
                        .map(|index| track(source, &index.to_string()))
                        .collect(),
                    has_more: false,
                })
            })
        }
        fn prepare(&self, _: SourceId, _: ProviderTrackKey) -> ProviderFuture<'_, PreparedMedia> {
            panic!("search must never prepare or play media")
        }
    }
    async fn finish(
        controller: &mut Controller,
        rx: &mut mpsc::Receiver<crate::Event>,
        account: u64,
    ) {
        while controller.busy {
            let crate::Event::Search(reply) = rx.recv().await.unwrap() else {
                panic!("unexpected event")
            };
            controller.accept(reply, account).unwrap();
        }
    }
    fn action(controller: &Controller, source: SourceId, key: &str) -> Value {
        json!({"search_revision":controller.revision.to_string(),"account_epoch":"4","source":source,"key":key,"title":"client metadata ignored"})
    }
    #[tokio::test(start_paused = true)]
    async fn identical_provider_keys_remain_distinct_labelled_and_only_current_metadata_is_actionable()
     {
        let provider = provider(2, false);
        let (tx, mut rx) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx.clone())
            .unwrap();
        finish(&mut controller, &mut rx, 4).await;
        let view = controller.view();
        assert_eq!(view["results"].as_array().unwrap().len(), 2);
        assert_ne!(
            view["results"][0]["track"]["reference"]["source"],
            view["results"][1]["track"]["reference"]["source"]
        );
        assert_eq!(view["results"][0]["library"], "Music 0");
        assert_eq!(view["results"][1]["server"], "Fixture server");
        let request = action(&controller, provider.libraries[0].id, "0");
        assert_eq!(controller.resolve(&request, 4).unwrap().title, "Same title");
        assert!(controller.resolve(&request, 5).is_err());
        controller
            .begin("new query", 4, provider, Cache::default(), tx)
            .unwrap();
        assert!(controller.resolve(&request, 4).is_err());
        controller.invalidate().unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn denial_is_an_explicit_partial_source_while_other_libraries_remain_usable() {
        let mut provider = provider(2, false);
        let denied = provider.libraries[1].id;
        Arc::get_mut(&mut provider).unwrap().denied = Some(denied);
        let (tx, mut rx) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx)
            .unwrap();
        finish(&mut controller, &mut rx, 4).await;
        let view = controller.view();
        assert_eq!(view["partial"], true);
        assert_eq!(view["sources"][1]["phase"], "failed");
        assert!(
            view["sources"][1]["error"]
                .as_str()
                .unwrap()
                .contains("denied")
        );
        assert!(
            controller
                .resolve(&action(&controller, provider.libraries[0].id, "0"), 4)
                .is_ok()
        );
        assert!(
            controller
                .resolve(&action(&controller, denied, "0"), 4)
                .is_err()
        );
    }
    #[tokio::test(start_paused = true)]
    async fn a_ready_library_is_actionable_while_another_source_is_still_pending() {
        let mut provider = provider(2, false);
        let slow = provider.libraries[1].id;
        Arc::get_mut(&mut provider).unwrap().slow_source = Some(slow);
        let (tx, mut rx) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx)
            .unwrap();
        let crate::Event::Search(reply) = rx.recv().await.unwrap() else {
            panic!("unexpected event")
        };
        controller.accept(reply, 4).unwrap();
        assert!(controller.busy);
        assert!(
            controller
                .resolve(&action(&controller, provider.libraries[0].id, "0"), 4)
                .is_ok()
        );
        assert_eq!(controller.view()["sources"][1]["phase"], "pending");
        finish(&mut controller, &mut rx, 4).await;
        assert_eq!(controller.view()["sources"][1]["phase"], "timeout");
    }
    #[tokio::test(start_paused = true)]
    async fn concurrency_global_deadline_source_limit_and_cancellation_bound_work() {
        let provider = provider(40, true);
        let (tx, mut rx) = mpsc::channel(128);
        let mut controller = Controller::default();
        let start = tokio::time::Instant::now();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx.clone())
            .unwrap();
        finish(&mut controller, &mut rx, 4).await;
        assert!(start.elapsed() <= Duration::from_secs(15));
        assert!(provider.maximum.load(Ordering::SeqCst) <= 4);
        assert!(provider.calls.load(Ordering::SeqCst) <= SOURCES);
        assert_eq!(provider.active.load(Ordering::SeqCst), 0);
        let view = controller.view();
        assert_eq!(view["omitted_sources"], 8);
        assert_eq!(view["truncated"], true);
        assert!(
            view["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|source| source["phase"] == "timeout")
        );
        controller
            .begin("new", 4, provider.clone(), Cache::default(), tx)
            .unwrap();
        tokio::task::yield_now().await;
        let old = controller.key.unwrap();
        controller.invalidate().unwrap();
        tokio::task::yield_now().await;
        assert_eq!(provider.active.load(Ordering::SeqCst), 0);
        controller
            .accept(
                Reply::Page(
                    old,
                    provider.libraries[0].id,
                    Ok(Box::new(CatalogPage {
                        source: provider.libraries[0].id,
                        query: "new".into(),
                        offset: Counter::ZERO,
                        total: None,
                        tracks: vec![track(provider.libraries[0].id, "0")],
                        has_more: false,
                    })),
                    Info::default(),
                ),
                4,
            )
            .unwrap();
        assert!(controller.view()["results"].as_array().unwrap().is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn aggregate_limit_is_visible_and_hidden_matches_cannot_be_queued() {
        let mut provider = provider(6, false);
        Arc::get_mut(&mut provider).unwrap().count = 100;
        let (tx, mut rx) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx)
            .unwrap();
        finish(&mut controller, &mut rx, 4).await;
        let view = controller.view();
        assert_eq!(view["results"].as_array().unwrap().len(), 500);
        assert_eq!(view["truncated"], true);
        assert!(
            controller
                .resolve(&action(&controller, provider.libraries[5].id, "0"), 4)
                .is_err()
        );
    }
    #[tokio::test(start_paused = true)]
    async fn cache_provenance_and_permission_loss_revoke_actions_and_wrong_source_is_refused() {
        let provider = provider(1, true);
        let source = provider.libraries[0].id;
        let (tx, _) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider, Cache::default(), tx)
            .unwrap();
        let key = controller.key.unwrap();
        let page = || {
            Box::new(CatalogPage {
                source,
                query: "fixture".into(),
                offset: Counter::ZERO,
                total: None,
                tracks: vec![track(source, "0")],
                has_more: false,
            })
        };
        controller
            .accept(
                Reply::Page(
                    key,
                    source,
                    Ok(page()),
                    Info {
                        origin: "cached",
                        stale: true,
                        incomplete: true,
                        refreshing: true,
                        message: "Cached partial fixture",
                    },
                ),
                4,
            )
            .unwrap();
        let old = action(&controller, source, "0");
        assert!(controller.resolve(&old, 4).is_err());
        controller
            .accept(Reply::Page(key, source, Ok(page()), Info::default()), 4)
            .unwrap();
        assert!(controller.resolve(&old, 4).is_err());
        assert!(
            controller
                .resolve(&action(&controller, source, "0"), 4)
                .is_ok()
        );
        controller
            .accept(
                Reply::Page(
                    key,
                    source,
                    Err(ProviderError::AccessDenied),
                    Info::default(),
                ),
                4,
            )
            .unwrap();
        assert!(controller.view()["results"].as_array().unwrap().is_empty());
        let mut invalid = page();
        invalid.tracks[0].reference.source = SourceId::new();
        controller
            .accept(Reply::Page(key, source, Ok(invalid), Info::default()), 4)
            .unwrap();
        assert_eq!(controller.view()["sources"][0]["phase"], "failed");
    }
    #[tokio::test(start_paused = true)]
    async fn blank_queries_do_no_work_and_oversized_queries_do_not_replace_existing_results() {
        let provider = provider(1, false);
        let (tx, mut rx) = mpsc::channel(32);
        let mut controller = Controller::default();
        controller
            .begin("fixture", 4, provider.clone(), Cache::default(), tx.clone())
            .unwrap();
        finish(&mut controller, &mut rx, 4).await;
        let before = controller.view();
        assert!(
            controller
                .begin(
                    &"x".repeat(513),
                    4,
                    provider.clone(),
                    Cache::default(),
                    tx.clone()
                )
                .is_err()
        );
        assert_eq!(controller.view(), before);
        controller
            .begin("  ", 4, provider.clone(), Cache::default(), tx)
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(controller.view()["results"].as_array().unwrap().is_empty());
    }
}
