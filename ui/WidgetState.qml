import QtQuick

BackendState {
    allowedOperations: ["previous", "next", "pause", "resume", "quit"]
    queueFenced: true
    readonly property var selectedTrack: (snapshot.queue && snapshot.queue.entries || []).find(row => row.selected)?.track || snapshot.selection || null
    readonly property bool playing: !!snapshot.presentation && !!snapshot.presentation.known && snapshot.presentation.status === "Playing"
}
