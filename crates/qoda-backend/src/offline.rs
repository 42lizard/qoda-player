//! Cached discovery is a display allowlist, never a playback provider or credential.
use crate::metadata_cache::{Cache, Info, Page, Query};
use qoda_provider::{Library, MusicProvider, ProviderError};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, future::Future, sync::Arc};

pub const MAX_DISCOVERY: usize = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub version: u8,
    pub account: String,
    pub libraries: Vec<Library>,
    pub fetched: i64,
}
impl Discovery {
    pub fn valid(&self) -> bool {
        let mut ids = HashSet::new();
        self.version == 1
            && self.account.len() == 64
            && self.account.bytes().all(|b| b.is_ascii_hexdigit())
            && self.fetched > 0
            && self.libraries.len() <= 128
            && self.libraries.iter().all(|library| {
                ids.insert(library.id)
                    && !library.title.trim().is_empty()
                    && library.title.len() <= 1024
                    && library.server.len() <= 1024
            })
    }
}
#[derive(Clone)]
pub enum Access {
    Live(Arc<dyn MusicProvider>),
    Cached(Discovery),
    Hybrid(Arc<dyn MusicProvider>, Discovery),
}
impl Access {
    pub fn libraries(&self) -> Vec<Library> {
        match self {
            Self::Live(provider) => provider.libraries(),
            Self::Cached(discovery) => discovery.libraries.clone(),
            Self::Hybrid(provider, discovery) => {
                let mut libraries = provider.libraries();
                let live: HashSet<_> = libraries.iter().map(|l| l.id).collect();
                libraries.extend(
                    discovery
                        .libraries
                        .iter()
                        .filter(|l| !live.contains(&l.id))
                        .cloned(),
                );
                libraries
            }
        }
    }
    pub async fn read<F, Fut>(&self, cache: &Cache, query: Query, emit: F)
    where
        F: FnMut(Result<Page, ProviderError>, Info) -> Fut,
        Fut: Future<Output = ()>,
    {
        match self {
            Self::Live(provider) => cache.read(provider.clone(), query, emit).await,
            Self::Cached(discovery) => cache.read_offline(discovery, query, emit).await,
            Self::Hybrid(provider, discovery) => {
                if provider.libraries().iter().any(|l| l.id == query.source()) {
                    cache.read(provider.clone(), query, emit).await;
                } else {
                    cache.read_offline(discovery, query, emit).await;
                }
            }
        }
    }
}

impl<T: MusicProvider + 'static> From<Arc<T>> for Access {
    fn from(provider: Arc<T>) -> Self {
        Self::Live(provider)
    }
}
impl From<Arc<dyn MusicProvider>> for Access {
    fn from(provider: Arc<dyn MusicProvider>) -> Self {
        Self::Live(provider)
    }
}

pub fn access(
    provider: Option<Arc<dyn MusicProvider>>,
    cached: Option<Discovery>,
) -> Option<Access> {
    match (provider, cached) {
        (Some(provider), Some(cached)) => Some(Access::Hybrid(provider, cached)),
        (Some(provider), None) => Some(Access::Live(provider)),
        (None, Some(cached)) => Some(Access::Cached(cached)),
        (None, None) => None,
    }
}
pub fn library_views(libraries: &[Library], live: &[Library]) -> serde_json::Value {
    let live: HashSet<_> = live.iter().map(|l| l.id).collect();
    serde_json::Value::Array(libraries.iter().map(|l|serde_json::json!({"id":l.id,"title":l.title,"server":l.server,"offline":!live.contains(&l.id)})).collect())
}

pub fn reconcile(
    previous: Option<&Discovery>,
    account: String,
    live: Vec<Library>,
    unavailable: &HashSet<qoda_core::SourceId>,
    fetched: i64,
) -> Discovery {
    let mut libraries = live;
    if let Some(previous) = previous.filter(|d| d.account == account && d.valid()) {
        let present: HashSet<_> = libraries.iter().map(|l| l.id).collect();
        libraries.extend(
            previous
                .libraries
                .iter()
                .filter(|l| !present.contains(&l.id) && unavailable.contains(&l.id))
                .cloned(),
        );
    }
    Discovery {
        version: 1,
        account,
        libraries,
        fetched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_core::SourceId;
    #[test]
    fn partial_reconnect_retains_only_same_account_entitled_unreachable_sources() {
        let live = Library {
            id: SourceId::new(),
            title: "Live".into(),
            server: "Reachable".into(),
        };
        let unavailable = Library {
            id: SourceId::new(),
            title: "Cached".into(),
            server: "Unreachable".into(),
        };
        let removed = Library {
            id: SourceId::new(),
            title: "Removed".into(),
            server: "Denied or removed".into(),
        };
        let previous = Discovery {
            version: 1,
            account: "a".repeat(64),
            fetched: 1,
            libraries: vec![live.clone(), unavailable.clone(), removed],
        };
        let allowed = HashSet::from([unavailable.id]);
        let same = reconcile(
            Some(&previous),
            "a".repeat(64),
            vec![live.clone()],
            &allowed,
            2,
        );
        assert_eq!(same.libraries.len(), 2);
        assert_eq!(same.libraries[1].id, unavailable.id);
        let other = reconcile(
            Some(&previous),
            "b".repeat(64),
            vec![live.clone()],
            &allowed,
            2,
        );
        assert_eq!(other.libraries.len(), 1);
        let gone = reconcile(Some(&previous), "a".repeat(64), vec![], &HashSet::new(), 2);
        assert!(gone.libraries.is_empty());
        let only_cache = reconcile(Some(&previous), "a".repeat(64), vec![], &allowed, 2);
        assert_eq!(only_cache.libraries.len(), 1);
        assert_eq!(only_cache.libraries[0].id, unavailable.id);
        let view = library_views(&same.libraries, &[live]);
        assert_eq!(view[0]["offline"], false);
        assert_eq!(view[1]["offline"], true);
    }
}
