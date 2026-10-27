import QtQuick

BackendConnection {
    session: WidgetState { id: widget }
    readonly property var selectedTrack: widget.selectedTrack
    readonly property bool playing: widget.playing
}
