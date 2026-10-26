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
    SignalSpy { id: outputs; target: settings; signalName: "outputRequested" }
    SignalSpy { id: modes; target: settings; signalName: "modeRequested" }
    function init() {
        settings.connected = true;
        settings.output = null;
        settings.output = {selection: {mode: "desktop"}, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}], message: "Apply output pauses"};
        settings.snapshot = {mode: "auto", available: true, message: "Upcoming preparations only", metadata_scope: "Embedded FLAC tags"};
        modes.clear(); outputs.clear(); waitForPolish(settings);
    }
    function test_output_requires_device_selection_and_explicit_apply_then_preserves_draft_across_status_updates() {
        const apply = findChild(settings, "applyOutput");
        verify(!apply.enabled);
        settings.outputModeDraft = 1; verify(!apply.enabled); compare(outputs.count, 0);
        settings.outputDeviceDraft = "fixture"; verify(apply.enabled); compare(outputs.count, 0);
        settings.output = {selection: {mode: "desktop"}, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}], message: "Refreshed status"};
        compare(settings.outputModeDraft, 1); compare(settings.outputDeviceDraft, "fixture");
        apply.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(outputs.count, 1); compare(outputs.signalArguments[0][0], "fidelity"); compare(outputs.signalArguments[0][1], "fixture");
        settings.output = {selection: {mode: "fidelity", device: "fixture"}, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}], message: "Paused; Resume explicitly"};
        verify(!apply.enabled);
    }
    function test_missing_device_and_disconnected_output_cannot_be_applied() {
        settings.outputModeDraft = 1; settings.outputDeviceDraft = "not-discovered";
        verify(!findChild(settings, "applyOutput").enabled);
        settings.outputDeviceDraft = "fixture"; settings.connected = false;
        verify(!findChild(settings, "applyOutput").enabled); compare(outputs.count, 0);
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
