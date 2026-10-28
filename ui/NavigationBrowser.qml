import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: navigation
    required property var snapshot
    property bool connected: false
    property var rows: []
    property string rowsJson: "[]"
    property string pageKey: ""
    property string total: "0"
    property bool followingLatest: false
    readonly property real viewportHeight: pathView.height
    signal pageRequested(var offset, var fence)
    signal revisitRequested(var fields)
    onSnapshotChanged: updateRows()
    Component.onCompleted: updateRows()
    function updateRows() {
        const entries = snapshot ? snapshot.entries : [];
        const encoded = JSON.stringify(entries);
        const key = snapshot ? snapshot.session + ":" + snapshot.offset : "";
        const pageChanged = key !== pageKey;
        const rowsChanged = encoded !== rowsJson;
        const sizeChanged = !!snapshot && snapshot.total !== total;
        const followed = !!snapshot && snapshot.following_latest && !followingLatest;
        total = snapshot ? snapshot.total : "0";
        followingLatest = !!snapshot && snapshot.following_latest;
        pageKey = key;
        if (rowsChanged) { rowsJson = encoded; rows = entries; }
        if (pageChanged || rowsChanged || followed) Qt.callLater(() => {
            if (snapshot && snapshot.following_latest && (pageChanged || sizeChanged || followed)) pathView.positionViewAtEnd();
            else if (pageChanged) pathView.positionViewAtBeginning();
        });
    }
    function fence() {
        return {queue_session: snapshot.session, queue_revision: snapshot.revision};
    }
    function revisit(row) {
        if (!connected || !snapshot || !rows.some(current => current.index === row.index && current.occurrence === row.occurrence)) return;
        const fields = fence();
        fields.index = row.index;
        fields.occurrence = row.occurrence;
        revisitRequested(fields);
    }
    Label {
        Layout.fillWidth: true
        text: I18n.t("Confirmed nominal navigation only. Selected or skipped tracks without a clock are excluded. Go here keeps paused/playing intent.")
        wrapMode: Text.Wrap; textFormat: Text.PlainText; color: Theme.muted
    }
    Label {
        visible: navigation.rows.length === 0
        text: I18n.t("No confirmed entries in the Previous path.")
        wrapMode: Text.Wrap; color: Theme.muted; Layout.fillWidth: true
    }
    ListView {
        id: pathView
        Layout.fillWidth: true; Layout.fillHeight: true
        model: navigation.rows
        clip: true; spacing: 6
        ScrollBar.vertical: ScrollBar {}
        delegate: Rectangle {
            required property var modelData
            width: pathView.width; height: 112
            color: modelData.selected ? Theme.selected : Theme.surface; radius: 5
            ColumnLayout {
                anchors.fill: parent; anchors.margins: 8; spacing: 3
                Label { text: (modelData.selected ? I18n.t("Selected path entry · ") : "") + modelData.track.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Label { text: modelData.track.artist + " · " + modelData.track.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                Button { objectName: "revisitNavigation"; text: I18n.t("Go here"); enabled: navigation.connected; onClicked: navigation.revisit(modelData) }
            }
        }
    }
    Label {
        Layout.fillWidth: true
        text: snapshot && snapshot.total !== "0" ? I18n.t("Entries %1–%2 of %3", [I18n.number(Number(snapshot.offset) + 1), I18n.number(Number(snapshot.offset) + navigation.rows.length), I18n.number(snapshot.total)]) : I18n.t("0 entries")
        color: Theme.muted; wrapMode: Text.Wrap
    }
    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: childrenRect.height
        spacing: 6
        Button { objectName: "olderNavigation"; text: I18n.t("Older"); enabled: navigation.connected && !!snapshot && snapshot.has_previous; onClicked: navigation.pageRequested(String(Math.max(0, Number(snapshot.offset) - 50)), navigation.fence()) }
        Button { objectName: "newerNavigation"; text: I18n.t("Newer"); enabled: navigation.connected && !!snapshot && snapshot.has_next; onClicked: navigation.pageRequested(String(Number(snapshot.offset) + 50), navigation.fence()) }
        Button { objectName: "latestNavigation"; text: I18n.t("Follow latest"); enabled: navigation.connected && !!snapshot; onClicked: navigation.pageRequested(null, navigation.fence()) }
    }
}
