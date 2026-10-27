import QtQuick
import Quickshell
import Quickshell.Io

Scope {
    id: client
    property string path: Quickshell.env("XDG_RUNTIME_DIR") + "/qoda/backend.sock"
    property string role: ""
    property alias allowedOperations: state.allowedOperations
    property alias queueFenced: state.queueFenced
    readonly property bool connected: socket.connected && !!state.snapshot.instance
    readonly property var snapshot: state.snapshot
    signal received(var value)
    signal reset()
    function send(op, fields, requiresQueue) {
        if (!connected) return;
        const request = state.request(op, fields, requiresQueue);
        if (request) { socket.write(JSON.stringify(request) + "\n"); socket.flush(); }
    }
    BackendState { id: state }
    Socket {
        id: socket
        path: client.path
        connected: true
        onConnectedChanged: {
            state.reset(); client.reset();
            if (connected) {
                let hello = {op: "hello", protocol: 1};
                if (client.role) hello.role = client.role;
                write(JSON.stringify(hello) + "\n"); flush();
            }
        }
        parser: SplitParser {
            onRead: line => {
                try {
                    // Bounds complete lines only; SplitParser owns unfinished buffering.
                    if (line.length > 16 * 1024 * 1024) { socket.connected = false; return; }
                    const previous = state.snapshot;
                    if (!state.accept(JSON.parse(line))) { socket.connected = false; return; }
                    if (state.snapshot !== previous) client.received(state.snapshot);
                } catch (_) { socket.connected = false; }
            }
        }
    }
    Timer { interval: 1000; repeat: true; running: !socket.connected; onTriggered: socket.connected = true }
}
