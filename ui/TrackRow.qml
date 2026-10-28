import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: row
    required property var track
    required property int index
    required property bool selected
    property bool playlistEditable: false
    property bool localCopyAvailable: false
    property bool downloadEnabled: false
    readonly property bool wide: width >= 720
    signal selectionRequested(int modifiers)
    signal checkboxRequested()
    signal actionRequested(string op)
    signal playlistRequested()
    signal downloadRequested()
    signal localCopyRequested()
    height: wide ? Math.max(68, actions.height + 20) : details.implicitHeight + actions.height + 28
    radius: 5
    color: selected ? Theme.selected : index % 2 ? Theme.alternate : Theme.surface
    RowLayout {
        id: details
        x: 10; y: row.wide ? (row.height - height) / 2 : 10
        width: row.wide ? row.width - actions.width - 30 : row.width - 20
        height: implicitHeight
        CheckBox {
            objectName: "trackCheckbox"
            checked: row.selected
            Accessible.name: I18n.t("Select %1", [row.track.title])
            onClicked: row.checkboxRequested()
        }
        ColumnLayout {
            objectName: "trackDetails"
            Layout.fillWidth: true
            TapHandler { onTapped: row.selectionRequested(point.modifiers) }
            Label { text: row.track.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: row.track.artist + " · " + row.track.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
        }
    }
    Flow {
        id: actions
        objectName: "trackActions"
        spacing: 6
        function preferredWidth() {
            let total = 0; let count = 0;
            for (const child of children) if (child.visible) { total += child.implicitWidth; ++count; }
            return total + spacing * Math.max(0, count - 1);
        }
        width: row.wide ? Math.min(preferredWidth(), row.width - 150) : row.width - 20
        height: childrenRect.height
        x: row.wide ? row.width - width - 10 : 10
        y: row.wide ? (row.height - height) / 2 : details.y + details.height + 8
        Button {
            objectName: "moreTrackActions"
            text: "⋮"; implicitWidth: 28
            visible: row.downloadEnabled || row.localCopyAvailable
            Accessible.name: I18n.t("More track actions")
            onClicked: more.open()
            Menu {
                id: more
                MenuItem { objectName: "downloadOriginal"; text: I18n.t("Download original"); visible: row.downloadEnabled; onTriggered: row.downloadRequested() }
                MenuItem { objectName: "queueLocalCopy"; text: I18n.t("Add local copy to queue"); visible: row.localCopyAvailable; onTriggered: row.localCopyRequested() }
            }
        }
        Button { objectName: "playNow"; text: I18n.t("Play now"); onClicked: row.actionRequested("play") }
        Button { text: I18n.t("Next"); onClicked: row.actionRequested("play_next") }
        Button { text: I18n.t("+ Playlist"); enabled: row.playlistEditable; onClicked: row.playlistRequested() }
        Button { text: I18n.t("+ Queue"); onClicked: row.actionRequested("enqueue") }
    }
}
