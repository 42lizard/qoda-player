import QtQuick
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "CompactControls"
    width: 440; height: 360; visible: true; when: windowShown
    Qoda.CompactControls { id: controls; anchors.fill: parent; snapshot: ({}); selectedTrack: null; connected: false }
    SignalSpy { id: transport; target: controls; signalName: "transportRequested" }
    SignalSpy { id: queue; target: controls; signalName: "queueRequested" }
    SignalSpy { id: expanded; target: controls; signalName: "expandRequested" }
    SignalSpy { id: hidden; target: controls; signalName: "hideRequested" }
    function init() { controls.snapshot = ({}); controls.selectedTrack = null; controls.connected = false; controls.active = true; transport.clear(); queue.clear(); expanded.clear(); hidden.clear(); }
    function test_opening_and_disconnected_controls_never_start_playback() {
        controls.focusPrimary();
        compare(transport.count, 0); compare(queue.count, 0);
        verify(!findChild(controls, "compactPlay").enabled);
        mouseClick(findChild(controls, "compactPlay")); compare(transport.count, 0);
        keyClick(Qt.Key_Escape); compare(hidden.count, 1);
    }
    function test_shared_transport_and_queue_controls_emit_only_explicit_actions() {
        controls.connected = true;
        controls.snapshot = ({intent: "paused", presentation: {known: false, can_play: true, can_next: true, can_previous: true}});
        mouseClick(findChild(controls, "compactPlay")); compare(transport.count, 1); compare(transport.signalArguments[0][0], "resume");
        mouseClick(findChild(controls, "compactNext")); compare(queue.signalArguments[0][0], "next");
        mouseClick(findChild(controls, "compactQueue")); compare(queue.signalArguments[1][0], "show"); compare(expanded.count, 0);
        mouseClick(findChild(controls, "compactExpand")); compare(expanded.count, 1);
        controls.snapshot = ({intent: "playing", presentation: {known: true, status: "Playing", can_pause: true, track: {title: "Fictional", artist: "Fixture"}}});
        mouseClick(findChild(controls, "compactPlay")); compare(transport.signalArguments[1][0], "pause");
    }
}
