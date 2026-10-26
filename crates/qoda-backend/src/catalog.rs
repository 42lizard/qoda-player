//! Resolve bulk actions from one current catalog page; never trust client metadata.
use qoda_core::TrackRef;
use qoda_provider::{CatalogPage, Track};
use serde_json::Value;
use std::collections::HashSet;

pub fn is_batch(op: &str) -> bool {
    matches!(
        op,
        "play_selected" | "enqueue_selected" | "play_next_selected"
    )
}
pub fn selected_tracks(
    state: &Value,
    request: &Value,
    account: u64,
) -> Result<Vec<Track>, &'static str> {
    if request["account_epoch"].as_str() != Some(account.to_string().as_str())
        || state["sign_in"]["connected"] != true
    {
        return Err("The account changed. Connect and select tracks again.");
    }
    if state["catalog"]["cache"]["origin"] == "offline" {
        return Err("This cached source is offline. Reconnect and select tracks again.");
    }
    if state["catalog"]["busy"] != false
        || request["catalog_revision"].as_str().is_none()
        || request["catalog_revision"] != state["catalog"]["revision"]
    {
        return Err("The catalog page changed. Select tracks from the current page.");
    }
    let references = request["tracks"]
        .as_array()
        .filter(|tracks| !tracks.is_empty() && tracks.len() <= 100)
        .ok_or("Select between 1 and 100 tracks on the current page.")?;
    let page: CatalogPage = serde_json::from_value(state["page"].clone())
        .map_err(|_| "Load a catalog page before selecting tracks.")?;
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
        .into_iter()
        .filter(|track| {
            track.reference.source == page.source
                && selected.contains(&String::from(track.reference.key.clone()))
        })
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

/// A background refresh invalidates selections only when the displayed contents change.
pub fn publish_page(
    state: &mut Value,
    revision: &mut u64,
    page: Option<CatalogPage>,
) -> Result<(), &'static str> {
    let page = serde_json::to_value(page).map_err(|_| "Catalog page could not be encoded")?;
    if state["page"] != page {
        *revision = revision
            .checked_add(1)
            .ok_or("Catalog revision exhausted")?;
    }
    state["page"] = page;
    state["catalog"]["revision"] = serde_json::json!(revision.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cached_only_source_cannot_queue_even_when_other_libraries_are_connected() {
        let (mut state, request) = fixture();
        assert!(selected_tracks(&state, &request, 4).is_ok());
        state["catalog"]["cache"] = json!({"origin":"offline","stale":true,"incomplete":true});
        assert!(selected_tracks(&state, &request, 4).is_err());
    }
    use super::*;
    use qoda_core::SourceId;
    use serde_json::json;
    fn fixture() -> (Value, Value) {
        let source = SourceId::new();
        let tracks: Vec<_> = ["A", "B", "C"]
            .into_iter()
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
            .collect();
        let state = json!({"sign_in":{"connected":true},"catalog":{"revision":"9007199254740993","busy":false},"page":CatalogPage{source,offset:qoda_core::Counter::ZERO,total:None,query:"fixture".into(),tracks:tracks.clone(),has_more:false}});
        let request = json!({"account_epoch":"4","catalog_revision":"9007199254740993","tracks":[tracks[2].reference,tracks[0].reference]});
        (state, request)
    }
    #[test]
    fn identical_refresh_retains_selection_but_changed_cached_contents_reject_old_actions() {
        let (mut state, request) = fixture();
        let mut revision = 9_007_199_254_740_993;
        let page: CatalogPage = serde_json::from_value(state["page"].clone()).unwrap();
        publish_page(&mut state, &mut revision, Some(page.clone())).unwrap();
        assert!(selected_tracks(&state, &request, 4).is_ok());
        let mut changed = page;
        changed.tracks.swap(0, 1);
        publish_page(&mut state, &mut revision, Some(changed)).unwrap();
        assert_eq!(state["catalog"]["revision"], "9007199254740994");
        assert!(selected_tracks(&state, &request, 4).is_err());
        publish_page(&mut state, &mut revision, None).unwrap();
        assert!(state["page"].is_null());
    }
    #[test]
    fn batch_uses_current_server_metadata_and_display_order_with_lossless_page_fences() {
        let (state, mut request) = fixture();
        request["tracks"][0]["title"] = json!("Untrusted title");
        let tracks = selected_tracks(&state, &request, 4).unwrap();
        assert_eq!(
            tracks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["A", "C"]
        );
        request["catalog_revision"] = json!(9007199254740993_u64);
        assert!(selected_tracks(&state, &request, 4).is_err());
    }
    #[test]
    fn stale_account_page_and_in_flight_browse_cannot_reuse_a_selection() {
        let (mut state, request) = fixture();
        assert!(selected_tracks(&state, &request, 5).is_err());
        state["catalog"]["busy"] = json!(true);
        assert!(selected_tracks(&state, &request, 4).is_err());
        state["catalog"]["busy"] = json!(false);
        state["catalog"]["revision"] = json!("9007199254740994");
        assert!(selected_tracks(&state, &request, 4).is_err());
        state["catalog"]["revision"] = request["catalog_revision"].clone();
        state["sign_in"]["connected"] = json!(false);
        assert!(selected_tracks(&state, &request, 4).is_err());
    }
    #[test]
    fn malformed_missing_cross_source_duplicate_and_oversized_batches_are_rejected_whole() {
        let (state, request) = fixture();
        let mut cases = vec![
            json!([]),
            json!([null]),
            json!([request["tracks"][0], request["tracks"][0]]),
            json!(vec![request["tracks"][0].clone(); 101]),
        ];
        let mut missing = request["tracks"].clone();
        missing[0]["key"] = json!("missing");
        cases.push(missing);
        let mut other = request["tracks"].clone();
        other[0]["source"] = json!(SourceId::new());
        cases.push(other);
        for tracks in cases {
            let mut bad = request.clone();
            bad["tracks"] = tracks;
            assert!(selected_tracks(&state, &bad, 4).is_err());
        }
    }
}
