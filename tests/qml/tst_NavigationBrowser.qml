import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "PreviousPathInspector"
    width: 340
    height: 600
    visible: true
    when: windowShown
    Qoda.NavigationBrowser {
        id: browser
        anchors.fill: parent
        snapshot: null
        connected: true
    }
    SignalSpy { id: revisits; target: browser; signalName: "revisitRequested" }
    SignalSpy { id: pages; target: browser; signalName: "pageRequested" }
    function frame() {
        return {
            session: "session-fixture", revision: "9007199254740993", offset: "50", total: "102",
            has_previous: true, has_next: true, following_latest: false,
            entries: [0, 1].map(index => ({index: String(50 + index), occurrence: "duplicate-occurrence", selected: index === 1, track: {title: "Fictional repeated track", artist: "Fixture", album: "Fixture album"}}))
        };
    }
    function init() {
        browser.connected = true;
        browser.snapshot = frame();
        pages.clear(); revisits.clear();
        waitForPolish(browser);
        tryVerify(() => !!findChild(browser, "revisitNavigation"));
    }
    function test_revisit_keeps_exact_index_and_lossless_page_fences() {
        let button = findChild(browser, "revisitNavigation");
        button.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(revisits.count, 1);
        const fields = revisits.signalArguments[0][0];
        compare(fields.index, "50");
        compare(fields.occurrence, "duplicate-occurrence");
        compare(fields.queue_session, "session-fixture");
        compare(fields.queue_revision, "9007199254740993");
        browser.revisit(browser.rows[1]);
        compare(revisits.signalArguments[1][0].index, "51");
        compare(pages.count, 0);
    }
    function test_paging_never_selects_or_plays_an_entry() {
        for (let name of ["olderNavigation", "newerNavigation", "latestNavigation"]) mouseClick(findChild(browser, name));
        compare(pages.count, 3);
        compare(pages.signalArguments[0][0], "0");
        compare(pages.signalArguments[1][0], "100");
        compare(pages.signalArguments[2][0], null);
        compare(pages.signalArguments[0][1].queue_revision, "9007199254740993");
        compare(revisits.count, 0);
    }
    function test_disconnected_and_replaced_views_cannot_send_old_rows() {
        const old = browser.rows[0];
        browser.connected = false;
        verify(!findChild(browser, "revisitNavigation").enabled);
        verify(!findChild(browser, "olderNavigation").enabled);
        browser.revisit(old); compare(revisits.count, 0);
        browser.connected = true;
        browser.snapshot = {session: "new-session", revision: "0", offset: "0", total: "0", entries: [], has_previous: false, has_next: false, following_latest: true};
        browser.revisit(old); compare(revisits.count, 0);
        compare(browser.rows.length, 0);
    }
    function test_metadata_ticks_keep_the_list_model_until_rows_change() {
        const rows = browser.rows;
        const next = frame(); next.revision = "9007199254740994";
        browser.snapshot = next;
        verify(browser.rows === rows);
        const changed = frame(); changed.entries[0].track.title = "Fresh title";
        browser.snapshot = changed;
        verify(browser.rows !== rows);
        compare(browser.rows[0].track.title, "Fresh title");
        verify(browser.viewportHeight > 100);
    }
}
