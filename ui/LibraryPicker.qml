import QtQuick
import QtQuick.Window
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: picker
    required property var libraries
    required property string selectedId
    readonly property var selected: libraries.find(library => library.id === selectedId) || null
    readonly property bool popupVisible: combo.popup.visible
    signal chosen(string source)
    signal searchRequested()
    signal dismissed()
    spacing: 5
    function dismiss() { if (combo && combo.popup) combo.popup.close(); }
    function title(library) { return library && library.title.trim() ? library.title.trim() : I18n.t("Untitled library"); }
    function server(library) { return library && library.server && library.server.trim() ? library.server.trim() : I18n.t("Unknown server"); }
    function suffix(library) {
        if (!library) return "";
        const peers = libraries.filter(other => title(other) === title(library) && server(other) === server(library) && other.id !== library.id);
        if (!peers.length) return "";
        let length = 8;
        while (length < library.id.length && peers.some(other => other.id.slice(0, length) === library.id.slice(0, length))) length += 4;
        return library.id.slice(0, length);
    }
    function caption(library) {
        if (!library) return I18n.t("Select a source to browse");
        const identity = suffix(library);
        return server(library) + (identity ? " · " + identity : "") + (library.offline ? I18n.t(" · cached") : "");
    }
    function label(library) { return library ? title(library) + " · " + caption(library) : I18n.t("Choose a music library"); }
    onLibrariesChanged: dismiss()
    ComboBox {
        id: combo
        objectName: "libraryCombo"
        Layout.fillWidth: true
        Layout.minimumWidth: 0
        model: picker.libraries
        enabled: picker.libraries.length > 0
        textRole: "title"
        currentIndex: picker.libraries.findIndex(library => library.id === picker.selectedId)
        displayText: picker.selected ? picker.title(picker.selected) : picker.libraries.length ? I18n.t("Choose library") : I18n.t("No libraries")
        Accessible.name: picker.label(picker.selected)
        ToolTip.visible: hovered
        ToolTip.text: picker.label(picker.selected)
        onActivated: index => {
            const library = picker.libraries[index];
            if (library && library.id !== picker.selectedId) picker.chosen(library.id);
        }
        popup: Popup {
            y: combo.height
            width: Math.min(440, picker.Window.window ? picker.Window.window.width - 32 : 440)
            padding: 1; margins: 12; focus: true
            palette: combo.palette
            onClosed: picker.dismissed()
            background: Rectangle { color: Theme.surface; border.color: Theme.border; radius: 5 }
            contentItem: Rectangle {
                color: Theme.surface
                implicitHeight: Math.min(sourceChoices.contentHeight, picker.Window.window ? Math.max(80, picker.Window.window.height - 80) : 320)
                ListView {
                    id: sourceChoices
                    objectName: "sourceChoices"
                    anchors.fill: parent; clip: true
                    model: combo.popup.visible ? combo.delegateModel : null
                    currentIndex: combo.highlightedIndex
                    ScrollBar.vertical: ScrollBar {}
                }
            }
            Shortcut { sequence: "Ctrl+K"; enabled: combo.popup.visible; onActivated: { picker.dismiss(); picker.searchRequested(); } }
        }
        delegate: ItemDelegate {
            required property var modelData
            width: ListView.view.width
            implicitHeight: details.implicitHeight + 16
            highlighted: combo.highlightedIndex === index
            required property int index
            Accessible.name: picker.label(modelData)
            background: Rectangle { color: parent.highlighted ? Theme.selected : parent.hovered ? Theme.button : Theme.surface }
            contentItem: ColumnLayout {
                id: details
                spacing: 3
                Label { text: picker.title(modelData); font.bold: true; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true; Layout.minimumWidth: 0 }
                Label { text: picker.caption(modelData); color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true; Layout.minimumWidth: 0 }
            }
        }
    }
    Label {
        Layout.fillWidth: true
        Layout.minimumWidth: 0
        text: picker.selected && picker.suffix(picker.selected) ? picker.suffix(picker.selected) + "\n" + picker.server(picker.selected) : picker.caption(picker.selected)
        font.pixelSize: 12
        color: Theme.muted; textFormat: Text.PlainText
        wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight
        ToolTip.visible: sourceHover.hovered; ToolTip.text: picker.label(picker.selected)
        HoverHandler { id: sourceHover }
    }
}
