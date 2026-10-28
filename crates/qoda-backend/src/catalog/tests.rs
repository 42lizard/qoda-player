use super::*;
use qoda_core::{Counter, SourceId};
use qoda_provider::{Library, MusicProvider, PreparedMedia, ProviderFuture};
use std::{sync::Arc, time::Duration};
pub(crate) struct Provider(pub(crate) CatalogPage);
impl MusicProvider for Provider {
    fn libraries(&self) -> Vec<Library> {
        vec![Library {
            id: self.0.source,
            title: "Fixture".into(),
            server: "Synthetic".into(),
        }]
    }
    fn tracks(&self, _: SourceId, _: u64, _: String) -> ProviderFuture<'_, CatalogPage> {
        Box::pin(async { Ok(self.0.clone()) })
    }
    fn prepare(
        &self,
        _: SourceId,
        _: qoda_core::ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
}
pub(crate) fn page() -> CatalogPage {
    let source = SourceId::new();
    CatalogPage {
        source,
        offset: Counter::ZERO,
        total: None,
        query: "fixture".into(),
        has_more: false,
        tracks: ["A", "B", "C"]
            .map(|key| Track {
                reference: TrackRef {
                    source,
                    key: key.to_owned().try_into().unwrap(),
                },
                title: key.into(),
                artist: "Fixture".into(),
                album: "Synthetic".into(),
                duration_ms: None,
            })
            .to_vec(),
    }
}
fn browse(page: &CatalogPage) -> Value {
    json!({"source": page.source, "offset":"0", "query":"fixture"})
}
fn selection(catalog: &Controller, page: &CatalogPage) -> Value {
    let mut state = json!({});
    catalog.publish(&mut state);
    json!({"account_epoch":"4", "catalog_revision": state["catalog"]["revision"], "tracks":[page.tracks[2].reference, page.tracks[0].reference]})
}
async fn reply(rx: &mut mpsc::Receiver<crate::Event>) -> Reply {
    let crate::Event::Page(reply) = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("Expected catalog reply")
    };
    reply
}
#[tokio::test]
async fn delayed_old_page_cannot_repopulate_after_new_browse_or_invalidation() {
    let page = page();
    let provider = Arc::new(Provider(page.clone()));
    let mut catalog = Controller::new(Cache::default());
    let (tx, mut rx) = mpsc::channel(8);
    catalog
        .begin(&browse(&page), provider.clone().into(), tx.clone())
        .unwrap();
    let old = reply(&mut rx).await;
    catalog.begin(&browse(&page), provider.into(), tx).unwrap();
    assert!(!catalog.accept(old, &mut json!({})).unwrap());
    let latest = reply(&mut rx).await;
    catalog.invalidate().unwrap();
    assert!(!catalog.accept(latest, &mut json!({})).unwrap());
    assert!(
        catalog
            .selected_tracks(&selection(&catalog, &page), 4, true)
            .is_err()
    );
}

#[tokio::test]
async fn typed_selection_preserves_page_order_authority_and_all_or_nothing_fences() {
    let page = page();
    let mut catalog = Controller::new(Cache::default());
    let (tx, mut rx) = mpsc::channel(8);
    catalog
        .begin(
            &browse(&page),
            Arc::new(Provider(page.clone())).into(),
            tx.clone(),
        )
        .unwrap();
    catalog
        .accept(reply(&mut rx).await, &mut json!({}))
        .unwrap();
    let mut request = selection(&catalog, &page);
    request["tracks"][0]["title"] = json!("Untrusted title");
    let tracks = catalog.selected_tracks(&request, 4, true).unwrap();
    assert_eq!(
        tracks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["A", "C"]
    );
    let single = json!({"source":page.source,"key":"A","title":"Untrusted title"});
    assert_eq!(
        catalog
            .track(&albums::Controller::default(), &single)
            .unwrap()
            .title,
        "A"
    );
    assert!(catalog.selected_tracks(&request, 5, true).is_err());
    assert!(catalog.selected_tracks(&request, 4, false).is_err());
    let mut invalid = request.clone();
    invalid["catalog_revision"] = json!(0);
    assert!(catalog.selected_tracks(&invalid, 4, true).is_err());
    let mut wrong_source = page.tracks[0].reference.clone();
    wrong_source.source = SourceId::new();
    for references in [
        json!([]),
        json!([null]),
        json!([page.tracks[0].reference, page.tracks[0].reference]),
        json!(vec![page.tracks[0].reference.clone(); 101]),
        json!([wrong_source]),
        json!([{"source":page.source,"key":"missing"}]),
    ] {
        invalid = request.clone();
        invalid["tracks"] = references;
        assert!(catalog.selected_tracks(&invalid, 4, true).is_err());
    }
    catalog
        .begin(&browse(&page), Arc::new(Provider(page.clone())).into(), tx)
        .unwrap();
    assert!(catalog.selected_tracks(&request, 4, true).is_err());
    catalog
        .accept(reply(&mut rx).await, &mut json!({}))
        .unwrap();
    assert!(catalog.selected_tracks(&request, 4, true).is_err());
}

