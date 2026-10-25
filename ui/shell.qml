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
    property var queueModel: []
    property string queueJson: "[]"
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
    function editQueue(op, fields) {
        if (!snapshot.queue) return;
        let data = fields || {};
        data.queue_session = snapshot.queue.session;
        data.queue_revision = snapshot.queue.revision;
        send(op, data);
    }
    function moveEntry(index, delta) {
        let target = index + delta;
        if (target < 0 || target >= queueModel.length || !queueModel[index].can_move || !queueModel[target].can_move) return;
        editQueue("queue_move", {occurrence: queueModel[index].id, before: delta < 0 ? queueModel[target].id : (target + 1 < queueModel.length ? queueModel[target + 1].id : null)});
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
                    if (!next.libraries || next.libraries.length === 0) { root.libraryId = ""; search.text = ""; }
                    root.instance = next.instance;
                    let libraries = JSON.stringify(next.libraries || []);
                    if (libraries !== root.libraryJson) { root.libraryJson = libraries; root.libraryModel = next.libraries || []; }
                    let tracks = JSON.stringify(next.page ? next.page.tracks : []);
                    if (tracks !== root.trackJson) { root.trackJson = tracks; root.trackModel = next.page ? next.page.tracks : []; }
                    let queue = JSON.stringify(next.queue ? next.queue.entries : []);
                    if (queue !== root.queueJson) { root.queueJson = queue; root.queueModel = next.queue ? next.queue.entries : []; }
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
        implicitWidth: 1200
        implicitHeight: 760
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
                Button { text: "Disconnect"; ToolTip.visible: hovered; ToolTip.text: "Stops playback and removes the saved queue and session backups. Keeps saved sign-in."; onClicked: root.send("disconnect") }
                Button { text: "Hide"; onClicked: window.visible = false }
                Button { text: "Quit"; onClicked: { root.send("quit"); Qt.quit(); } }
            }
            RowLayout {
                Button { text: "Restore saved sign-in"; enabled: socket.connected && !!root.snapshot.sign_in && !root.snapshot.sign_in.connected && !root.snapshot.sign_in.keyring_action; onClicked: root.send("restore_signin") }
                Button { text: "Save sign-in"; enabled: socket.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.can_save && !root.snapshot.sign_in.keyring_action; onClicked: root.send("save_signin") }
                Button { text: "Sign out & forget"; ToolTip.visible: hovered; ToolTip.text: "Stops playback and removes Qoda sign-in, saved queue and session backups. Keeps general preferences."; enabled: socket.connected && !!root.snapshot.sign_in && root.snapshot.sign_in.keyring_action !== "forget"; onClicked: root.send("forget_signin") }
                Label { text: root.snapshot.sign_in ? root.snapshot.sign_in.message || "" : ""; color: "#acb8c8"; Layout.fillWidth: true; wrapMode: Text.Wrap; textFormat: Text.PlainText }
            }
            RowLayout {
                Label { text: root.snapshot.storage ? root.snapshot.storage.message : "Opening session storage…"; color: root.snapshot.storage && (root.snapshot.storage.state === "unsaved" || root.snapshot.storage.state === "blocked") ? "#f2a89b" : "#acb8c8"; Layout.fillWidth: true; wrapMode: Text.Wrap; textFormat: Text.PlainText }
                Button { text: "Retry save"; enabled: socket.connected; onClicked: root.send("retry_save") }
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
            RowLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 16
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
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
                        Button { text: "Play now"; onClicked: root.send("play", {source: modelData.reference.source, key: modelData.reference.key}) }
                        Button { text: "Next"; onClicked: root.editQueue("play_next", {source: modelData.reference.source, key: modelData.reference.key}) }
                        Button { text: "+ Queue"; onClicked: root.editQueue("enqueue", {source: modelData.reference.source, key: modelData.reference.key}) }
                    }
                }
            }
            RowLayout {
                Label { text: root.snapshot.page ? root.snapshot.page.tracks.length + " tracks on this page" : "Choose a library to browse"; color: "#acb8c8"; Layout.fillWidth: true }
                Button { text: "Previous"; enabled: !!root.snapshot.page && root.snapshot.page.offset !== "0"; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, -100)) }
                Button { text: "Next"; enabled: !!root.snapshot.page && root.snapshot.page.has_more; onClicked: root.browse(root.pageOffset(root.snapshot.page.offset, 100)) }
            }
                }
                Rectangle { width: 1; Layout.fillHeight: true; color: "#394355" }
                ColumnLayout {
                    Layout.preferredWidth: 340
                    Layout.fillHeight: true
                    Label { text: "Queue · " + root.queueModel.length; color: "#f2e9db"; font.pixelSize: 20; font.bold: true }
                    RowLayout {
                Button { text: "Shuffle: " + (root.snapshot.queue && root.snapshot.queue.shuffled ? "On" : "Off"); enabled: !!root.snapshot.queue; onClicked: root.editQueue("set_shuffle", {enabled: !root.snapshot.queue.shuffled}) }
                Button { text: "Repeat: " + (root.snapshot.queue ? root.snapshot.queue.repeat : "off"); enabled: !!root.snapshot.queue; onClicked: { let modes = ["off", "one", "all"]; root.editQueue("set_repeat", {mode: modes[(modes.indexOf(root.snapshot.queue.repeat) + 1) % 3]}); } }
                    }
                    Label { text: root.snapshot.queue && root.snapshot.queue.navigating ? "Previous navigation · Next returns forward" : root.snapshot.queue && root.snapshot.queue.ended ? "End of selection · Play pending or Replay" : "Selected and upcoming"; color: "#acb8c8"; textFormat: Text.PlainText }
                    ListView {
                        id: queueView
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        model: root.queueModel
                        clip: true
                        spacing: 6
                        ScrollBar.vertical: ScrollBar {}
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: queueView.width
                            height: 108
                            radius: 5
                            color: modelData.selected ? "#344354" : "#202735"
                            ColumnLayout {
                                anchors.fill: parent
                                anchors.margins: 8
                                spacing: 3
                                Label { text: (modelData.selected ? "Selected · " : "") + modelData.track.title; color: "#f2e9db"; font.bold: true; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                Label { text: modelData.track.artist; color: "#acb8c8"; textFormat: Text.PlainText; elide: Text.ElideRight; Layout.fillWidth: true }
                                RowLayout {
                                    Button { text: "Play"; enabled: !!root.snapshot.sign_in && root.snapshot.sign_in.connected; onClicked: root.editQueue("queue_play", {occurrence: modelData.id}) }
                                    Button { text: "↑"; enabled: modelData.can_move && index > 0 && root.queueModel[index - 1].can_move; onClicked: root.moveEntry(index, -1) }
                                    Button { text: "↓"; enabled: modelData.can_move && index + 1 < root.queueModel.length && root.queueModel[index + 1].can_move; onClicked: root.moveEntry(index, 1) }
                                    Button { text: "Remove"; onClicked: root.editQueue("queue_remove", {occurrence: modelData.id}) }
                                }
                            }
                        }
                    }
                    RowLayout {
                        Button { text: "Clear upcoming"; onClicked: root.editQueue("clear_upcoming") }
                        Button { text: "Stop & clear"; onClicked: root.editQueue("stop_clear") }
                    }
                    Label { visible: !!root.snapshot.queue && root.snapshot.queue.navigation_dropped !== "0"; text: "Previous retains the latest 4096 nominal visits; earlier visits were omitted."; color: "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true }
                    Label { text: "Queue checkpoints restore paused after backend restart."; color: "#acb8c8"; wrapMode: Text.Wrap; Layout.fillWidth: true }
                }
            }
            Rectangle { Layout.fillWidth: true; height: 1; color: "#394355" }
            Label { text: root.snapshot.selection ? root.snapshot.selection.title + " · " + root.snapshot.selection.artist : "No track selected"; color: "#f2e9db"; textFormat: Text.PlainText }
            RowLayout {
                Button { text: root.snapshot.intent === "playing" ? "Pause" : "Resume"; enabled: root.queueModel.length > 0 && !!root.snapshot.sign_in && root.snapshot.sign_in.connected; onClicked: root.send(root.snapshot.intent === "playing" ? "pause" : "resume") }
                Button { text: "Previous"; enabled: !!root.snapshot.queue && root.snapshot.queue.previous_available; onClicked: root.editQueue("previous") }
                Button { text: "Next track"; enabled: root.queueModel.length > 0; onClicked: root.editQueue("next") }
                Button { text: "Replay"; enabled: !!root.snapshot.queue && root.snapshot.queue.membership_count > 0; onClicked: root.editQueue("replay") }
                Label { text: "Seek (seconds)"; color: "#acb8c8" }
                TextField { id: seek; placeholderText: "0"; Layout.preferredWidth: 90; validator: DoubleValidator { bottom: 0; top: 1000000000; locale: "C" } onAccepted: root.send("seek", {seconds: Number(text)}) }
                Button { text: "Seek"; enabled: !!root.snapshot.queue && !!root.snapshot.queue.selected && seek.acceptableInput; onClicked: root.send("seek", {seconds: Number(seek.text)}) }
                Item { Layout.fillWidth: true }
                Label { text: (root.snapshot.audio && root.snapshot.audio.position_seconds !== null ? root.snapshot.audio.position_seconds.toFixed(1) + " s · " : root.snapshot.queue && root.snapshot.queue.selected ? root.snapshot.queue.position_seconds.toFixed(1) + " s local · " : "") + "Desktop / PipeWire · ReplayGain off"; color: "#acb8c8" }
            }
        }
    }
    IpcHandler {
        target: "control"
        function show(): void { window.visible = true;  }
        function hide(): void { window.visible = false; }
        function status(): string { return JSON.stringify({connected: socket.connected, instance: root.instance, storage: root.snapshot.storage || null, queue: root.snapshot.queue ? {shuffled: root.snapshot.queue.shuffled, repeat: root.snapshot.queue.repeat, previousAvailable: root.snapshot.queue.previous_available, position: root.snapshot.queue.position_seconds, session: root.snapshot.queue.session, revision: root.snapshot.queue.revision, entries: root.snapshot.queue.entries.map(row => ({id: row.id, selected: row.selected, key: row.track.reference.key})), ended: root.snapshot.queue.ended} : null, signIn: root.snapshot.sign_in || null, libraries: (root.snapshot.libraries || []).length, tracks: root.snapshot.page ? root.snapshot.page.tracks.length : 0, visible: window.visible}); }
        function quit(): void { root.send("quit"); Qt.quit(); }
    }
}
