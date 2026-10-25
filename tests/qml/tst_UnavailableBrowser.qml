import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "UnavailableEntries"
    width: 340; height: 650; visible: true
    when: windowShown
    Qoda.UnavailableBrowser {
        id: browser
        anchors.fill: parent
        snapshot: null
        connected: true
        canRetry: true
    }
    SignalSpy { id: actions; target: browser; signalName: "actionRequested" }
    function frame() {
        return {session: "session", revision: "9007199254740993", selected_unavailable: "missing", unavailable_entries: [{id: "one", unavailable: "missing", selected: true, track: {title: "Fictional track", artist: "Fixture"}}]};
    }
    function init() {
        browser.connected = true; browser.canRetry = true; browser.snapshot = frame();
        actions.clear(); waitForPolish(browser);
        tryVerify(() => !!findChild(browser, "retryUnavailable"));
    }
    function test_retry_and_skip_are_explicit_and_keep_lossless_fences() {
        let retry = findChild(browser, "retryUnavailable");
        retry.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 1); compare(actions.signalArguments[0][0], "retry_unavailable");
        compare(actions.signalArguments[0][1].occurrence, "one");
        compare(actions.signalArguments[0][1].queue_revision, "9007199254740993");
        mouseClick(findChild(browser, "skipUnavailable"));
        compare(actions.count, 2); compare(actions.signalArguments[1][0], "skip_unavailable");
        compare(actions.signalArguments[1][1].queue_session, "session");
    }
    function test_offline_entries_can_be_removed_without_starting_playback() {
        browser.canRetry = false;
        verify(!findChild(browser, "retryUnavailable").enabled);
        verify(!findChild(browser, "skipUnavailable").enabled);
        mouseClick(findChild(browser, "removeUnavailable"));
        compare(actions.count, 1); compare(actions.signalArguments[0][0], "queue_remove");
        browser.act("retry_unavailable",browser.rows[0]); compare(actions.count, 1);
        browser.connected = false;
        browser.act("queue_remove",browser.rows[0]); compare(actions.count, 1);
    }
    function test_replaced_views_reject_old_rows_and_hide_skip_when_no_failed_selection() {
        const row = browser.rows[0];
        browser.snapshot = {session: "new", revision: "0", selected_unavailable: null, unavailable_entries: []};
        browser.act("retry_unavailable",row); compare(actions.count, 0);
        verify(!findChild(browser, "skipUnavailable").visible);
        compare(browser.rows.length, 0);
    }
}