#[tokio::test]
async fn cached_live_refresh_retains_identical_selection_and_rejects_changed_or_offline_pages() {
    for changed in [false, true] {
        let root = std::env::temp_dir().join(format!("qoda-catalog-{}", uuid::Uuid::new_v4()));
        let cache = Cache::start(Ok(crate::metadata_cache::Paths {
            cache: root.join("cache"),
            settings: root.join("settings"),
        }))
        .await;
        let seed = page();
        let provider = Arc::new(Provider(seed.clone()));
        cache
            .read(
                provider.clone(),
                Query::Tracks {
                    source: seed.source,
                    offset: 0,
                    query: seed.query.clone(),
                },
                |_, _| async {},
            )
            .await;
        let mut live = seed.clone();
        live.query = "A".into();
        live.tracks.truncate(if changed { 2 } else { 1 });
        let mut catalog = Controller::new(cache.clone());
        let (tx, mut rx) = mpsc::channel(8);
        let mut request = browse(&seed);
        request["query"] = json!("A");
        catalog
            .begin(&request, Arc::new(Provider(live)).into(), tx.clone())
            .unwrap();
        catalog
            .accept(reply(&mut rx).await, &mut json!({}))
            .unwrap();
        let mut state = json!({});
        catalog.publish(&mut state);
        assert_eq!(state["catalog"]["cache"]["origin"], "cached_search");
        let selected = json!({"account_epoch":"4", "catalog_revision": state["catalog"]["revision"], "tracks":[seed.tracks[0].reference]});
        assert!(catalog.selected_tracks(&selected, 4, true).is_ok());
        catalog
            .accept(reply(&mut rx).await, &mut json!({}))
            .unwrap();
        assert_eq!(
            catalog.selected_tracks(&selected, 4, true).is_err(),
            changed
        );
        catalog.publish(&mut state);
        assert_eq!(state["catalog"]["cache"]["origin"], "live");
        let offline = crate::offline::Discovery {
            version: 1,
            account: "a".repeat(64),
            libraries: provider.libraries(),
            fetched: 1,
        };
        catalog
            .begin(&browse(&seed), Access::Cached(offline), tx)
            .unwrap();
        catalog
            .accept(reply(&mut rx).await, &mut json!({}))
            .unwrap();
        catalog.publish(&mut state);
        assert_eq!(state["catalog"]["cache"]["origin"], "offline");
        assert!(
            catalog
                .selected_tracks(&selection(&catalog, &seed), 4, true)
                .is_err()
        );
        cache.clear().await;
        catalog.invalidate().unwrap();
        assert!(
            catalog
                .track(
                    &albums::Controller::default(),
                    &json!({"source":seed.source,"key":"A"})
                )
                .is_err()
        );
        drop(catalog);
        cache.shutdown().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn local_scan_invalidation_preserves_other_sources_and_fences_pending_local_reads() {
    let page = page();
    let mut catalog = Controller::new(Cache::default());
    let (tx, mut rx) = mpsc::channel(8);
    catalog
        .begin(
            &browse(&page),
            Arc::new(Provider(page.clone())).into(),
            tx.clone(),
        )
        .unwrap();
    let pending = reply(&mut rx).await;
    catalog
        .invalidate_sources(&HashSet::from([SourceId::new()]))
        .unwrap();
    assert!(catalog.accept(pending, &mut json!({})).unwrap());
    assert_eq!(catalog.page.as_ref().unwrap().source, page.source);
    catalog
        .begin(&browse(&page), Arc::new(Provider(page.clone())).into(), tx)
        .unwrap();
    let pending = reply(&mut rx).await;
    catalog
        .invalidate_sources(&HashSet::from([page.source]))
        .unwrap();
    assert!(!catalog.accept(pending, &mut json!({})).unwrap());
    assert!(catalog.page.is_none() && !catalog.busy);
}
