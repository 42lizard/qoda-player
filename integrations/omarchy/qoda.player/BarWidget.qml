import QtQuick
import Quickshell
import Quickshell.Io
import qs.Ui
import qs.Commons as Commons
import "qoda-ui" as Qoda

BarWidget {
    id: root
    moduleName: "qoda.player"
    implicitWidth: barSize
    implicitHeight: barSize
    activeFocusOnTab: true
    property bool popupOpen: false
    readonly property bool opened: popupOpen
    property bool openingLibrary: false
    readonly property string playerPath: String(setting("playerPath", ""))
    Accessible.name: "Qoda Player"
    Accessible.role: Accessible.Button
    Accessible.description: "Open compact playback controls"
    function close() { popupOpen = false; }
    function open() { popupOpen = true; Qt.callLater(controls.focusPrimary); }
    function toggle() { if (popupOpen) close(); else open(); }
    property string libraryAction: "showFull"
    property int openAttempts: 0
    property bool launchStarted: false
    property string launchError: ""
    function openLibrary(action) {
        if (!playerPath || openingLibrary) return;
        libraryAction = ["showFull", "showSearch", "showQueue"].includes(action) ? action : "showFull";
        openingLibrary = true; launchStarted = false; openAttempts = 0; launchError = "";
        tryOpen();
    }
    function tryOpen() {
        if (!openingLibrary) return;
        openAttempts++;
        opener.command = ["quickshell", "ipc", "--path", playerPath + "/ui", "call", "--", "control", libraryAction];
        opener.running = true;
        openDeadline.restart();
    }
    Binding {
        target: Qoda.Theme; property: "colors"
        value: ({background: Commons.Color.background, foreground: Commons.Color.foreground, accent: Commons.Color.accent, urgent: Commons.Color.urgent})
    }
    Qoda.WidgetClient { id: client }
    Item {
        anchors.centerIn: parent
        width: 22; height: 22
        opacity: client.connected ? 1 : 0.55
        Rectangle { x: 3; y: 2; width: 15; height: 15; radius: 8; color: "transparent"; border.width: 2; border.color: root.bar ? root.bar.barForeground : Commons.Color.foreground }
        Rectangle { x: 14; y: 16; width: 8; height: 2; rotation: 45; color: root.bar ? root.bar.barForeground : Commons.Color.foreground }
        Rectangle { x: 0; y: 18; width: 4; height: 4; radius: 2; color: Commons.Color.accent; visible: client.connected && client.playing }
    }
    WidgetButton {
        id: iconButton
        objectName: "qodaWidgetButton"
        anchors.fill: parent
        bar: root.bar
        hasVisualContent: true
        labelVisible: false
        tooltipText: "Qoda Player · click for controls"
        onPressed: button => { if (button === Qt.LeftButton) root.toggle(); }
    }
    Keys.onReturnPressed: root.toggle()
    Keys.onSpacePressed: event => { root.toggle(); event.accepted = true; }
    Process {
        id: opener
        stdout: StdioCollector {}
        stderr: StdioCollector {}
        onExited: (code, status) => {
            openDeadline.stop();
            if (code === 0) { root.openingLibrary = false; root.close(); }
            else {
                if (!root.launchStarted) {
                    root.launchStarted = true;
                    Quickshell.execDetached(["python3", root.playerPath + "/tools/dev.py"]);
                }
                if (root.openAttempts < 120) openRetry.restart();
                else { root.openingLibrary = false; root.launchError = "Could not open Qoda. Try launching the player again."; }
            }
        }
    }
    Timer { id: openDeadline; interval: 3000; onTriggered: if (opener.running) opener.signal(9) }
    Timer { id: openRetry; interval: 500; onTriggered: root.tryOpen() }
    PopupCard {
        id: popup
        anchorItem: root; bar: root.bar; owner: root
        open: root.popupOpen
        contentWidth: fittedContentWidth(440)
        contentHeight: fittedContentHeight(controls.implicitHeight, 390)
        Qoda.CompactControls {
            id: controls
            anchors.fill: parent
            snapshot: client.snapshot
            selectedTrack: client.selectedTrack
            connected: client.connected
            active: root.popupOpen
            notice: root.launchError || (root.openingLibrary ? "Opening Qoda…" : "")
            font.family: root.bar ? root.bar.fontFamily : Commons.Style.font.family
            onExpandRequested: root.openLibrary("showFull")
            onSearchRequested: root.openLibrary("showSearch")
            onHideRequested: root.close()
            onQuitRequested: { client.send("quit"); root.close(); }
            onTransportRequested: op => client.send(op)
            onQueueRequested: op => { if (op === "show") root.openLibrary("showQueue"); else client.send(op); }
        }
    }
}
