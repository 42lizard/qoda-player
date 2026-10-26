import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Drawer {
    id: sheet
    property string section: "account"
    default property alias settingsContent: body.data
    readonly property bool scrollable: scroll.contentHeight > scroll.availableHeight
    signal sectionRequested(string section)
    signal closeRequested()
    signal searchRequested()
    Shortcut { sequence: "Ctrl+K"; enabled: sheet.visible; onActivated: sheet.searchRequested() }
    edge: Qt.RightEdge
    modal: true
    focus: true
    leftPadding: 16; rightPadding: 16; topPadding: 16; bottomPadding: 16
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    background: Rectangle { color: Theme.surface; border.color: Theme.border }
    contentItem: Rectangle {
        color: Theme.surface
        ColumnLayout {
            anchors.fill: parent
            spacing: 12
            RowLayout {
                Layout.fillWidth: true
                Label { text: "Settings"; color: Theme.foreground; font.pixelSize: 20; font.bold: true; Layout.fillWidth: true }
                Button { text: "Close"; Accessible.name: "Close settings"; onClicked: sheet.closeRequested() }
            }
            Flow {
                id: sectionTabs
                Layout.fillWidth: true
                Layout.preferredHeight: childrenRect.height
                property int columns: width >= 380 ? 4 : 2
                spacing: 6
                Repeater {
                    model: [{key: "account", title: "Account"}, {key: "audio", title: "Audio"}, {key: "cache", title: "Cache"}, {key: "history", title: "History"}]
                    Button {
                        required property var modelData
                        text: modelData.title
                        highlighted: sheet.section === modelData.key
                        width: (sectionTabs.width - sectionTabs.spacing * (sectionTabs.columns - 1)) / sectionTabs.columns
                        onClicked: sheet.sectionRequested(modelData.key)
                    }
                }
            }
            ScrollView {
                id: scroll
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                contentWidth: availableWidth
                ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
                Column { id: body; width: scroll.availableWidth; spacing: 12 }
            }
        }
    }
}
