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
    readonly property var presentation: snapshot.presentation || ({})
    readonly property bool known: connected && !!presentation.known
    readonly property var track: known ? presentation.track : selectedTrack
    readonly property var gain: snapshot.replaygain ? snapshot.replaygain.applied : null
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
    Timer { id: focusPrime; interval: 90; onTriggered: if (panel.visible && panel.interactive) { panel.focusPrimed = true; (play.enabled ? play : expand).forceActiveFocus(); } }

    function timestamp(seconds) {
        if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "—";
        return Math.floor(seconds / 60) + ":" + String(Math.floor(seconds % 60)).padStart(2, "0");
    }
    ThemedControl {
        anchors.fill: parent
        focus: true
        Keys.priority: Keys.AfterItem
        Keys.onPressed: event => {
            if (event.key === Qt.Key_Escape) { panel.hideRequested(); event.accepted = true; }
            else if (event.key === Qt.Key_Space && event.modifiers === Qt.NoModifier && play.enabled) {
                panel.transportRequested(panel.snapshot.intent === "playing" ? "pause" : "resume"); event.accepted = true;
            }
        }
        Shortcut { sequence: "Ctrl+K"; enabled: panel.visible; onActivated: panel.searchRequested() }
        TapHandler { onPressedChanged: if (pressed && !panel.interactive) { panel.interactive = true; panel.focusPrimed = true; } }
        ScrollView {
            anchors.fill: parent
            anchors.margins: 14
            contentWidth: availableWidth
            ColumnLayout {
                width: parent.width
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    Label { text: "Qoda"; color: Theme.foreground; font.bold: true; font.pixelSize: 20; Layout.fillWidth: true }
                    Button { id: expand; text: "Expand"; onClicked: panel.expandRequested() }
                    Button { text: "Hide"; onClicked: panel.hideRequested() }
                    Button { text: "Quit"; onClicked: panel.quitRequested() }
                }
                Label { text: panel.known ? panel.presentation.status : panel.track ? "Selected · playback unconfirmed" : "No track selected"; color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
                Label { text: panel.track ? panel.track.title : "Choose music in the browser"; color: Theme.foreground; font.pixelSize: 22; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: panel.track ? panel.track.artist + (panel.track.album ? " · " + panel.track.album : "") : ""; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                RowLayout {
                    Layout.fillWidth: true
                    Label { text: panel.timestamp(panel.known ? panel.presentation.position_seconds : null); color: Theme.muted }
                    ProgressBar { from: 0; to: panel.known && panel.track.duration_seconds > 0 ? panel.track.duration_seconds : 1; value: panel.known ? panel.presentation.position_seconds || 0 : 0; Layout.fillWidth: true }
                    Label { text: panel.timestamp(panel.known ? panel.track.duration_seconds : null); color: Theme.muted }
                }
                OutputPresentation { id: compactOutput; output: panel.snapshot.output || null }
                Label { visible: compactOutput.fidelity; text: compactOutput.text; color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
                Label { visible: !!panel.gain; text: panel.gain ? (panel.known ? "" : "Prepared · ") + "ReplayGain " + panel.gain.basis + " · " + panel.gain.db.toFixed(2) + " dB" : ""; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                RowLayout {
                    Button { text: "Previous"; enabled: panel.connected && !!panel.presentation.can_previous; onClicked: panel.queueRequested("previous") }
                    Button { id: play; text: panel.snapshot.intent === "playing" ? "Pause" : "Resume"; enabled: panel.connected && (panel.snapshot.intent === "playing" ? !!panel.presentation.can_pause : !!panel.presentation.can_play); onClicked: panel.transportRequested(panel.snapshot.intent === "playing" ? "pause" : "resume") }
                    Button { text: "Next"; enabled: panel.connected && !!panel.presentation.can_next; onClicked: panel.queueRequested("next") }
                    Button { text: "Queue"; onClicked: { panel.expandRequested(); panel.queueRequested("show"); } }
                }
                Label { text: panel.connected ? panel.snapshot.status || "" : "Backend disconnected — reconnecting"; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { visible: !!panel.snapshot.error; text: panel.snapshot.error || ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
                Label { visible: !!panel.snapshot.gapless && panel.snapshot.gapless.phase === "review"; text: panel.snapshot.gapless ? panel.snapshot.gapless.message : ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
                Label { visible: !!panel.snapshot.storage && ["unsaved", "blocked"].includes(panel.snapshot.storage.state); text: panel.snapshot.storage ? panel.snapshot.storage.message : ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            }
        }
    }
}
