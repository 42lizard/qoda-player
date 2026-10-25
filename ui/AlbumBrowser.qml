import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: albums
    required property var snapshot
    property bool connected: false
    property var page: snapshot ? snapshot.page : null
    property var selected: snapshot ? snapshot.selected : null
    property bool busy: !!snapshot && snapshot.busy
    property var albumModel: []
    property var trackModel: []
    property string pageJson: "[]"
    property string tracksJson: "[]"
    property string selectedKey: ""
    property bool showDetails: false
    readonly property bool coverReady: cover.status === Image.Ready
    readonly property real trackViewport: albumTracks.height
    signal openAlbum(var reference)
    signal pageRequested(string offset)
    signal queueAlbum(string op, var fields)
    signal trackAction(string op, var track)
    onSnapshotChanged: updateModels()
    Component.onCompleted: updateModels()
    function updateModels() {
        let nextPage = snapshot ? snapshot.page : null;
        let nextSelected = snapshot ? snapshot.selected : null;
        let rows = nextPage ? nextPage.albums : [];
        let encoded = JSON.stringify(rows);
        if (encoded !== pageJson) { pageJson = encoded; albumModel = rows; }
        rows = nextSelected ? nextSelected.tracks : [];
        encoded = JSON.stringify(rows);
        if (encoded !== tracksJson) { tracksJson = encoded; trackModel = rows; }
        let key = nextSelected ? JSON.stringify([nextSelected.album.reference, nextSelected.revision]) : "";
        if (key !== selectedKey) { selectedKey = key; showDetails = !!nextSelected; }
    }
    function queue(op, key) {
        if (!selected) return;
        let fields = {reference: selected.album.reference, expected_revision: selected.revision};
        if (key) fields.track_key = key;
        queueAlbum(op, fields);
    }
    Label {
        Layout.fillWidth: true
        visible: !albums.showDetails || albums.busy || !!albums.selected && albums.selected.artwork_unavailable
        text: snapshot ? snapshot.message : "Choose a music library."
        color: "#acb8c8"
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }
    RowLayout {
        Layout.fillWidth: true
        visible: albums.showDetails && !!albums.selected
        spacing: 16
        Rectangle {
            Layout.preferredWidth: 92; Layout.preferredHeight: 92
            color: "#202735"; radius: 5
            Label { anchors.centerIn: parent; text: "♪"; font.pixelSize: 44; color: "#acb8c8"; visible: cover.status !== Image.Ready }
            Image {
                id: cover
                anchors.fill: parent
                source: albums.selected && albums.selected.artwork ? albums.selected.artwork : ""
                sourceSize.width: 320; sourceSize.height: 320
                fillMode: Image.PreserveAspectFit
                asynchronous: true
                cache: false
            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Label { text: albums.selected ? albums.selected.album.title : ""; color: "#f2e9db"; font.bold: true; font.pixelSize: 20; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
            Label { text: albums.selected ? albums.selected.album.artist + (albums.selected.album.year ? " · " + albums.selected.album.year : "") : ""; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: albums.trackModel.length + " tracks · disc and track order"; color: "#acb8c8" }
            RowLayout {
                Button { text: "Back to albums"; onClicked: albums.showDetails = false }
                Button { text: "Play album"; enabled: albums.connected && !albums.busy && albums.trackModel.length > 0; onClicked: albums.queue("album_play", null) }
                Button { text: "+ Queue"; enabled: albums.connected && !albums.busy && albums.trackModel.length > 0; onClicked: albums.queue("album_enqueue", null) }
            }
        }
    }
    GridView {
        id: albumGrid
        visible: !albums.showDetails
        Layout.fillWidth: true; Layout.fillHeight: true
        cellWidth: Math.max(180, Math.floor(width / Math.max(1, Math.floor(width / 210))))
        cellHeight: 110
        model: albums.albumModel
        clip: true
        ScrollBar.vertical: ScrollBar {}
        delegate: Item {
            required property var modelData
            width: albumGrid.cellWidth; height: albumGrid.cellHeight
            Rectangle {
                anchors.fill: parent; anchors.margins: 4
                color: albumTap.hovered ? "#344354" : "#202735"; radius: 6
                HoverHandler { id: albumTap }
                ColumnLayout {
                    anchors.fill: parent; anchors.margins: 12; spacing: 5
                    Label { text: modelData.title; font.bold: true; color: "#f2e9db"; Layout.fillWidth: true; elide: Text.ElideRight; textFormat: Text.PlainText }
                    Label { text: modelData.artist; color: "#acb8c8"; Layout.fillWidth: true; elide: Text.ElideRight; textFormat: Text.PlainText }
                    Label { text: (modelData.year ? modelData.year + " · " : "") + (modelData.track_count !== null ? modelData.track_count + " tracks" : "Album"); color: "#acb8c8" }
                }
                // A focusable control gives the same action to keyboard users.
                Button {
                    anchors.fill: parent
                    flat: true
                    text: ""
                    background: Item {}
                    Accessible.name: modelData.title + " by " + modelData.artist
                    enabled: !albums.busy
                    onClicked: albums.openAlbum(modelData.reference)
                }
            }
        }
    }
    ListView {
        id: albumTracks
        visible: albums.showDetails
        Layout.fillWidth: true; Layout.fillHeight: true
        model: albums.trackModel
        clip: true; spacing: 5
        ScrollBar.vertical: ScrollBar {}
        delegate: Rectangle {
            required property var modelData
            required property int index
            width: albumTracks.width; height: 68
            radius: 5; color: index % 2 ? "#1a202a" : "#202735"
            RowLayout {
                anchors.fill: parent; anchors.margins: 8
                Label { text: modelData.disc + "." + modelData.number; color: "#acb8c8"; Layout.preferredWidth: 36 }
                ColumnLayout {
                    Layout.fillWidth: true
                    Label { text: modelData.track.title; font.bold: true; color: "#f2e9db"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Label { text: modelData.track.artist; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                }
                Button { text: "Play from here"; enabled: albums.connected && !albums.busy; onClicked: albums.queue("album_play", modelData.track.reference.key) }
                Button { text: "+ Queue"; enabled: albums.connected && !albums.busy; onClicked: albums.trackAction("enqueue", modelData.track) }
                Button { text: "Next"; enabled: albums.connected && !albums.busy; onClicked: albums.trackAction("play_next", modelData.track) }
            }
        }
    }
    RowLayout {
        visible: !albums.showDetails
        Label { text: albums.page ? albums.albumModel.length + " albums on this page" : "Choose a library to browse albums"; color: "#acb8c8"; Layout.fillWidth: true }
        Button { text: "Previous"; enabled: !!albums.page && albums.page.offset !== "0" && !albums.busy; onClicked: albums.pageRequested("previous") }
        Button { text: "Next"; enabled: !!albums.page && albums.page.has_more && !albums.busy; onClicked: albums.pageRequested("next") }
    }
}
