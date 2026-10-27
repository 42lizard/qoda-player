//! Displayed track catalog owns request lifetime, revision and authoritative selection.
use crate::{
    albums,
    metadata_cache::{Cache, Info, Page, Query},
    offline::Access,
};
use qoda_core::{Counter, SourceId, TrackRef};
use qoda_provider::{CatalogPage, ProviderError, Track};
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::{sync::mpsc, task::JoinHandle};

pub struct Reply {
    generation: u64,
    result: Result<CatalogPage, ProviderError>,
    info: Info,
}
#[derive(Default)]
pub struct Controller {
    page: Option<CatalogPage>,
    revision: u64,
    generation: u64,
    busy: bool,
    cache: Cache,
    info: Info,
    task: Option<JoinHandle<()>>,
}
impl Controller {
    pub fn new(cache: Cache) -> Self {
        let mut this = Self::default();
        this.cache = cache;
        this
    }
    pub fn publish(&self, state: &mut Value) {
        state["page"] = json!(self.page);
        state["catalog"] = json!({"revision":self.revision.to_string(),"busy":self.busy,
            "refreshing":self.info.refreshing,"cache":self.info});
    }
    pub fn invalidate(&mut self) -> Result<(), &'static str> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("Browse generation exhausted")?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("Catalog revision exhausted")?;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = generation;
        self.revision = revision;
        self.page = None;
        self.busy = false;
        self.info = Info::default();
        Ok(())
    }
    pub fn begin(
        &mut self,
        request: &Value,
        access: Access,
        tx: mpsc::Sender<crate::Event>,
    ) -> Result<(), &'static str> {
        let source: SourceId = serde_json::from_value(request["source"].clone())
            .map_err(|_| "Choose a music library.")?;
        let offset: Counter = serde_json::from_value(request["offset"].clone())
            .map_err(|_| "Choose a valid catalog page.")?;
        let query = request["query"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(512)
            .collect();
        self.invalidate()?;
        self.busy = true;
        let generation = self.generation;
        let cache = self.cache.clone();
        self.task = Some(tokio::spawn(async move {
            access
                .read(
                    &cache,
                    Query::Tracks {
                        source,
                        offset: offset.get(),
                        query,
                    },
                    move |result, info| {
                        let tx = tx.clone();
                        async move {
                            let result = result.and_then(|page| match page {
                                Page::Tracks(page) => Ok(page),
                                _ => Err(ProviderError::InvalidReply),
                            });
                            let _ = tx
                                .send(crate::Event::Page(Reply {
                                    generation,
                                    result,
                                    info,
                                }))
                                .await;
                        }
                    },
                )
                .await;
        }));
        Ok(())
    }
    pub fn accept(&mut self, reply: Reply, state: &mut Value) -> Result<bool, &'static str> {
        if reply.generation != self.generation {
            return Ok(false);
        }
        self.busy = false;
        self.info = reply.info;
        match reply.result {
            Ok(page) => {
                // Identical background refreshes retain the exact selection revision.
                if serde_json::to_value(&self.page)
                    .map_err(|_| "Catalog page could not be encoded")?
                    != json!(page)
                {
                    self.revision = self
                        .revision
                        .checked_add(1)
                        .ok_or("Catalog revision exhausted")?;
                }
                self.page = Some(page);
                state["status"] = json!(if self.info.origin == "live" {
                    "Ready"
                } else {
                    self.info.message
                });
            }
            Err(error) => {
                if self.info.origin == "offline" && error == ProviderError::MissingTrack {
                    state["status"] = json!(self.info.message);
                    state["error"] = Value::Null;
                } else {
                    state["status"] = json!("Browse failed");
                    state["error"] = json!(error.to_string());
                }
                if matches!(
                    error,
                    ProviderError::AccessDenied | ProviderError::UnknownSource
                ) && self.page.take().is_some()
                {
                    self.revision = self
                        .revision
                        .checked_add(1)
                        .ok_or("Catalog revision exhausted")?;
                }
            }
        }
        Ok(true)
    }
    pub fn track(
        &self,
        albums: &albums::Controller,
        request: &Value,
    ) -> Result<Track, &'static str> {
        let reference = serde_json::from_value::<TrackRef>(
            json!({"source":request["source"],"key":request["key"]}),
        )
        .map_err(|_| "Choose a track from the current catalog page.")?;
        self.page
            .as_ref()
            .into_iter()
            .flat_map(|p| &p.tracks)
            .find(|t| t.reference == reference)
            .cloned()
            .or_else(|| albums.track(&reference))
            .ok_or("That track is no longer on the displayed page. Browse again.")
    }
    pub fn selected_tracks(
        &self,
        request: &Value,
        account: u64,
        connected: bool,
    ) -> Result<Vec<Track>, &'static str> {
        if request["account_epoch"].as_str() != Some(account.to_string().as_str()) || !connected {
            return Err("The account changed. Connect and select tracks again.");
        }
        if self.info.origin == "offline" {
            return Err("This cached source is offline. Reconnect and select tracks again.");
        }
        if self.busy
            || request["catalog_revision"].as_str() != Some(self.revision.to_string().as_str())
        {
            return Err("The catalog page changed. Select tracks from the current page.");
        }
        let references = request["tracks"]
            .as_array()
            .filter(|tracks| !tracks.is_empty() && tracks.len() <= 100)
            .ok_or("Select between 1 and 100 tracks on the current page.")?;
        let page = self
            .page
            .as_ref()
            .ok_or("Load a catalog page before selecting tracks.")?;
        let mut selected = HashSet::new();
        for reference in references {
            let reference: TrackRef = serde_json::from_value(reference.clone())
                .map_err(|_| "Invalid selected track reference.")?;
            if reference.source != page.source || !selected.insert(String::from(reference.key)) {
                return Err("The selection contains a different source or duplicate reference.");
            }
        }
        let tracks: Vec<_> = page
            .tracks
            .iter()
            .filter(|track| {
                track.reference.source == page.source
                    && selected.contains(&String::from(track.reference.key.clone()))
            })
            .cloned()
            .collect();
        let found: HashSet<_> = tracks
            .iter()
            .map(|track| String::from(track.reference.key.clone()))
            .collect();
        if tracks.len() != selected.len() || found != selected {
            return Err("Some selected tracks left this page. Nothing was partially queued.");
        }
        Ok(tracks)
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
#[path = "catalog/tests.rs"]
pub(crate) mod tests;
