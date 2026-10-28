import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: folders
    required property var snapshot
    property bool connected: false
    property string problem: ""
    property string installedPaths: ""
    readonly property bool available: connected && !!snapshot && snapshot.available
    readonly property bool scanning: !!snapshot && snapshot.phase === "scanning"
    readonly property bool dirty: JSON.stringify(paths()) !== JSON.stringify(snapshot ? snapshot.paths || [] : [])
    signal applyRequested(var paths)
    signal rescanRequested()
    spacing: 8
    ListModel { id: draft }
    function paths() { let result = []; for (let i = 0; i < draft.count; ++i) result.push(draft.get(i).path); return result; }
    function sync() {
        const values = snapshot ? snapshot.paths || [] : [];
        const json = JSON.stringify(values);
        if (installedPaths === json) return;
        installedPaths = json; draft.clear();
        for (const path of values) draft.append({path: path});
    }
    onSnapshotChanged: sync()
    Component.onCompleted: sync()
    Label { text: I18n.t("Local music folders"); font.bold: true; color: Theme.foreground; Layout.fillWidth: true }
    Label { text: I18n.t("Folders are read-only and watched recursively. Removing a folder never deletes its music. Applying changes pauses playback."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!folders.snapshot && folders.snapshot.defaulted; text: I18n.t("Using the XDG music folder default."); color: Theme.muted; Layout.fillWidth: true; wrapMode: Text.Wrap }
    Repeater {
        model: draft
        delegate: RowLayout {
            required property int index
            required property string path
            Layout.fillWidth: true
            TextField {
                objectName: "localFolderPath"
                text: path; Layout.fillWidth: true; maximumLength: 4096
                Accessible.name: I18n.t("Local folder path")
                enabled: folders.available
                onTextEdited: draft.setProperty(index, "path", text)
            }
            Button { objectName: "removeLocalFolder"; text: I18n.t("Remove"); enabled: folders.available; onClicked: draft.remove(index) }
        }
    }
    RowLayout {
        Layout.fillWidth: true
        TextField { id: addPath; objectName: "newLocalFolder"; placeholderText: I18n.t("Absolute folder path"); maximumLength: 4096; Layout.fillWidth: true; enabled: folders.available; onAccepted: if (add.enabled) add.clicked() }
        Button { id: add; objectName: "addLocalFolder"; text: I18n.t("Add folder"); enabled: folders.available && addPath.text.trim().length > 0 && draft.count < 16; onClicked: { draft.append({path: addPath.text.trim()}); addPath.clear(); } }
    }
    Flow {
        Layout.fillWidth: true; Layout.preferredHeight: childrenRect.height; spacing: 6
        Button { objectName: "applyLocalFolders"; text: I18n.t("Apply folders"); enabled: folders.available && folders.dirty; onClicked: folders.applyRequested(folders.paths()) }
        Button { objectName: "rescanLocalFolders"; text: I18n.t("Rescan now"); enabled: folders.available && !folders.scanning; onClicked: folders.rescanRequested() }
    }
    Label { text: folders.snapshot ? I18n.message(folders.snapshot.message) : I18n.t("Local folder settings unavailable"); color: Theme.muted; textFormat: Text.PlainText; Layout.fillWidth: true; wrapMode: Text.Wrap }
    Label { visible: !!folders.problem; text: I18n.message(folders.problem); color: Theme.danger; textFormat: Text.PlainText; Layout.fillWidth: true; wrapMode: Text.Wrap }
    Repeater {
        model: folders.snapshot ? folders.snapshot.folders || [] : []
        delegate: ColumnLayout {
            required property var modelData
            Layout.fillWidth: true
            Label { text: modelData.path; color: Theme.foreground; textFormat: Text.PlainText; Layout.fillWidth: true; wrapMode: Text.WrapAnywhere }
            Label { text: I18n.count("%1 track", "%1 tracks", modelData.tracks); color: Theme.muted }
            Label { text: I18n.message(modelData.message); color: Theme.muted; textFormat: Text.PlainText; Layout.fillWidth: true; wrapMode: Text.Wrap }
        }
    }
}
