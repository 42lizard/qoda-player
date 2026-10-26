import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Wayland

PanelWindow {
    id: panel
    required property var snapshot
    required property var selectedTrack
    required property bool connected
    property var targetScreen: null
    property bool interactive: false
    property bool focusPrimed: false
    readonly property int keyboardMode: WlrLayershell.keyboardFocus
    signal searchRequested()
    signal expandRequested()
    signal hideRequested()
    signal quitRequested()
    signal transportRequested(string op)
    signal queueRequested(string op)

    visible: false
    screen: Quickshell.screens.includes(targetScreen) ? targetScreen : Quickshell.screens[0] ?? null
    implicitWidth: Math.min(440, (screen ? screen.width : 464) - 24)
    implicitHeight: Math.min(340, (screen ? screen.height : 364) - 24)
    anchors { bottom: true; right: true }
    margins { bottom: 12; right: 12 }
    exclusionMode: ExclusionMode.Ignore
    color: Theme.background
    WlrLayershell.namespace: "qoda-player-compact"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: !visible || !interactive ? WlrKeyboardFocus.None : focusPrimed ? WlrKeyboardFocus.OnDemand : WlrKeyboardFocus.Exclusive
    onVisibleChanged: {
        focusPrime.stop(); focusPrimed = false;
        if (visible && interactive) focusPrime.restart();
    }
    onBackingWindowVisibleChanged: if (backingWindowVisible && interactive) focusPrime.restart()
    Timer { id: focusPrime; interval: 90; onTriggered: if (panel.visible && panel.interactive) { panel.focusPrimed = true; controls.focusPrimary(); } }

    TapHandler { onPressedChanged: if (pressed && !panel.interactive) { panel.interactive = true; panel.focusPrimed = true; } }
    CompactControls {
        id: controls
        anchors.fill: parent
        anchors.margins: 14
        snapshot: panel.snapshot
        selectedTrack: panel.selectedTrack
        connected: panel.connected
        active: panel.visible
        onSearchRequested: panel.searchRequested()
        onExpandRequested: panel.expandRequested()
        onHideRequested: panel.hideRequested()
        onQuitRequested: panel.quitRequested()
        onTransportRequested: op => panel.transportRequested(op)
        onQueueRequested: op => panel.queueRequested(op)
    }
}
