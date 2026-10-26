import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "OutputDrawer"
    width: 540; height: 760; visible: true; when: windowShown
    Qoda.SettingsSheet {
        id: sheet
        width: 516; height: 760
        section: "audio"
        Qoda.AudioSettings {
            id: settings
            width: parent.width
            snapshot: ({mode: "auto", available: true, message: "", metadata_scope: ""})
            connected: true
            output: ({selection: {mode: "desktop"}, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}], message: "Apply output pauses"})
        }
    }
    function init() { sheet.open(); tryCompare(sheet, "opened", true); }
    function cleanup() { sheet.close(); tryCompare(sheet, "visible", false); }
    function test_mode_dropdown_opens_in_modal_settings() {
        const mode = findChild(settings, "outputMode");
        verify(mode.enabled); verify(mode.visible);
        mouseClick(mode, mode.width / 2, mode.height / 2);
        tryCompare(mode.popup, "opened", true);
        const fidelity = mode.popup.contentItem.itemAtIndex(1);
        verify(!!fidelity);
        mouseClick(fidelity, fidelity.width / 2, fidelity.height / 2);
        tryCompare(settings, "outputModeDraft", 1);
    }
}
