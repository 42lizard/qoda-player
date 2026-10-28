import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: artists
    required property var snapshot
    property var page: snapshot ? snapshot.artists : null
    property bool busy: !!snapshot && snapshot.busy
    property var artistModel: []
    property string modelJson: "[]"
    signal openArtist(var reference)
    signal pageRequested(string direction)
    function updateModel() {
        let rows = snapshot && snapshot.artists ? snapshot.artists.artists : [];
        let encoded = JSON.stringify(rows);
        if (encoded !== modelJson) { modelJson = encoded; artistModel = rows; }
    }
    onSnapshotChanged: updateModel()
    Component.onCompleted: updateModel()
    Label { text: artists.snapshot ? I18n.message(artists.snapshot.message) : I18n.t("Choose a music library."); Layout.fillWidth: true; wrapMode: Text.Wrap; color: Theme.muted; textFormat: Text.PlainText }
    ListView {
        id: list
        Layout.fillWidth: true; Layout.fillHeight: true
        clip: true; spacing: 6
        model: artists.artistModel
        ScrollBar.vertical: ScrollBar {}
        delegate: Button {
            required property var modelData
            width: list.width; height: 58
            enabled: !artists.busy
            contentItem: Text { text: modelData.title + "  ›"; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            background: Rectangle { color: parent.hovered || parent.visualFocus ? Theme.selected : Theme.surface; radius: 5 }
            onClicked: artists.openArtist(modelData.reference)
        }
    }
    RowLayout {
        Label { text: artists.page ? I18n.count("%1 artist on this page", "%1 artists on this page", artists.artistModel.length) : I18n.t("Choose a library to browse artists"); Layout.fillWidth: true; color: Theme.muted }
        Button { text: I18n.t("Previous"); enabled: !!artists.page && artists.page.offset !== "0" && !artists.busy; onClicked: artists.pageRequested("previous") }
        Button { text: I18n.t("Next"); enabled: !!artists.page && artists.page.has_more && !artists.busy; onClicked: artists.pageRequested("next") }
    }
}
