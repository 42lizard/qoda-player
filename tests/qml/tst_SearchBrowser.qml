import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "CrossLibrarySearch"
    width: 380; height: 520; visible: true; when: windowShown
    Qoda.SearchBrowser {
        id: browser
        anchors.fill: parent
        snapshot: null; query: "fixture"; connected: true
    }
    SignalSpy { id: actions; target: browser; signalName: "actionRequested" }
    function fixture() {
        return {revision: "9007199254740993", query: "fixture", busy: false, message: "2 matches · 2 libraries", sources: [], results: [
            {track: {reference: {source: "source-A", key: "42"}, title: "Same title", artist: "Fixture", album: "Album"}, library: "Music", server: "Server A", actionable: true},
            {track: {reference: {source: "source-B", key: "42"}, title: "Same title", artist: "Fixture", album: "Album"}, library: "Music", server: "Server B", actionable: true}]};
    }
    function init() {
        browser.query = "fixture"; browser.connected = true;
        browser.libraries = []; browser.snapshot = fixture(); actions.clear(); waitForPolish(browser);
    }
    function test_duplicate_keys_are_source_distinct_and_keyboard_actions_keep_lossless_search_revision() {
        compare(browser.rows.length, 2);
        const list = findChild(browser, "searchResults"); list.forceLayout();
        const button = findChild(list.itemAtIndex(1), "playSearchResult");
        button.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 1); compare(actions.signalArguments[0][0], "search_play");
        compare(actions.signalArguments[0][1].source, "source-B"); compare(actions.signalArguments[0][1].key, "42");
        compare(actions.signalArguments[0][1].search_revision, "9007199254740993");
    }
    function test_query_edits_disconnect_and_stale_cached_rows_disable_actions_without_search_playback() {
        const row = browser.rows[0];
        browser.query = "different"; browser.act("search_play", row); compare(actions.count, 0);
        browser.query = "fixture"; browser.connected = false;
        browser.act("search_next", row); compare(actions.count, 0);
        browser.connected = true;
        const value = fixture(); value.results[0].actionable = false; browser.snapshot = value;
        browser.act("search_enqueue", browser.rows[0]); compare(actions.count, 0);
    }
    function test_download_actions_fit_narrow_results_and_keep_source_fences() {
        browser.libraries = [{id: "source-A", downloads: true}, {id: "source-B", kind: "local", downloads: false}];
        const list = findChild(browser, "searchResults"); list.forceLayout();
        const flow = findChild(list.itemAtIndex(0), "searchResultActions");
        tryVerify(() => [...flow.children].filter(child => child.visible).every(child => child.x >= 0 && child.x + child.width <= flow.width + 1));
        const button = findChild(list.itemAtIndex(0), "downloadSearchResult");
        button.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 1); compare(actions.signalArguments[0][0], "download_start");
        compare(actions.signalArguments[0][1].source, "source-A");
        compare(actions.signalArguments[0][1].search_revision, "9007199254740993");
        compare(findChild(list.itemAtIndex(1), "downloadSearchResult").visible, false);
    }
    function test_status_only_updates_preserve_model_and_scroll_selection() {
        const list = findChild(browser, "searchResults"); list.currentIndex = 1;
        const rows = browser.rows;
        const value = fixture(); value.message = "Another library timed out · partial results";
        value.sources = [{library: "Music", server: "Server C", phase: "timeout", count: 0, has_more: false, cache: {stale: true}, error: "Fixture timeout"}];
        browser.snapshot = value;
        verify(browser.rows === rows); compare(list.currentIndex, 1); compare(browser.sources.length, 1);
        compare(actions.count, 0);
    }
}
