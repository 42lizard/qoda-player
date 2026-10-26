//! One playlist operation at a time. Unknown writes require an explicit fresh
//! read; changing accounts invalidates every old reply without replaying it.
use qoda_core::{Counter, SourceId};
use qoda_provider::*;
use serde::Serialize;
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::mpsc;
#[derive(Default, Serialize)]
pub struct View {
    pub list: Option<PlaylistPage>,
    pub selected: Option<PlaylistSnapshot>,
    pub busy: bool,
    pub uncertain: bool,
    pub message: String,
}
pub struct Reply {
    pub auth: u64,
    pub generation: u64,
    pub result: Result<Update, ProviderError>,
}
pub enum Update {
    List(PlaylistPage),
    Open(PlaylistSnapshot),
    Refresh(PlaylistPage, Option<PlaylistSnapshot>),
    Edit(PlaylistOutcome),
}
#[derive(Default)]
pub struct Controller {
    pub view: View,
    generation: u64,
    task: Option<tokio::task::JoinHandle<()>>,
    writing: bool,
    seen: VecDeque<String>,
}
impl Controller {
    pub fn invalidate(&mut self) {
        self.generation = self.generation.saturating_add(1);
        if !self.writing
            && let Some(task) = self.task.take()
        {
            task.abort();
        }
        // A sent write cannot be unsent. Its eventual reply cannot repopulate this view.
        self.task = None;
        self.writing = false;
        self.view = View::default();
        self.seen.clear();
    }
    pub fn command(
        &mut self,
        request: &Value,
        provider: Arc<dyn MusicProvider>,
        auth: u64,
        events: mpsc::Sender<crate::Event>,
    ) -> Result<(), String> {
        if self.view.busy {
            return Err("Wait for the current playlist operation to finish.".into());
        }
        let op = request["op"].as_str().unwrap_or_default();
        let writing = matches!(op, "playlist_create" | "playlist_edit");
        if writing && self.view.uncertain {
            return Err(ProviderError::UncertainMutation.to_string());
        }
        let source = || {
            serde_json::from_value::<SourceId>(request["source"].clone())
                .map_err(|_| ProviderError::UnknownSource.to_string())
        };
        let reference = || {
            serde_json::from_value::<PlaylistRef>(request["reference"].clone())
                .map_err(|_| ProviderError::InvalidPlaylist.to_string())
        };
        let offset = || {
            serde_json::from_value::<Counter>(request["offset"].clone())
                .map(|n| n.get())
                .map_err(|_| ProviderError::InvalidPlaylist.to_string())
        };
        enum Action {
            List(SourceId, u64),
            Open(PlaylistRef),
            Refresh(SourceId, Option<PlaylistRef>),
            Edit(PlaylistChange),
        }
        let action = match op {
            "playlist_list" => Action::List(source()?, offset()?),
            "playlist_open" => {
                let reference = reference()?;
                if self
                    .view
                    .list
                    .as_ref()
                    .is_some_and(|list| list.source != reference.source)
                {
                    return Err(ProviderError::PlaylistChanged.to_string());
                }
                Action::Open(reference)
            }
            "playlist_refresh" => {
                let source = source()?;
                let reference = self
                    .view
                    .selected
                    .as_ref()
                    .filter(|view| view.playlist.reference.source == source)
                    .map(|view| view.playlist.reference.clone());
                Action::Refresh(source, reference)
            }
            "playlist_create" => {
                let title = request["title"]
                    .as_str()
                    .ok_or("Enter a playlist title.")?
                    .to_owned();
                let seed =
                    serde_json::from_value::<Option<qoda_core::TrackRef>>(request["seed"].clone())
                        .map_err(|_| ProviderError::InvalidPlaylist.to_string())?;
                Action::Edit(PlaylistChange::Create {
                    source: source()?,
                    title,
                    seed,
                })
            }
            "playlist_edit" => {
                let reference = reference()?;
                let selected = self.checked_snapshot(
                    &reference,
                    request["expected_revision"].as_str().unwrap_or_default(),
                )?;
                if !selected.playlist.can_edit {
                    return Err(ProviderError::ReadOnly.to_string());
                }
                let edit = serde_json::from_value::<PlaylistEdit>(request["edit"].clone())
                    .map_err(|_| ProviderError::InvalidPlaylist.to_string())?;
                Action::Edit(PlaylistChange::Update {
                    reference,
                    expected_revision: selected.revision.clone(),
                    edit,
                })
            }
            _ => return Err("Unknown playlist action.".into()),
        };
        if writing {
            let id = request["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .ok_or("A unique playlist operation ID is required.")?;
            if self.seen.iter().any(|seen| seen == id) {
                return Err(
                    "This playlist operation was already submitted. Refresh to review its result."
                        .into(),
                );
            }
            self.seen.push_back(id.to_owned());
            while self.seen.len() > 128 {
                self.seen.pop_front();
            }
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Playlist generation exhausted")?;
        let generation = self.generation;
        self.view.busy = true;
        self.writing = writing;
        self.view.message = if writing {
            "Saving playlist to Plex…"
        } else {
            "Loading playlists…"
        }
        .into();
        self.task = Some(tokio::spawn(async move {
            let work = async {
                match action {
                    Action::List(source, offset) => {
                        provider.playlists(source, offset).await.map(Update::List)
                    }
                    Action::Open(reference) => provider.playlist(reference).await.map(Update::Open),
                    Action::Refresh(source, reference) => {
                        let list = provider.playlists(source, 0).await?;
                        let selected = if let Some(reference) = reference {
                            match provider.playlist(reference).await {
                                Ok(s) => Some(s),
                                Err(ProviderError::MissingTrack) => None,
                                Err(e) => return Err(e),
                            }
                        } else {
                            None
                        };
                        Ok(Update::Refresh(list, selected))
                    }
                    Action::Edit(change) => provider.edit_playlist(change).await.map(Update::Edit),
                }
            };
            let result = tokio::time::timeout(std::time::Duration::from_secs(45), work)
                .await
                .unwrap_or(Err(if writing {
                    ProviderError::UncertainMutation
                } else {
                    ProviderError::Network
                }));
            let _ = events
                .send(crate::Event::Playlists(Reply {
                    auth,
                    generation,
                    result,
                }))
                .await;
        }));
        Ok(())
    }
    pub fn checked_snapshot(
        &self,
        reference: &PlaylistRef,
        revision: &str,
    ) -> Result<&PlaylistSnapshot, String> {
        let view = self
            .view
            .selected
            .as_ref()
            .filter(|view| view.playlist.reference == *reference && view.revision == revision)
            .ok_or_else(|| ProviderError::PlaylistChanged.to_string())?;
        if self.view.busy || self.view.uncertain {
            return Err("Refresh and wait for the playlist before using this snapshot.".into());
        }
        Ok(view)
    }
    pub fn apply(&mut self, reply: Reply, auth: u64) {
        if reply.auth != auth || reply.generation != self.generation {
            return;
        }
        self.task = None;
        self.view.busy = false;
        self.writing = false;
        match reply.result {
            Ok(Update::List(page)) => {
                self.view.list = Some(page);
                self.view.selected = None;
                self.view.message = if self.view.uncertain {
                    "Previous edit outcome is unverified. Use Refresh from Plex before editing."
                } else {
                    "Choose a playlist on this server."
                }
                .into();
            }
            Ok(Update::Open(snapshot)) => {
                self.view.selected = Some(snapshot);
                self.view.message=if self.view.uncertain{"Previous edit outcome is unverified. Use Refresh from Plex before editing."}else{"Playlist loaded. Playback and queue edits are separate from saved playlist edits."}.into();
            }
            Ok(Update::Refresh(page, selected)) => {
                self.view.list = Some(page);
                self.view.selected = selected;
                self.view.uncertain = false;
                self.view.message =
                    "Refreshed from Plex. Review the result before making another edit.".into();
            }
            Ok(Update::Edit(PlaylistOutcome::Saved(snapshot))) => {
                if let Some(list) = self.view.list.as_mut() {
                    if let Some(row) = list
                        .playlists
                        .iter_mut()
                        .find(|row| row.reference == snapshot.playlist.reference)
                    {
                        *row = snapshot.playlist.clone();
                    } else {
                        self.view.list = None;
                    }
                }
                self.view.selected = Some(snapshot);
                self.view.message =
                    "Playlist saved and read back from Plex. Local queue unchanged.".into();
            }
            Ok(Update::Edit(PlaylistOutcome::Deleted)) => {
                self.view.selected = None;
                self.view.list = None;
                self.view.message = "Playlist deleted from Plex. Local queue unchanged.".into();
            }
            Err(error) => {
                self.view.uncertain = matches!(
                    error,
                    ProviderError::UncertainMutation
                        | ProviderError::PlaylistChanged
                        | ProviderError::PlaylistDuplicateUnconfirmed
                );
                self.view.message = error.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fixture {
        snapshot: PlaylistSnapshot,
        writes: AtomicUsize,
    }
    impl Fixture {
        fn new() -> Arc<Self> {
            let source = SourceId::new();
            Arc::new(Self {
                snapshot: PlaylistSnapshot {
                    playlist: Playlist {
                        reference: PlaylistRef {
                            source,
                            key: "10".into(),
                        },
                        title: "Fixture".into(),
                        smart: false,
                        can_edit: true,
                        item_count: Counter::ZERO,
                    },
                    revision: "snapshot-v1".into(),
                    entries: Vec::new(),
                },
                writes: AtomicUsize::new(0),
            })
        }
    }
    impl MusicProvider for Fixture {
        fn libraries(&self) -> Vec<Library> {
            Vec::new()
        }
        fn tracks(&self, _: SourceId, _: u64, _: String) -> ProviderFuture<'_, CatalogPage> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn prepare(
            &self,
            _: SourceId,
            _: qoda_core::ProviderTrackKey,
        ) -> ProviderFuture<'_, PreparedMedia> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn playlists(&self, source: SourceId, offset: u64) -> ProviderFuture<'_, PlaylistPage> {
            Box::pin(async move {
                Ok(PlaylistPage {
                    source,
                    offset: Counter::new(offset),
                    playlists: vec![self.snapshot.playlist.clone()],
                    has_more: false,
                })
            })
        }
        fn playlist(&self, _: PlaylistRef) -> ProviderFuture<'_, PlaylistSnapshot> {
            Box::pin(async { Ok(self.snapshot.clone()) })
        }
        fn edit_playlist(&self, _: PlaylistChange) -> ProviderFuture<'_, PlaylistOutcome> {
            Box::pin(async {
                self.writes.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::UncertainMutation)
            })
        }
    }
    async fn apply(c: &mut Controller, rx: &mut mpsc::Receiver<crate::Event>) {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let crate::Event::Playlists(reply) = event else {
            panic!("wrong event")
        };
        c.apply(reply, 1);
    }
    #[tokio::test]
    async fn uncertainty_blocks_writes_until_explicit_refresh_and_deduplicates_operation_ids() {
        let p = Fixture::new();
        let (mut c, (tx, mut rx)) = (Controller::default(), mpsc::channel(8));
        c.command(
            &serde_json::json!({"op":"playlist_open","reference":p.snapshot.playlist.reference}),
            p.clone(),
            1,
            tx.clone(),
        )
        .unwrap();
        apply(&mut c, &mut rx).await;
        let mut request = serde_json::json!({"op":"playlist_edit","id":"operation-1","reference":p.snapshot.playlist.reference,"expected_revision":"snapshot-v1","edit":{"action":"rename","title":"changed"}});
        c.command(&request, p.clone(), 1, tx.clone()).unwrap();
        assert!(c.command(&request, p.clone(), 1, tx.clone()).is_err());
        apply(&mut c, &mut rx).await;
        assert!(c.view.uncertain);
        request["id"] = serde_json::json!("operation-2");
        assert!(c.command(&request, p.clone(), 1, tx.clone()).is_err());
        assert_eq!(p.writes.load(Ordering::SeqCst), 1);
        c.command(&serde_json::json!({"op":"playlist_refresh","source":p.snapshot.playlist.reference.source}),p.clone(),1,tx.clone()).unwrap();
        apply(&mut c, &mut rx).await;
        assert!(!c.view.uncertain);
        request["id"] = serde_json::json!("operation-1");
        assert!(c.command(&request, p.clone(), 1, tx.clone()).is_err());
        request["id"] = serde_json::json!("operation-2");
        c.command(&request, p.clone(), 1, tx).unwrap();
        apply(&mut c, &mut rx).await;
        assert_eq!(p.writes.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn unconfirmed_duplicate_keeps_the_explicit_refresh_fence_and_specific_message() {
        let p = Fixture::new();
        let mut controller = Controller::default();
        controller.view.selected = Some(p.snapshot.clone());
        controller.apply(
            Reply {
                auth: 1,
                generation: controller.generation,
                result: Err(ProviderError::PlaylistDuplicateUnconfirmed),
            },
            1,
        );
        assert!(controller.view.uncertain);
        assert!(
            controller
                .view
                .message
                .contains("collapse playlist duplicates")
        );
        let (tx, mut rx) = mpsc::channel(4);
        assert!(controller.command(&serde_json::json!({"op":"playlist_edit","id":"duplicate-retry","reference":p.snapshot.playlist.reference,"expected_revision":"snapshot-v1","edit":{"action":"rename","title":"blocked"}}),p.clone(),1,tx.clone()).is_err());
        assert_eq!(p.writes.load(Ordering::SeqCst), 0);
        controller.command(&serde_json::json!({"op":"playlist_refresh","source":p.snapshot.playlist.reference.source}),p,1,tx).unwrap();
        apply(&mut controller, &mut rx).await;
        assert!(!controller.view.uncertain);
    }
    #[tokio::test]
    async fn switching_source_drops_the_old_server_selection_and_rejects_stale_open() {
        let p = Fixture::new();
        let mut c = Controller::default();
        c.view.selected = Some(p.snapshot.clone());
        let source = SourceId::new();
        c.view.list = Some(PlaylistPage {
            source,
            offset: Counter::ZERO,
            playlists: Vec::new(),
            has_more: false,
        });
        let (tx, mut rx) = mpsc::channel(4);
        assert!(c.command(&serde_json::json!({"op":"playlist_open","reference":p.snapshot.playlist.reference}),p.clone(),1,tx.clone()).is_err());
        c.command(
            &serde_json::json!({"op":"playlist_refresh","source":source}),
            p,
            1,
            tx,
        )
        .unwrap();
        apply(&mut c, &mut rx).await;
        assert!(c.view.selected.is_none());
        assert_eq!(c.view.list.unwrap().source, source);
    }
    #[test]
    fn old_authentication_replies_cannot_repopulate_a_signed_out_view() {
        let mut c = Controller::default();
        let p = Fixture::new();
        c.generation = 1;
        c.view.selected = Some(p.snapshot.clone());
        c.invalidate();
        c.apply(
            Reply {
                auth: 1,
                generation: 1,
                result: Ok(Update::Open(p.snapshot.clone())),
            },
            2,
        );
        assert!(c.view.selected.is_none());
        assert!(c.view.list.is_none());
    }
}
