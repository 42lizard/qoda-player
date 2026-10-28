import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: panel
    required property var snapshot
    property var folders: []
    property bool connected: false
    property string problem: ""
    property string chosenSource: ""
    readonly property bool available: connected && !!snapshot && snapshot.available
    function syncFolder() {
        if (!destination) return;
        const source = chosenSource || (snapshot && snapshot.destination ? snapshot.destination.source : "");
        const index = folders.findIndex(folder => folder.id === source);
        destination.currentIndex = index >= 0 ? index : (folders.length ? 0 : -1);
    }
    onFoldersChanged: syncFolder()
    Component.onCompleted: syncFolder()
    signal actionRequested(string op, var fields)
    signal foldersRequested()
    spacing: 10
    function phase(value) {
        const labels = {queued: "Queued", running: "Downloading", paused: "Paused", completed: "Completed", failed: "Failed", cancelled: "Cancelled"};
        return I18n.t(labels[value] || "Unknown");
    }
    Label { text: I18n.t("Downloads"); color: Theme.foreground; font.bold: true; Layout.fillWidth: true }
    Label { text: I18n.t("Save original Plex files as local music. Nothing plays automatically. Completed files are kept when you disconnect or clear history."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: I18n.t("Choose a watched folder. Create / use folder explicitly creates missing folders and the Qoda Downloads subfolder."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    ComboBox {
        id: destination
        objectName: "downloadFolder"
        Layout.fillWidth: true
        model: panel.folders
        textRole: "path"
        enabled: panel.available && count > 0
        Accessible.name: I18n.t("Download destination")
        onActivated: panel.chosenSource = model[currentIndex].id
    }
    Flow {
        Layout.fillWidth: true; Layout.preferredHeight: childrenRect.height; spacing: 6
        Button {
            objectName: "useDownloadFolder"
            text: I18n.t("Create / use folder")
            enabled: panel.available && panel.folders.length > 0
            onClicked: {
                const source = destination.currentIndex >= 0 ? panel.folders[destination.currentIndex].id : "";
                panel.actionRequested("download_destination", {source: source});
            }
        }
        Button { text: I18n.t("Local music folders"); enabled: panel.connected; onClicked: panel.foldersRequested() }
        Button { objectName: "clearDownloads"; text: I18n.t("Clear finished history"); enabled: panel.available; onClicked: panel.actionRequested("download_clear", {}) }
    }
    Label { visible: panel.folders.length === 0; text: I18n.t("Add a local music folder first. Disabled local folders are never enabled automatically."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: panel.snapshot && panel.snapshot.destination ? I18n.t("Destination: %1", [panel.snapshot.destination.path]) : I18n.t("No download destination selected."); color: Theme.foreground; wrapMode: Text.WrapAnywhere; textFormat: Text.PlainText; Layout.fillWidth: true }
    Label { text: I18n.t("One transfer at a time · 200 jobs · 8 GiB per file · 100 GiB managed storage. Continue safely restarts from zero; originals are never silently transcoded."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!panel.problem || !!panel.snapshot && !!panel.snapshot.message; text: I18n.message(panel.problem || (panel.snapshot ? panel.snapshot.message : "")); color: Theme.danger; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true }
    ListView {
        id: jobs
        objectName: "downloadJobs"
        model: panel.snapshot ? panel.snapshot.jobs || [] : []
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(440, contentHeight)
        clip: true; spacing: 10
        ScrollBar.vertical: ScrollBar {}
        delegate: ColumnLayout {
            required property var modelData
            width: jobs.width; spacing: 5
            Label { text: modelData.track.title; color: Theme.foreground; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { text: modelData.track.artist + " · " + modelData.track.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
            Label { visible: !!modelData.destination; text: I18n.t("Folder: %1", [modelData.destination || ""]); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideMiddle; Layout.fillWidth: true }
            Label { visible: !!modelData.unwatched; text: I18n.t("Add this folder to Local music folders to queue its copy."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { text: panel.phase(modelData.phase) + " · " + (modelData.total ? I18n.t("%1 / %2 bytes", [I18n.number(modelData.bytes, 0), I18n.number(modelData.total, 0)]) : I18n.t("%1 bytes", [I18n.number(modelData.bytes, 0)])); color: Theme.muted; Layout.fillWidth: true; wrapMode: Text.Wrap }
            ProgressBar { visible: modelData.phase === "running"; indeterminate: !modelData.total; value: modelData.total ? Number(modelData.bytes) / Number(modelData.total) : 0; Layout.fillWidth: true }
            Label { visible: !!modelData.message; text: I18n.message(modelData.message); color: modelData.phase === "failed" ? Theme.danger : Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Label { visible: !!modelData.missing; text: I18n.t("The downloaded file is missing or changed. Browse local music to select changed content explicitly."); color: Theme.danger; wrapMode: Text.Wrap; Layout.fillWidth: true }
            Flow {
                Layout.fillWidth: true; Layout.preferredHeight: childrenRect.height; spacing: 6
                Button { objectName: "continueDownload"; visible: ["paused", "failed", "cancelled"].includes(modelData.phase); text: modelData.phase === "paused" ? I18n.t("Continue from zero") : I18n.t("Retry from zero"); enabled: panel.available; onClicked: panel.actionRequested("download_retry", {job_id: modelData.id}) }
                Button { objectName: "cancelDownload"; visible: ["queued", "running", "paused"].includes(modelData.phase); text: I18n.t("Cancel"); enabled: panel.available; onClicked: panel.actionRequested("download_cancel", {job_id: modelData.id}) }
                Button { objectName: "queueLocalDownload"; visible: modelData.phase === "completed"; text: I18n.t("Add local copy to queue"); enabled: panel.available && !!modelData.local; onClicked: panel.actionRequested("download_local", {job_id: modelData.id}) }
            }
        }
    }
    Label { visible: jobs.count === 0; text: I18n.t("Choose Download original from a Plex track's menu or download selected tracks. Downloaded files appear in the local library."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
}
