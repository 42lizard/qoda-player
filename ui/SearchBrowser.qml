import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: browser
    required property var snapshot
    required property string query
    required property bool connected
    property var rows: []
    property var sources: []
    property var libraries: []
    function serverName(row) {
        const source = row.id || (row.track ? row.track.reference.source : "");
        return libraries.some(l => l.id === source && l.kind === "local") ? I18n.t("Local files") : row.server;
    }
    property string rowsJson: "[]"
    property string sourcesJson: "[]"
    readonly property bool currentQuery: !!snapshot && snapshot.query === query.trim()
    function canDownload(source) { return libraries.some(l => l.id === source && l.downloads === true); }
    signal actionRequested(string op, var fields)
    onSnapshotChanged: sync()
    Component.onCompleted: sync()
    function sync() {
        const encoded = JSON.stringify(snapshot ? snapshot.results || [] : []);
        if (encoded !== rowsJson) { rowsJson = encoded; rows = snapshot ? snapshot.results || [] : []; }
        const scope = JSON.stringify(snapshot ? snapshot.sources || [] : []);
        if (scope !== sourcesJson) { sourcesJson = scope; sources = snapshot ? snapshot.sources || [] : []; }
    }
    function act(op, row) {
        if (!connected || !currentQuery || !row.actionable || !snapshot) return;
        actionRequested(op, {source: row.track.reference.source, key: row.track.reference.key, search_revision: snapshot.revision});
    }
    Label {
        text: browser.currentQuery ? I18n.message(browser.snapshot.message) : I18n.t("Search query changed · waiting for results")
        color: Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true
    }
    ScrollView {
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(110, scopeList.contentHeight)
        clip: true
        ListView {
            id: scopeList
            model: browser.sources
            delegate: Label {
                required property var modelData
                width: ListView.view.width
                text: modelData.library + " · " + browser.serverName(modelData) + " · " + I18n.state(modelData.phase) + (modelData.has_more ? I18n.t(" · more matches available") : "") + (I18n.message(modelData.error) ? " · " + I18n.message(modelData.error) : "") + (modelData.cache && modelData.cache.origin && modelData.cache.origin !== "live" ? I18n.t(" · cached metadata") : "") + (modelData.cache && modelData.cache.stale ? I18n.t(" · stale") : "") + (modelData.cache && modelData.cache.incomplete ? I18n.t(" · incomplete") : "")
                color: ["failed", "timeout"].includes(modelData.phase) ? Theme.danger : Theme.muted
                textFormat: Text.PlainText; wrapMode: Text.Wrap
            }
        }
    }
    ListView {
        id: results
        objectName: "searchResults"
        Layout.fillWidth: true; Layout.fillHeight: true
        model: browser.rows
        clip: true; spacing: 6
        ScrollBar.vertical: ScrollBar {}
        delegate: Rectangle {
            id: resultRow
            required property var modelData
            required property int index
            width: results.width; height: width >= 640 ? 88 : 124; radius: 5
            color: index % 2 ? Theme.alternate : Theme.surface
            GridLayout {
                anchors.fill: parent; anchors.margins: 10; columns: resultRow.width >= 640 ? 2 : 1; columnSpacing: 12; rowSpacing: 4
                ColumnLayout {
                    Layout.fillWidth: true; spacing: 4
                    Label { text: modelData.track.title; font.bold: true; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Label { text: modelData.track.artist + " · " + modelData.track.album; color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                    Label { text: modelData.library + " · " + browser.serverName(modelData) + (modelData.actionable ? "" : I18n.t(" · awaiting fresh library data")); color: Theme.muted; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                }
                RowLayout {
                    Layout.alignment: resultRow.width >= 640 ? Qt.AlignRight : Qt.AlignLeft
                    enabled: browser.connected && browser.currentQuery && modelData.actionable
                    Button { text: I18n.t("Download original"); visible: browser.canDownload(modelData.track.reference.source); onClicked: browser.act("download_start", modelData) }
                    Button { objectName: "playSearchResult"; text: I18n.t("Play now"); onClicked: browser.act("search_play", modelData) }
                    Button { text: I18n.t("Next"); onClicked: browser.act("search_next", modelData) }
                    Button { text: I18n.t("+ Queue"); onClicked: browser.act("search_enqueue", modelData) }
                }
            }
        }
    }
}
