import QtQuick
import Quickshell
import Quickshell.Io

Scope {
    id: client
    property string path: Quickshell.env("XDG_RUNTIME_DIR") + "/qoda/backend.sock"
    readonly property bool connected: socket.connected && !!state.snapshot.instance
    readonly property bool playing: state.playing
    readonly property var snapshot: state.snapshot
    readonly property var selectedTrack: state.selectedTrack
    function send(op) {
        if (!connected) return;
        const request = state.request(op);
        if (request) { socket.write(JSON.stringify(request) + "\n"); socket.flush(); }
    }
    WidgetState { id: state }
    Socket {
        id: socket
        path: client.path
        connected: true
        onConnectedChanged: {
            state.reset();
            // Subscribe as a non-raising client: the browser owns desktop Raise.
            if (connected) { write('{"op":"hello","protocol":1}\n'); flush(); }
        }
        parser: SplitParser {
            onRead: line => {
                try {
                    if (line.length > 16 * 1024 * 1024 || !state.accept(JSON.parse(line))) socket.connected = false;
                } catch (_) { socket.connected = false; }
            }
        }
    }
    Timer { interval: 1000; repeat: true; running: !socket.connected; onTriggered: socket.connected = true }
}
