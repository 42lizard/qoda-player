import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "DesktopControlsRecovery"
    width: 500; height: 200; visible: true
    when: windowShown
    Qoda.DesktopControls {
        id: controls
        anchors.fill: parent
        snapshot: null
        connected: true
    }
    SignalSpy { id: retries; target: controls; signalName: "retryRequested" }
    function init() {
        controls.connected = true;
        controls.snapshot = {state: "unavailable", message: "Fictional disconnected bus"};
        retries.clear(); waitForPolish(controls);
    }
    function test_retry_is_keyboard_accessible_when_unavailable() {
        const button = findChild(controls, "retryDesktopControls");
        verify(controls.visible); verify(button.enabled);
        button.forceActiveFocus(); keyClick(Qt.Key_Space);
        compare(retries.count, 1);
    }
    function test_connecting_and_disconnected_controls_cannot_retry() {
        const button = findChild(controls, "retryDesktopControls");
        controls.snapshot = {state: "connecting", message: "Connecting…"};
        verify(controls.visible); verify(!button.enabled); mouseClick(button);
        controls.snapshot = {state: "unavailable", message: "Fictional unavailable bus"};
        controls.connected = false; verify(!button.enabled); mouseClick(button);
        compare(retries.count, 0);
    }
    function test_available_and_initial_views_hide_recovery_controls() {
        controls.snapshot = {state: "available", message: "Available"};
        verify(!controls.visible);
        controls.snapshot = null;
        verify(!controls.visible);
        verify(!findChild(controls, "retryDesktopControls").enabled);
        compare(retries.count, 0);
    }
}
