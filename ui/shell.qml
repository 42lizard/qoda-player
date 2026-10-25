//@ pragma AppId qoda-player
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io

ShellRoot {
    id: root
    property var snapshot: ({libraries: [], status: "Connecting backend", intent: "paused"})
    property var libraryModel: []
    property var trackModel: []
    property string libraryJson: "[]"
    property string trackJson: "[]"
    property string libraryId: ""
    property string instance: ""
    property int serial: 0
    property string nonce: Date.now().toString(36)
    function send(op, fields) {
        if (!socket.connected || !instance) return;
        let data = fields || {};
        data.op = op;
        data.instance = instance;
        data.id = nonce + "-" + (++serial);
        socket.write(JSON.stringify(data) + "\n");
        socket.flush();
    }
    function pageOffset(value, delta) {
        // Decimal strings preserve the socket's u64 contract without JS Number rounding.
        let digits = value.split("");
        let carry = delta;
        for (let i = digits.length - 1; i >= 0; --i) {
            let n = Number(digits[i]) + carry;
            digits[i] = String((n % 10 + 10) % 10);
            carry = Math.floor(n / 10);
        }
        if (carry < 0) return "0";
        while (carry > 0) { digits.unshift(String(carry % 10)); carry = Math.floor(carry / 10); }
        return digits.join("").replace(/^0+(?=.)/, "");
    }
    function browse(offset) {
        if (libraryId) send("browse", {source: libraryId, offset: offset || "0", query: search.text});
    }
    Process { id: browser }
    Socket {
        id: socket
        path: Quickshell.env("XDG_RUNTIME_DIR") + "/qoda/backend.sock"
        connected: true
        onConnectedChanged: {
            if (connected) { root.instance = ""; write('{"op":"hello","protocol":1}\n'); flush(); }
        }
        parser: SplitParser {
            onRead: data => {
                try {
                    let next = JSON.parse(data);
                    if (next.protocol !== 1 || !next.instance) return;
                    if (root.instance === next.instance) {
                        let a = next.revision, b = root.snapshot.revision || "0";
                        if (a.length < b.length || (a.length === b.length && a < b)) return;
                    }
                    if (!root.instance && next.page) {
                        root.libraryId = next.page.source;
                        search.text = next.page.query;
                    }
                    root.instance = next.instance;
                    let libraries = JSON.stringify(next.libraries || []);
                    if (libraries !== root.libraryJson) { root.libraryJson = libraries; root.libraryModel = next.libraries || []; }
                    let tracks = JSON.stringify(next.page ? next.page.tracks : []);
                    if (tracks !== root.trackJson) { root.trackJson = tracks; root.trackModel = next.page ? next.page.tracks : []; }
                    root.snapshot = next;
                } catch (e) { }
            }
        }
    }
    Timer { interval: 1000; repeat: true; running: !socket.connected; onTriggered: socket.connected = true }
    Timer { id: searchDelay; interval: 300; onTriggered: root.browse("0") }
    FloatingWindow {
        id: window
        title: "Qoda Player"
        visible: true
        implicitWidth: 1000
        implicitHeight: 700
        color: "#141820"
        onClosed: visible = false

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 22
            spacing: 14
            RowLayout {
                Label { text: "Qoda"; color: "#f2e9db"; font.pixelSize: 28; font.bold: true }
                Label { text: "Plex music"; color: "#acb8c8"; Layout.fillWidth: true }
                Button { text: "Connect Plex (this session)"; enabled: socket.connected; onClicked: root.send("auth_start") }
                Button { text: "Disconnect"; onClicked: root.send("disconnect") }
                Button { text: "Hide"; onClicked: window.visible = false }
                Button { text: "Quit"; onClicked: { root.send("quit"); Qt.quit(); } }
            }
            Label { text: socket.connected ? root.snapshot.status : "Backend disconnected — reconnecting"; color: "#acb8c8"; textFormat: Text.PlainText }
            Label { visible: !!root.snapshot.error; text: root.snapshot.error || ""; color: "#f2a89b"; wrapMode: Text.Wrap; Layout.fillWidth: true; textFormat: Text.PlainText }
            RowLayout {
                visible: !!root.snapshot.signin_url
                Button { text: "Open Plex sign-in"; onClicked: { browser.command = ["/usr/bin/brave", root.snapshot.signin_url]; browser.running = true; } }
                TextField { text: root.snapshot.signin_url || ""; readOnly: true; selectByMouse: true; Layout.fillWidth: true }
            }
            RowLayout {
                ComboBox {
                    currentIndex: root.libraryModel.findIndex(library => library.id === root.libraryId)
                    Layout.preferredWidth: 300
                    model: root.libraryModel
                    textRole: "title"
                    onActivated: index => { root.libraryId = model[index].id; root.browse("0"); }
                    displayText: root.libraryId ? currentText : "Choose a music library"
                }
                TextField { id: search; Layout.fillWidth: true; placeholderText: "Search this library"; onTextEdited: searchDelay.restart(); onAccepted: { searchDelay.stop(); root.browse("0"); } }
            }
            ListView {
                id: tracks
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 5
                model: root.trackModel
                ScrollBar.vertical: ScrollBar {}
                delegate: Rectangle {
                    required property var modelData
                    required property int index
                    width: tracks.width
                    height: 68
                    color: index % 2 ? "#1a202a" : "#202735"
                    radius: 5
                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: 10
                        ColumnLayout {
                            Layout.fillWidth: true
                            Label { text: modelData.title; color: "#f2e9db"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                            Label { text: modelData.artist + " · " + modelData.album; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                        }
                        Button { text: "Play"; onClicked: root.send("play", {source: modelData.reference.source, key: modelData.reference.key}) }
                    }
                }
            }
            RowLayout {
                Label { text: root.snapshot.page ? root.snapshot.page.tracks.length + " tracks on this page" : "Choose a library to browse"; color: "#acb8c8"; Layout.fillWidth: true }
                Button { text: "Previous"; enabled: !!root.snapshot.page && root.snapshot.page.offset !== "0"; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, -100)) }
                Button { text: "Next"; enabled: !!root.snapshot.page && root.snapshot.page.has_more; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, 100)) }
            }
            Rectangle { Layout.fillWidth: true; height: 1; color: "#394355" }
            Label { text: root.snapshot.selection ? root.snapshot.selection.title + " · " + root.snapshot.selection.artist : "No track selected"; color: "#f2e9db"; textFormat: Text.PlainText }
            RowLayout {
                Button { text: root.snapshot.intent === "playing" ? "Pause" : "Resume"; enabled: !!root.snapshot.selection; onClicked: root.send(root.snapshot.intent === "playing" ? "pause" : "resume") }
                Label { text: "Seek (seconds)"; color: "#acb8c8" }
                TextField { id: seek; placeholderText: "0"; Layout.preferredWidth: 90; validator: DoubleValidator { bottom: 0 } onAccepted: root.send("seek", {seconds: Number(text)}) }
                Button { text: "Seek"; enabled: !!root.snapshot.selection && seek.acceptableInput; onClicked: root.send("seek", {seconds: Number(seek.text)}) }
                Item { Layout.fillWidth: true }
                Label { text: (root.snapshot.audio && root.snapshot.audio.position_seconds !== null ? root.snapshot.audio.position_seconds.toFixed(1) + " s · " : "") + "Desktop / PipeWire · ReplayGain off"; color: "#acb8c8" }
            }
        }
    }
    IpcHandler {
        target: "control"
        function show(): void { window.visible = true;  }
        function hide(): void { window.visible = false; }
        function status(): string { return JSON.stringify({connected: socket.connected, instance: root.instance, libraries: (root.snapshot.libraries || []).length, tracks: root.snapshot.page ? root.snapshot.page.tracks.length : 0, visible: window.visible}); }
        function quit(): void { root.send("quit"); Qt.quit(); }
    }
}
