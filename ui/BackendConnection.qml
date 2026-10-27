import QtQuick
import Quickshell
import Quickshell.Io

Scope {
    id: client
    property string path: Quickshell.env("XDG_RUNTIME_DIR") + "/qoda/backend.sock"
    property string role: ""
    property BackendState session: BackendState {}
    readonly property bool connected: socket.connected && !!session.snapshot.instance
    readonly property var snapshot: session.snapshot
    signal received(var value)
    signal reset()
    function send(op, fields, requiresQueue) {
        if (!connected) return;
        const request = session.request(op, fields, requiresQueue);
        if (request) { socket.write(JSON.stringify(request) + "\n"); socket.flush(); }
    }
    Socket {
        id: socket
        path: client.path
        connected: true
        onConnectedChanged: {
            session.reset(); client.reset();
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
                    const previous = session.snapshot;
                    if (!session.accept(JSON.parse(line))) { socket.connected = false; return; }
                    if (session.snapshot !== previous) client.received(session.snapshot);
                } catch (_) { socket.connected = false; }
            }
        }
    }
    Timer { interval: 1000; repeat: true; running: !socket.connected; onTriggered: socket.connected = true }
}
