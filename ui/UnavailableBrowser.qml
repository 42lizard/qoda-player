import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: failures
    required property var snapshot
    property bool connected: false
    property bool canRetry: false
    property var rows: []
    property string rowsJson: "[]"
    signal actionRequested(string op, var fields)
    onSnapshotChanged: updateRows()
    Component.onCompleted: updateRows()
    function updateRows() {
        const entries = snapshot && snapshot.unavailable_entries ? snapshot.unavailable_entries : [];
        const encoded = JSON.stringify(entries);
        if (encoded !== rowsJson) { rowsJson = encoded; rows = entries; }
    }
    function reason(value) {
        if (value === "missing") return I18n.t("Track missing from its library");
        if (value === "unsupported") return I18n.t("Unsupported media version");
        return I18n.t("Saved media version changed; browse and Play now to accept a new version");
    }
    function act(op, row) {
        if (!connected || !snapshot || (op === "retry_unavailable" && !canRetry)) return;
        if (row && !rows.some(current => current.id === row.id)) return;
        if (!row && (!snapshot.selected_unavailable || !canRetry)) return;
        const fields = {queue_session: snapshot.session, queue_revision: snapshot.revision};
        if (row) fields.occurrence = row.id;
        actionRequested(op, fields);
    }
    Label {
        Layout.fillWidth: true
        text: I18n.t("Last known individual failures stay in this session. Automatic progression skips these entries; server or sign-in failures pause for recovery.")
        color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap
    }
    Button {
        objectName: "skipUnavailable"
        text: I18n.t("Skip & continue")
        visible: !!snapshot && !!snapshot.selected_unavailable
        enabled: failures.connected && failures.canRetry
        onClicked: failures.act("skip_unavailable", null)
    }
    Label { visible: failures.rows.length === 0; text: I18n.t("No unavailable entries."); color: Theme.muted }
    ListView {
        id: entries
        Layout.fillWidth: true; Layout.fillHeight: true
        clip: true; spacing: 6; model: failures.rows
        ScrollBar.vertical: ScrollBar {}
        delegate: Rectangle {
            required property var modelData
            width: entries.width; height: 150; radius: 5
            color: modelData.selected ? Theme.selected : Theme.surface
            ColumnLayout {
                anchors.fill: parent; anchors.margins: 8; spacing: 3
                Label { text: modelData.track.title; font.bold: true; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: modelData.track.artist; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: failures.reason(modelData.unavailable); color: Theme.danger; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
                RowLayout {
                    Button { objectName: "retryUnavailable"; text: I18n.t("Retry"); enabled: failures.connected && failures.canRetry; onClicked: failures.act("retry_unavailable", modelData) }
                    Button { objectName: "removeUnavailable"; text: I18n.t("Remove"); enabled: failures.connected; onClicked: failures.act("queue_remove", modelData) }
                }
            }
        }
    }
}
