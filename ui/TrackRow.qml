import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: row
    required property var track
    required property int index
    required property bool selected
    property bool playlistEditable: false
    signal selectionRequested(int modifiers)
    signal checkboxRequested()
    signal actionRequested(string op)
    signal playlistRequested()
    height: 68
    radius: 5
    color: selected ? Theme.selected : index % 2 ? Theme.alternate : Theme.surface
    RowLayout {
        anchors.fill: parent
        anchors.margins: 10
        CheckBox {
            objectName: "trackCheckbox"
            checked: row.selected
            Accessible.name: "Select " + row.track.title
            onClicked: row.checkboxRequested()
        }
        ColumnLayout {
            objectName: "trackDetails"
            Layout.fillWidth: true
            TapHandler { onTapped: row.selectionRequested(point.modifiers) }
            Label { text: row.track.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: row.track.artist + " · " + row.track.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
        }
        Button { objectName: "playNow"; text: "Play now"; onClicked: row.actionRequested("play") }
        Button { text: "Next"; onClicked: row.actionRequested("play_next") }
        Button { text: "+ Playlist"; enabled: row.playlistEditable; onClicked: row.playlistRequested() }
        Button { text: "+ Queue"; onClicked: row.actionRequested("enqueue") }
    }
}
