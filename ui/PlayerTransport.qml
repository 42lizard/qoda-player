import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: transport
    required property var snapshot
    required property var selectedTrack
    required property bool connected
    readonly property var presentation: snapshot.presentation || ({})
    readonly property bool known: connected && !!presentation.known
    readonly property var track: known ? presentation.track : selectedTrack
    readonly property real duration: known && track && track.duration_seconds > 0 ? track.duration_seconds : selectedTrack && selectedTrack.duration_ms > 0 ? selectedTrack.duration_ms / 1000 : 0
    readonly property real position: known && presentation.position_seconds !== null ? presentation.position_seconds : snapshot.queue ? snapshot.queue.position_seconds || 0 : 0
    readonly property var gain: snapshot.replaygain && snapshot.replaygain.applied ? snapshot.replaygain.applied : null
    OutputPresentation { id: outputView; output: transport.snapshot.output || null }
    readonly property string outputText: outputView.text
    readonly property string gainText: gain ? (known ? "" : "Prepared · ") + (gain.basis === "off" ? "ReplayGain off" : "ReplayGain " + gain.basis + " · " + gain.db.toFixed(2) + " dB") : "ReplayGain " + (snapshot.replaygain ? snapshot.replaygain.mode : "off") + " · awaiting preparation"
    signal transportRequested(string op)
    signal queueRequested(string op)
    signal seekRequested(real seconds)
    color: "#1c251e"
    radius: 8
    implicitHeight: body.implicitHeight + 24

    function timestamp(seconds) {
        if (!Number.isFinite(seconds) || seconds < 0) return "—";
        return Math.floor(seconds / 60) + ":" + String(Math.floor(seconds % 60)).padStart(2, "0");
    }

    GridLayout {
        id: body
        anchors.fill: parent; anchors.margins: 12
        columns: transport.width >= 800 ? 3 : 1
        columnSpacing: 24; rowSpacing: 8
        ColumnLayout {
            Layout.fillWidth: true
            Layout.preferredWidth: 230
            spacing: 3
            Label { text: transport.track ? transport.track.title : "Choose something to play"; color: "#e8eee8"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: transport.track ? transport.track.artist + (transport.track.album ? " · " + transport.track.album : "") : "Browse your Plex music library"; color: "#a4b0a7"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { visible: transport.width < 800; text: transport.gainText; color: "#a4b0a7"; font.pixelSize: 11; Layout.fillWidth: true; elide: Text.ElideRight; ToolTip.visible: smallGainHover.hovered; ToolTip.text: transport.gain ? transport.gain.reason : "Effective gain is available after preparation"; HoverHandler { id: smallGainHover } }
            Label { visible: transport.width < 800 && outputView.fidelity; text: transport.outputText; color: "#a4b0a7"; font.pixelSize: 11; Layout.fillWidth: true; wrapMode: Text.Wrap }
            Label { visible: !!transport.track && !transport.known; text: "Selected · playback unconfirmed"; color: "#a4b0a7"; font.pixelSize: 11 }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.preferredWidth: 380
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Button { objectName: "previous"; text: "Previous"; enabled: transport.connected && !!transport.presentation.can_previous; onClicked: transport.queueRequested("previous") }
                Button { objectName: "playPause"; text: transport.snapshot.intent === "playing" ? "Pause" : "Play"; highlighted: true; enabled: transport.connected && (transport.snapshot.intent === "playing" ? !!transport.presentation.can_pause : !!transport.presentation.can_play); onClicked: transport.transportRequested(transport.snapshot.intent === "playing" ? "pause" : "resume") }
                Button { objectName: "next"; text: "Next"; enabled: transport.connected && !!transport.presentation.can_next; onClicked: transport.queueRequested("next") }
                ToolButton { objectName: "replay"; text: "↺"; Accessible.name: "Replay selection"; ToolTip.visible: hovered; ToolTip.text: "Replay selection"; enabled: transport.connected && !!transport.snapshot.queue && transport.snapshot.queue.membership_count > 0; onClicked: transport.queueRequested("replay") }
            }
            RowLayout {
                Layout.fillWidth: true
                Label { text: transport.timestamp(transport.position); color: "#a4b0a7"; font.pixelSize: 11 }
                Slider {
                    id: progress
                    objectName: "seekPosition"
                    Accessible.name: "Playback position"
                    Layout.fillWidth: true
                    from: 0; to: Math.max(1, transport.duration)
                    enabled: transport.connected && !!transport.presentation.can_seek && transport.duration > 0
                    Binding on value { when: !progress.pressed; value: Math.min(transport.position, progress.to); restoreMode: Binding.RestoreNone }
                    onMoved: if (!pressed) transport.seekRequested(value)
                    onPressedChanged: if (!pressed && enabled) transport.seekRequested(value)
                }
                Label { text: transport.duration > 0 ? transport.timestamp(transport.duration) : "—"; color: "#a4b0a7"; font.pixelSize: 11 }
            }
        }
        ColumnLayout {
            visible: transport.width >= 800
            Layout.fillWidth: true
            Layout.preferredWidth: 210
            Label { text: transport.outputText; color: "#e8eee8"; Layout.alignment: Qt.AlignRight; Layout.fillWidth: true; elide: Text.ElideRight; ToolTip.visible: outputHover.hovered; ToolTip.text: transport.outputText + " · Hardware qualification pending"; HoverHandler { id: outputHover } }
            Label { text: transport.gainText; color: "#a4b0a7"; Layout.alignment: Qt.AlignRight; ToolTip.visible: gainHover.hovered; ToolTip.text: transport.gain ? transport.gain.reason : "Effective gain is available after preparation"; HoverHandler { id: gainHover } }
        }
    }
}
