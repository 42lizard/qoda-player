import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "SettingsSheet"
    width: 540; height: 480; visible: true; when: windowShown
    Qoda.SettingsSheet {
        id: sheet
        width: 500; height: 460
        onCloseRequested: close()
        TextField { id: draft; width: parent.width; text: "Retained draft" }
        Rectangle { width: parent.width; implicitHeight: 900; color: "#191f1c" }
        Button { text: "Last setting" }
    }
    SignalSpy { id: searches; target: sheet; signalName: "searchRequested" }
    SignalSpy { id: changes; target: sheet; signalName: "sectionRequested" }
    function init() { changes.clear(); searches.clear(); sheet.open(); tryCompare(sheet, "opened", true); }
    function cleanup() { sheet.close(); tryCompare(sheet, "visible", false); }
    function test_search_shortcut_works_from_modal_field() {
        draft.forceActiveFocus(); keyClick(Qt.Key_K, Qt.ControlModifier);
        compare(searches.count, 1);
    }
    function test_long_body_scrolls_without_expanding_popup() {
        tryCompare(sheet, "scrollable", true);
        compare(sheet.height, 460);
        compare(draft.text, "Retained draft");
    }
    function test_escape_dismisses_and_reopen_keeps_draft() {
        draft.text = "Unsaved local draft"; draft.forceActiveFocus();
        keyClick(Qt.Key_Escape); tryCompare(sheet, "visible", false);
        verify(test.visible);
        sheet.open(); tryCompare(sheet, "opened", true);
        compare(draft.text, "Unsaved local draft");
        draft.text = "Retained draft";
    }
}
