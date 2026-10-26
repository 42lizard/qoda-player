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
    function test_pointer_output_selection_and_apply() {
        const mode = findChild(settings, "outputMode");
        mouseClick(mode, mode.width / 2, mode.height / 2);
        tryCompare(mode.popup, "opened", true);
        const fidelity = mode.popup.contentItem.itemAtIndex(1);
        verify(!!fidelity);
        mouseClick(fidelity, fidelity.width / 2, fidelity.height / 2);
        tryCompare(settings, "outputModeDraft", 1);
        tryCompare(mode.popup, "visible", false);
        waitForPolish(settings);
        const device = findChild(settings, "outputDevice");
        verify(waitForRendering(settings));
        mouseClick(device, device.width / 2, device.height / 2);
        tryCompare(device.popup, "opened", true);
        const pcm = device.popup.contentItem.itemAtIndex(0);
        verify(!!pcm);
        mouseClick(pcm, pcm.width / 2, pcm.height / 2);
        tryCompare(settings, "outputDeviceDraft", "fixture");
        const apply = findChild(settings, "applyOutput");
        verify(apply.enabled);
        mouseClick(apply, apply.width / 2, apply.height / 2);
        compare(outputs.count, 1);
        compare(outputs.signalArguments[0][0], "fidelity");
        compare(outputs.signalArguments[0][1], "fixture");
    }
    function test_missing_device_and_disconnected_output_cannot_be_applied() {
        settings.outputModeDraft = 1; settings.outputDeviceDraft = "not-discovered";
        verify(!findChild(settings, "applyOutput").enabled);
        settings.outputDeviceDraft = "fixture"; settings.connected = false;
        verify(!findChild(settings, "applyOutput").enabled); compare(outputs.count, 0);
    }
    function test_hq_conversion_is_an_explicit_persisted_output_choice() {
        settings.output = {selection: {mode: "fidelity", device: "fixture"}, ready: true, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}]};
        verify(!settings.allowConversionDraft); verify(!settings.canApplyOutput);
        const toggle = findChild(settings, "allowHqConversion");
        toggle.forceActiveFocus(); keyClick(Qt.Key_Space);
        verify(settings.allowConversionDraft); verify(settings.canApplyOutput);
        verify(!settings.assumeStereoDraft);
        const stereo = findChild(settings, "assumeStereo");
        stereo.forceActiveFocus(); keyClick(Qt.Key_Space); verify(settings.assumeStereoDraft);
        const apply = findChild(settings, "applyOutput");
        apply.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(outputs.count, 1); compare(outputs.signalArguments[0][0], "fidelity_resampled"); compare(outputs.signalArguments[0][1], "fixture"); compare(outputs.signalArguments[0][2], true);
        settings.output = {selection: {mode: "fidelity_resampled", device: "fixture", assume_stereo: true}, ready: true, available: true, fidelity_available: true, devices: [{id: "fixture", name: "Fictional direct PCM"}], processing: {source: {rate: 44100, bits: 24}, target: {rate: 48000, bits: 16}}};
        compare(settings.outputModeDraft, 1); verify(settings.allowConversionDraft); verify(!settings.canApplyOutput);
        verify(findChild(settings, "outputProcessing").text.includes("SoXR VHQ"));
        verify(findChild(settings, "outputProcessing").text.includes("TPDF dither"));
        toggle.forceActiveFocus(); keyClick(Qt.Key_Space);
        verify(!settings.allowConversionDraft); compare(settings.requestedOutputMode, "fidelity"); verify(settings.canApplyOutput);
    }
    function test_continuity_and_independent_fallback_are_explicit() {
        settings.output = {available: true, selection: {mode: "fidelity_resampled"}, continuous: true};
        const label = findChild(settings, "outputContinuity");
        verify(label.visible); verify(label.text.includes("Continuous conversion"));
        settings.output = {available: true, selection: {mode: "fidelity_resampled"}, continuous: false, chain_fallback: "Independent conversion: short source"};
        verify(label.visible); compare(label.text, "Independent conversion: short source");
        settings.output = {available: true, selection: {mode: "desktop"}};
        verify(!label.visible);
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
