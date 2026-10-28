import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "TrackRowInput"
    width: 1000
    height: 160
    visible: true
    when: windowShown
    Qoda.TrackSelection { id: selection }
    Qoda.TrackRow {
        id: row
        width: test.width
        index: 1
        track: ({title: "Synthetic title", artist: "Fixture", album: "Fixture album", reference: {source: "fixture", key: "B"}})
        selected: selection.contains(index)
        onSelectionRequested: modifiers => selection.choose(index, modifiers)
        onCheckboxRequested: selection.toggle(index)
    }
    SignalSpy { id: actions; target: row; signalName: "actionRequested" }
    SignalSpy { id: downloads; target: row; signalName: "downloadRequested" }
    function init() {
        selection.tracks = ["A", "B", "C"].map(key => ({reference: {source: "fixture", key: key}}));
        selection.clear(); actions.clear(); downloads.clear(); row.enabled = true;
        test.width = 1000; row.downloadEnabled = false; row.localCopyAvailable = false;
        waitForPolish(row);
    }
    function test_title_modifiers_select_without_playing() {
        let details = findChild(row, "trackDetails");
        selection.choose(0, Qt.NoModifier);
        mouseClick(details, 8, 8, Qt.LeftButton, Qt.ControlModifier);
        compare(selection.indexes, [0, 1]);
        mouseClick(details, 8, 8, Qt.LeftButton, Qt.ControlModifier);
        compare(selection.indexes, [0]);
        selection.choose(2, Qt.NoModifier);
        mouseClick(details, 8, 8, Qt.LeftButton, Qt.ShiftModifier);
        compare(selection.indexes, [1, 2]);
        compare(actions.count, 0);
    }
    function test_checkbox_is_keyboard_usable_and_stays_bound_to_selection() {
        let checkbox = findChild(row, "trackCheckbox");
        mouseClick(checkbox);
        compare(selection.indexes, [1]);
        compare(checkbox.checked, true);
        selection.clear(); compare(checkbox.checked, false);
        checkbox.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(selection.indexes, [1]);
        compare(actions.count, 0);
    }
    function test_play_is_explicit_and_disabled_rows_do_not_select() {
        let play = findChild(row, "playNow");
        play.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(actions.count, 1);
        compare(actions.signalArguments[0][0], "play");
        compare(selection.count, 0);
        row.enabled = false;
        mouseClick(findChild(row, "trackDetails"), 8, 8);
        compare(selection.count, 0);
    }
    function test_narrow_download_actions_wrap_and_remain_keyboard_usable() {
        test.width = 300;
        row.downloadEnabled = true;
        waitForPolish(row);
        let flow = findChild(row, "trackActions");
        tryVerify(() => [...flow.children].filter(child => child.visible).every(child => child.x >= 0 && child.x + child.width <= flow.width + 1));
        tryVerify(() => flow.y + flow.height <= row.height);
        let more = findChild(row, "moreTrackActions");
        more.forceActiveFocus(); keyClick(Qt.Key_Space);
        let original = findChild(row, "downloadOriginal");
        tryCompare(original, "visible", true);
        original.forceActiveFocus(); keyClick(Qt.Key_Return);
        compare(downloads.count, 1);
        compare(actions.count, 0);
    }
    function test_local_track_without_export_or_copy_has_no_download_control() {
        compare(findChild(row, "moreTrackActions").visible, false);
    }
}
