import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "PlayerTransport"
    width: 1100; height: 230; visible: true
    when: windowShown
    Qoda.PlayerTransport {
        id: transport
        width: parent.width
        height: implicitHeight
        snapshot: ({})
        selectedTrack: null
        connected: true
    }
    SignalSpy { id: commands; target: transport; signalName: "transportRequested" }
    SignalSpy { id: seeks; target: transport; signalName: "seekRequested" }
    function init() {
        test.width = 1100;
        transport.connected = true;
        transport.selectedTrack = {title: "Fictional selection", artist: "Fixture", album: "Album", duration_ms: "240000"};
        transport.snapshot = {intent: "paused", queue: {position_seconds: 42, membership_count: 2}, presentation: {known: false, can_play: true, can_pause: false, can_seek: false}};
        commands.clear(); seeks.clear(); waitForPolish(transport);
    }
    function test_unconfirmed_selection_can_retry_but_cannot_seek() {
        verify(!transport.known);
        const play = findChild(transport, "playPause");
        verify(play.enabled);
        play.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(commands.count, 1); compare(commands.signalArguments[0][0], "resume");
        verify(!findChild(transport, "seekPosition").enabled);
        compare(seeks.count, 0);
    }
    function test_nominal_seek_uses_confirmed_duration_and_updates_with_clock() {
        transport.snapshot = {intent: "paused", queue: {}, presentation: {known: true, track: {title: "Nominal", artist: "Fixture", album: "Album", duration_seconds: 120}, position_seconds: 12, can_seek: true}};
        const slider = findChild(transport, "seekPosition");
        verify(slider.enabled); compare(slider.to, 120); compare(slider.value, 12);
        slider.forceActiveFocus(); keyClick(Qt.Key_Right);
        compare(seeks.count, 1); verify(seeks.signalArguments[0][0] > 12);
        transport.snapshot = {intent: "paused", queue: {}, presentation: {known: true, track: {title: "Nominal", artist: "Fixture", album: "Album", duration_seconds: 120}, position_seconds: 35, can_seek: true}};
        compare(slider.value, 35);
    }
    function test_gain_label_separates_settings_and_immutable_prepared_gain() {
        transport.snapshot = {intent: "paused", replaygain: {mode: "album", applied: {mode: "track", basis: "track", db: -6, reason: "Embedded FLAC tags"}}, presentation: {known: false}};
        verify(transport.gainText.includes("Prepared")); verify(transport.gainText.includes("Track")); verify(transport.gainText.includes("-6.00"));
        transport.snapshot = {intent: "paused", replaygain: {mode: "off", applied: {mode: "track", basis: "track", db: 0, reason: "Measured zero tag"}}, presentation: {known: true}};
        verify(!transport.gainText.includes("Prepared")); verify(transport.gainText.includes("Track")); verify(transport.gainText.includes("0.00"));
    }
    function test_disconnected_controls_cannot_send_commands() {
        transport.connected = false;
        for (let name of ["previous", "playPause", "next", "replay", "seekPosition"]) verify(!findChild(transport, name).enabled);
        compare(commands.count, 0); compare(seeks.count, 0);
    }
    function test_transport_controls_fit_narrow_window() {
        test.width = 480;
        waitForPolish(transport); wait(50);
        for (let name of ["previous", "playPause", "next", "replay", "seekPosition"]) {
            const item = findChild(transport, name);
            const point = item.mapToItem(transport, 0, 0);
            verify(point.x >= 0 && point.x + item.width <= transport.width, name + " exceeds transport width: " + point.x + ", " + item.width + ", " + transport.width);
            verify(point.y >= 0 && point.y + item.height <= transport.height, name + " exceeds transport height");
        }
    }
}
