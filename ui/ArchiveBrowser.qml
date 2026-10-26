import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: archive
    required property var snapshot
    property bool connected: false
    property string accountEpoch: ""
    property var libraries: []
    readonly property bool ready: connected && !!snapshot && snapshot.available
    readonly property var rows: snapshot ? snapshot.entries || [] : []
    readonly property var policies: [90, 30, 7, 365, 0]
    property int draftDays: snapshot ? snapshot.retention_days : 90
    signal requested(string operation, var fields)
    property int persistedDays: -1
    onSnapshotChanged: {
        if (snapshot && snapshot.retention_days !== persistedDays) {
            persistedDays = snapshot.retention_days;
            draftDays = persistedDays;
        }
    }
    onAccountEpochChanged: clearDialog.close()
    onConnectedChanged: { if (!connected) clearDialog.close(); }
    function request(operation, fields) {
        if (!ready && !(connected && snapshot && snapshot.cleanup_pending && operation === "archive_clear")) return;
        fields.account_epoch = accountEpoch;
        requested(operation, fields);
    }
    function source(row) {
        const library = libraries.find(item => item.id === row.track.source);
        return library ? library.title + " · " + library.server : "Unlisted source";
    }
    function position(ms) {
        const seconds = Math.floor(ms / 1000);
        return Math.floor(seconds / 60) + ":" + String(seconds % 60).padStart(2, "0");
    }
    Label { text: "Listening archive"; color: Theme.foreground; font.bold: true; font.pixelSize: 18 }
    Label {
        Layout.fillWidth: true
        text: snapshot ? snapshot.message : "Opening listening archive…"
        wrapMode: Text.Wrap; textFormat: Text.PlainText; color: archive.snapshot && archive.snapshot.incomplete ? Theme.danger : Theme.muted
    }
    Label {
        Layout.fillWidth: true
        text: "Nominal playback visits across sessions. Seeking and recovery keep a visit; repeat and replay create new visits. Positions do not measure time spent listening."
        wrapMode: Text.Wrap; textFormat: Text.PlainText; color: Theme.muted
    }
    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: childrenRect.height
        spacing: 6
        ComboBox {
            objectName: "archiveRetention"
            width: Math.min(170, parent.width)
            model: ["90 days", "30 days", "7 days", "365 days", "Off · clear archive"]
            currentIndex: archive.policies.indexOf(archive.draftDays)
            enabled: archive.ready
            onActivated: index => archive.draftDays = archive.policies[index]
            Accessible.name: "Archive retention"
        }
        Button {
            objectName: "applyArchiveRetention"; text: "Apply retention"
            enabled: archive.ready && archive.snapshot.retention_days !== archive.draftDays
            onClicked: { if (archive.draftDays === 0) clearDialog.policyChange = true; else archive.request("archive_retention", {days: archive.draftDays}); if (archive.draftDays === 0) clearDialog.open(); }
        }
        Button { objectName: "exportArchive"; text: "Export JSON"; enabled: archive.ready; onClicked: archive.request("archive_export", {}) }
        Button { objectName: "clearArchive"; text: archive.snapshot && archive.snapshot.cleanup_pending ? "Retry account cleanup" : "Clear archive…"; enabled: archive.connected && !!archive.snapshot && (archive.snapshot.cleanup_pending || archive.ready && archive.snapshot.total > 0); onClicked: { if (archive.snapshot.cleanup_pending) archive.request("archive_clear", {}); else { clearDialog.policyChange = false; clearDialog.open(); } } }
    }
    Label {
        Layout.fillWidth: true
        text: "Exports are private copies retained after archive cleanup or sign-out. Remove those copies yourself when no longer needed."
        wrapMode: Text.Wrap; textFormat: Text.PlainText; color: Theme.muted
    }
    TextField {
        objectName: "archiveExportPath"
        Layout.fillWidth: true
        visible: !!archive.snapshot && !!archive.snapshot.export_path
        text: archive.snapshot ? archive.snapshot.export_path || "" : ""
        readOnly: true; selectByMouse: true
        Accessible.name: "Exported listening archive path"
    }
    Label { visible: archive.rows.length === 0; text: "No retained nominal visits."; color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    ListView {
        id: visits
        Layout.fillWidth: true
        Layout.preferredHeight: archive.rows.length ? 330 : 0
        clip: true; spacing: 6
        model: archive.rows
        ScrollBar.vertical: ScrollBar {}
        delegate: Rectangle {
            required property var modelData
            width: visits.width; height: 116; radius: 5; color: Theme.surface
            border.color: Theme.border
            ColumnLayout {
                anchors.fill: parent; anchors.margins: 9; spacing: 2
                Label { text: modelData.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: modelData.artist + " · " + modelData.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: archive.source(modelData); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: new Date(modelData.first_observed_ms).toLocaleString(Qt.locale(), Locale.ShortFormat) + " · position " + archive.position(modelData.last_position_ms); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            }
        }
    }
    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: childrenRect.height
        spacing: 6
        Button { objectName: "newerArchive"; text: "Newer"; enabled: archive.ready && archive.snapshot.offset > 0; onClicked: archive.request("archive_page", {offset: Math.max(0, archive.snapshot.offset - 20)}) }
        Button { objectName: "olderArchive"; text: "Older"; enabled: archive.ready && archive.snapshot.has_next; onClicked: archive.request("archive_page", {offset: archive.snapshot.offset + 20}) }
        Label { text: archive.snapshot ? archive.snapshot.total + " retained visits" : ""; color: Theme.muted; padding: 6 }
    }
    Dialog {
        id: clearDialog
        objectName: "confirmArchiveClear"
        property bool policyChange: false
        property string openedEpoch: ""
        onOpened: openedEpoch = archive.accountEpoch
        modal: true
        title: policyChange ? "Disable archive recording?" : "Clear listening archive?"
        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: if (openedEpoch === archive.accountEpoch) archive.request(policyChange ? "archive_retention" : "archive_clear", policyChange ? {days: 0} : {})
        Label { width: Math.min(320, archive.width - 24); text: "Retained visits will be removed. Your music, queue and exported copies remain."; wrapMode: Text.Wrap; textFormat: Text.PlainText; color: Theme.foreground }
    }
}
