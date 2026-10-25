import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "CachedAlbumDetails"
    width: 1100
    height: 650
    visible: true
    when: windowShown
    Qoda.AlbumBrowser {
        id: browser
        anchors.fill: parent
        snapshot: ({page: null, selected: null, busy: false})
        connected: true
        artworkEnabled: false
    }
    SignalSpy { id: queues; target: browser; signalName: "queueAlbum" }
    SignalSpy { id: tracks; target: browser; signalName: "trackAction" }
    SignalSpy { id: opens; target: browser; signalName: "openAlbum" }
    function frame(origin, refreshing, revision) {
        let reference = {source: "fixture", key: "album"};
        let album = {reference: reference, title: "Fictional album", artist: "Fixture artist", artist_ref: null, year: null, track_count: "1"};
        return {
            page: {source: "fixture", offset: "0", query: "", albums: [album], has_more: false, artist: null},
            selected: {album: album, revision: revision, tracks: [{disc: 1, number: 1, track: {reference: {source: "fixture", key: "track"}, title: "Fictional track", artist: "Fixture artist", album: "Fictional album", duration_ms: null}}], artwork: null, artwork_unavailable: true},
            covers: {}, busy: false, message: "Cached details; refresh required for queue actions.",
            cache: {origin: origin, refreshing: refreshing, stale: origin !== "live", incomplete: false}
        };
    }
    function init() {
        test.width = 1100;
        browser.snapshot = frame("cached", true, "cached-revision");
        browser.showDetails = true;
        queues.clear(); tracks.clear(); opens.clear();
        waitForPolish(browser);
        tryVerify(() => !!findChild(browser, "playAlbumFromHere"));
    }
    function test_cached_details_remain_readable_and_cannot_queue_after_failed_refresh() {
        compare(browser.trackModel.length, 1);
        compare(browser.canQueue, false);
        for (let name of ["playAlbum", "queueAlbum", "playAlbumFromHere", "enqueueAlbumTrack", "nextAlbumTrack"]) {
            let button = findChild(browser, name);
            verify(button); compare(button.enabled, false);
            mouseClick(button);
        }
        browser.queue("album_play", null);
        compare(queues.count, 0); compare(tracks.count, 0);
        browser.snapshot = frame("cached", false, "cached-revision");
        compare(browser.canQueue, false);
        compare(browser.trackModel.length, 1);
        mouseClick(findChild(browser, "refreshAlbum"));
        compare(opens.count, 1);
        compare(opens.signalArguments[0][0].key, "album");
        compare(queues.count, 0); compare(tracks.count, 0);
    }
    function test_detail_actions_wrap_in_a_narrow_browser() {
        test.width = 540;
        waitForPolish(browser);
        for (let name of ["refreshAlbum", "playAlbum", "queueAlbum"]) {
            let button = findChild(browser, name);
            verify(button.x >= 0);
            verify(button.x + button.width <= button.parent.width + 1, name + " exceeds the available width");
        }
    }
    function test_background_refresh_does_not_reopen_details_after_back() {
        browser.showDetails = false;
        browser.snapshot = frame("live", false, "new-live-revision");
        compare(browser.showDetails, false);
        compare(browser.trackModel.length, 1);
        compare(browser.canQueue, true);
    }
    function test_live_refresh_enables_actions_with_current_revision() {
        browser.snapshot = frame("live", false, "new-live-revision");
        waitForPolish(browser);
        compare(browser.canQueue, true);
        mouseClick(findChild(browser, "queueAlbum"));
        compare(queues.count, 1);
        compare(queues.signalArguments[0][0], "album_enqueue");
        compare(queues.signalArguments[0][1].expected_revision, "new-live-revision");
        mouseClick(findChild(browser, "nextAlbumTrack"));
        compare(tracks.count, 1);
        compare(tracks.signalArguments[0][0], "play_next");
        browser.snapshot = frame("cached", true, "new-live-revision");
        compare(browser.canQueue, false);
        verify(!findChild(browser, "nextAlbumTrack").enabled);
    }
}
