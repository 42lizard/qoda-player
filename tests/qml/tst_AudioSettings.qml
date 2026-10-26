import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "ReplayGainSettings"
    width: 600; height: 200; visible: true
    when: windowShown
    Qoda.AudioSettings {
        id: settings
        anchors.fill: parent
        snapshot: null
        connected: true
    }
    SignalSpy { id: modes; target: settings; signalName: "modeRequested" }
    function init() {
        settings.connected = true;
        settings.snapshot = {mode: "auto", available: true, message: "Upcoming preparations only", metadata_scope: "Embedded FLAC tags"};
        modes.clear(); waitForPolish(settings);
    }
    function test_mode_is_keyboard_accessible_without_implicit_playback() {
        const combo = findChild(settings, "replaygainMode");
        compare(combo.currentIndex, 3);
        combo.forceActiveFocus(); keyClick(Qt.Key_Home);
        compare(modes.count, 1); compare(modes.signalArguments[0][0], "off");
        settings.snapshot = {mode: "album", available: true, message: "Saved", metadata_scope: "Embedded FLAC tags"};
        compare(combo.currentIndex, 2);
    }
    function test_disconnected_or_invalid_settings_cannot_be_changed() {
        const combo = findChild(settings, "replaygainMode");
        settings.connected = false; verify(!combo.enabled);
        settings.connected = true;
        settings.snapshot = {mode: "off", available: false, message: "Newer settings preserved", metadata_scope: ""};
        verify(!combo.enabled); compare(modes.count, 0);
    }
}
