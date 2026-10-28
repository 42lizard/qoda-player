import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: albums
    required property var snapshot
    property bool connected: false
    property bool artworkEnabled: true
    property var covers: snapshot && snapshot.covers ? snapshot.covers : ({})
    readonly property int readyThumbnails: albumGrid.contentItem.children.filter(item => item.thumbnailReady).length
    property var page: snapshot ? snapshot.page : null
    property var selected: snapshot ? snapshot.selected : null
    property bool busy: !!snapshot && snapshot.busy
    readonly property bool canQueue: connected && !busy && !!selected && (!snapshot.cache || snapshot.cache.origin === "live")
    property var albumModel: []
    property bool downloadsEnabled: false
    signal downloadRequested(var fields)
    property var trackModel: []
    property string pageJson: "[]"
    property string tracksJson: "[]"
    property string selectedKey: ""
    property bool showDetails: false
    readonly property bool coverReady: cover.status === Image.Ready
    readonly property string detailCoverSource: selected && artworkEnabled && showDetails && visible ? covers[selected.album.reference.key] || "" : ""
    readonly property real trackViewport: albumTracks.height
    signal openAlbum(var reference)
    signal openArtist(var reference)
    signal allAlbums()
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
        let key = nextSelected ? JSON.stringify(nextSelected.album.reference) : "";
        if (key !== selectedKey) { selectedKey = key; showDetails = !!nextSelected; }
    }
    function queue(op, key) {
        if (!canQueue) return;
        let fields = {reference: selected.album.reference, expected_revision: selected.revision};
        if (key) fields.track_key = key;
        queueAlbum(op, fields);
    }
    Label {
        Layout.fillWidth: true
        visible: !albums.showDetails || albums.busy || !!albums.selected && (!albums.coverReady || !!snapshot.cache && snapshot.cache.origin !== "live")
        text: snapshot ? I18n.message(snapshot.message) : I18n.t("Choose a music library.")
        color: Theme.muted
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }
    RowLayout {
        visible: !!albums.page && !!albums.page.artist && !albums.showDetails
        Button { text: I18n.t("All albums"); onClicked: albums.allAlbums() }
        Label { text: albums.page && albums.page.artist ? albums.page.artist.title : ""; Layout.fillWidth: true; textFormat: Text.PlainText; elide: Text.ElideRight; color: Theme.foreground; font.bold: true }
    }
    RowLayout {
        Layout.fillWidth: true
        visible: albums.showDetails && !!albums.selected
        spacing: 16
        Rectangle {
            Layout.preferredWidth: 92; Layout.preferredHeight: 92
            color: Theme.surface; radius: 5
            Label { anchors.centerIn: parent; text: "♪"; font.pixelSize: 44; color: Theme.muted; visible: cover.status !== Image.Ready }
            Image {
                id: cover
                anchors.fill: parent
                source: albums.detailCoverSource
                sourceSize.width: 320; sourceSize.height: 320
                fillMode: Image.PreserveAspectFit
                asynchronous: true
                cache: false
            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Label { text: albums.selected ? albums.selected.album.title : ""; color: Theme.foreground; font.bold: true; font.pixelSize: 20; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
            Button {
                Layout.fillWidth: true
                flat: true
                enabled: !!albums.selected && !!albums.selected.album.artist_ref && !albums.busy
                contentItem: Text { text: albums.selected ? albums.selected.album.artist + (albums.selected.album.year ? " · " + albums.selected.album.year : "") + "  ›" : ""; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight }
                onClicked: albums.openArtist(albums.selected.album.artist_ref)
            }
            Label { text: I18n.count("%1 track · disc and track order", "%1 tracks · disc and track order", albums.trackModel.length); color: Theme.muted }
            Flow {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                Layout.preferredHeight: childrenRect.height
                spacing: 6
                Button { text: I18n.t("Back to albums"); onClicked: albums.showDetails = false }
                Button { objectName: "refreshAlbum"; text: I18n.t("Refresh album"); enabled: !albums.busy; onClicked: albums.openAlbum(albums.selected.album.reference) }
                Button { objectName: "playAlbum"; text: I18n.t("Play album"); enabled: albums.canQueue && albums.trackModel.length > 0; onClicked: albums.queue("album_play", null) }
                Button { objectName: "queueAlbum"; text: I18n.t("+ Queue"); enabled: albums.canQueue && albums.trackModel.length > 0; onClicked: albums.queue("album_enqueue", null) }
                Button { objectName: "downloadAlbum"; text: I18n.t("Download album"); visible: albums.downloadsEnabled; enabled: albums.canQueue && albums.trackModel.length > 0; onClicked: albums.downloadRequested({reference: albums.selected.album.reference, expected_revision: albums.selected.revision}) }
            }
        }
    }
    GridView {
        id: albumGrid
        visible: !albums.showDetails
        Layout.fillWidth: true; Layout.fillHeight: true
        cellWidth: Math.max(180, Math.floor(width / Math.max(1, Math.floor(width / 210))))
        cellHeight: 242
        cacheBuffer: 0
        model: albums.albumModel
        clip: true
        ScrollBar.vertical: ScrollBar {}
        delegate: Item {
            id: albumCard
            required property var modelData
            property bool thumbnailReady: tileCover.status === Image.Ready
            width: albumGrid.cellWidth; height: albumGrid.cellHeight
            Rectangle {
                anchors.fill: parent; anchors.margins: 4
                color: albumTap.hovered ? Theme.selected : Theme.surface; radius: 6
                HoverHandler { id: albumTap }
                ColumnLayout {
                    anchors.fill: parent; anchors.margins: 12; spacing: 5
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 142
                        color: Theme.background; radius: 4
                        Label { anchors.centerIn: parent; text: "♪"; font.pixelSize: 44; color: Theme.muted; visible: tileCover.status !== Image.Ready }
                        Image {
                            id: tileCover
                            anchors.fill: parent
                            source: albums.visible && albumGrid.visible && albums.artworkEnabled && albumCard.y + albumCard.height >= albumGrid.contentY && albumCard.y <= albumGrid.contentY + albumGrid.height ? albums.covers[modelData.reference.key] || "" : ""
                            sourceSize.width: 180; sourceSize.height: 180
                            asynchronous: true; cache: false
                            fillMode: Image.PreserveAspectFit
                        }
                    }
                    Label { text: modelData.title; font.bold: true; color: Theme.foreground; Layout.fillWidth: true; elide: Text.ElideRight; textFormat: Text.PlainText }
                    Label { text: modelData.artist; color: Theme.muted; Layout.fillWidth: true; elide: Text.ElideRight; textFormat: Text.PlainText }
                    Label { text: (modelData.year ? modelData.year + " · " : "") + (modelData.track_count !== null ? I18n.count("%1 track", "%1 tracks", modelData.track_count) : I18n.t("Album")); color: Theme.muted }
                }
                // A focusable control gives the same action to keyboard users.
                Button {
                    anchors.fill: parent
                    flat: true
                    text: ""
                    background: Item {}
                    Accessible.name: I18n.t("%1 by %2", [modelData.title, modelData.artist])
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
            id: albumTrackRow
            required property var modelData
            required property int index
            width: albumTracks.width; height: trackLayout.implicitHeight + 16
            radius: 5; color: index % 2 ? Theme.alternate : Theme.surface
            GridLayout {
                id: trackLayout
                columns: albumTrackRow.width >= 720 ? 2 : 1
                anchors.fill: parent; anchors.margins: 8
                RowLayout {
                    Layout.fillWidth: true
                    Label { text: modelData.disc + "." + modelData.number; color: Theme.muted; Layout.preferredWidth: 36 }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Label { text: modelData.track.title; font.bold: true; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                        Label { text: modelData.track.artist; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                }
                }
                Flow {
                    objectName: "albumTrackActions"
                    Layout.fillWidth: true; Layout.preferredHeight: childrenRect.height; spacing: 6
                    Button { text: "⋮"; implicitWidth: 28; visible: albums.downloadsEnabled; Accessible.name: I18n.t("More track actions"); onClicked: trackMenu.open()
                        Menu { id: trackMenu; MenuItem { text: I18n.t("Download original"); enabled: albums.canQueue && albums.downloadsEnabled; onTriggered: albums.downloadRequested({reference: albums.selected.album.reference, expected_revision: albums.selected.revision, track_key: modelData.track.reference.key}) } }
                }
                Button { objectName: "playAlbumFromHere"; text: I18n.t("Play from here"); enabled: albums.canQueue; onClicked: albums.queue("album_play", modelData.track.reference.key) }
                Button { objectName: "enqueueAlbumTrack"; text: I18n.t("+ Queue"); enabled: albums.canQueue; onClicked: albums.trackAction("enqueue", modelData.track) }
                Button { objectName: "nextAlbumTrack"; text: I18n.t("Next"); enabled: albums.canQueue; onClicked: albums.trackAction("play_next", modelData.track) }
                }            }
        }
    }
    RowLayout {
        visible: !albums.showDetails
        Label { text: albums.page ? I18n.count("%1 album on this page", "%1 albums on this page", albums.albumModel.length) : I18n.t("Choose a library to browse albums"); color: Theme.muted; Layout.fillWidth: true }
        Button { text: I18n.t("Previous"); enabled: !!albums.page && albums.page.offset !== "0" && !albums.busy; onClicked: albums.pageRequested("previous") }
        Button { text: I18n.t("Next"); enabled: !!albums.page && albums.page.has_more && !albums.busy; onClicked: albums.pageRequested("next") }
    }
}
