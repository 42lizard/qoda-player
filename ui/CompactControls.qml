import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ThemedControl {
    id: controls
    required property var snapshot
    required property var selectedTrack
    required property bool connected
    property bool active: true
    property string notice: ""
    readonly property var presentation: snapshot.presentation || ({})
    readonly property bool known: connected && !!presentation.known
    readonly property var track: known ? presentation.track : selectedTrack
    readonly property var gain: snapshot.replaygain ? snapshot.replaygain.applied : null
    signal searchRequested()
    signal expandRequested()
    signal hideRequested()
    signal quitRequested()
    signal transportRequested(string op)
    signal queueRequested(string op)
    function focusPrimary() { (play.enabled ? play : expand).forceActiveFocus(); }
    function timestamp(seconds) {
        if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return "—";
        return Math.floor(seconds / 60) + ":" + String(Math.floor(seconds % 60)).padStart(2, "0");
    }

    implicitWidth: 412
    implicitHeight: body.implicitHeight
    focus: true
    Keys.priority: Keys.AfterItem
    Keys.onPressed: event => {
        if (event.key === Qt.Key_Escape) { controls.hideRequested(); event.accepted = true; }
        else if (event.key === Qt.Key_Space && event.modifiers === Qt.NoModifier && play.enabled) {
            controls.transportRequested(controls.snapshot.intent === "playing" ? "pause" : "resume"); event.accepted = true;
        }
    }
    Shortcut { sequence: "Ctrl+K"; enabled: controls.active; onActivated: controls.searchRequested() }
    ScrollView {
        anchors.fill: parent
        anchors.margins: 0
        contentWidth: availableWidth
        ColumnLayout {
            id: body
            width: parent.width
            spacing: 8
            RowLayout {
                Layout.fillWidth: true
                Label { text: "Qoda"; color: Theme.foreground; font.bold: true; font.pixelSize: 20; Layout.fillWidth: true }
                Button { id: expand; objectName: "compactExpand"; text: I18n.t("Expand"); onClicked: controls.expandRequested() }
                Button { text: I18n.t("Hide"); onClicked: controls.hideRequested() }
                Button { text: I18n.t("Quit"); onClicked: controls.quitRequested() }
            }
            Label { text: controls.known ? I18n.message(controls.presentation.status) : controls.track ? I18n.t("Selected · playback unconfirmed") : I18n.t("No track selected"); color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { text: controls.track ? controls.track.title : I18n.t("Choose music in the browser"); color: Theme.foreground; font.pixelSize: 22; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: controls.track ? controls.track.artist + (controls.track.album ? " · " + controls.track.album : "") : ""; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            RowLayout {
                Layout.fillWidth: true
                Label { text: controls.timestamp(controls.known ? controls.presentation.position_seconds : null); color: Theme.muted }
                ProgressBar { from: 0; to: controls.known && !!controls.track && controls.track.duration_seconds > 0 ? controls.track.duration_seconds : 1; value: controls.known ? controls.presentation.position_seconds || 0 : 0; Layout.fillWidth: true }
                Label { text: controls.timestamp(controls.known && controls.track ? controls.track.duration_seconds : null); color: Theme.muted }
            }
            OutputPresentation { id: compactOutput; output: controls.snapshot.output || null }
            Label { visible: compactOutput.fidelity; text: compactOutput.text; color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!controls.gain; text: controls.gain ? (controls.known ? I18n.t("ReplayGain %1 · %2 dB", [I18n.state(controls.gain.basis), I18n.number(controls.gain.db, 2)]) : I18n.t("Prepared · %1", [I18n.t("ReplayGain %1 · %2 dB", [I18n.state(controls.gain.basis), I18n.number(controls.gain.db, 2)])])) : ""; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            RowLayout {
                Button { objectName: "compactPrevious"; text: I18n.t("Previous"); enabled: controls.connected && !!controls.presentation.can_previous; onClicked: controls.queueRequested("previous") }
                Button { id: play; objectName: "compactPlay"; text: controls.snapshot.intent === "playing" ? I18n.t("Pause") : I18n.t("Resume"); enabled: controls.connected && (controls.snapshot.intent === "playing" ? !!controls.presentation.can_pause : !!controls.presentation.can_play); onClicked: controls.transportRequested(controls.snapshot.intent === "playing" ? "pause" : "resume") }
                Button { objectName: "compactNext"; text: I18n.t("Next"); enabled: controls.connected && !!controls.presentation.can_next; onClicked: controls.queueRequested("next") }
                Button { objectName: "compactQueue"; text: I18n.t("Queue"); onClicked: controls.queueRequested("show") }
            }
            Label { text: controls.connected ? I18n.message(controls.snapshot.status) || "" : I18n.t("Player disconnected — reconnecting"); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { visible: !!controls.notice; text: controls.notice; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!controls.snapshot.error; text: I18n.message(controls.snapshot.error) || ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!controls.snapshot.gapless && controls.snapshot.gapless.phase === "review"; text: controls.snapshot.gapless ? I18n.message(controls.snapshot.gapless.message) : ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!controls.snapshot.storage && ["unsaved", "blocked"].includes(controls.snapshot.storage.state); text: controls.snapshot.storage ? I18n.message(controls.snapshot.storage.message) : ""; color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
        }
    }
}
