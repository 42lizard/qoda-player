import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: notice
    property var snapshot: null
    property bool canReconnect: false
    property bool busy: false
    signal reconnectRequested()
    signal signInRequested()
    visible: !!snapshot && ["offline", "partial"].includes(snapshot.origin)
    spacing: 6
    Label { text: notice.snapshot ? notice.snapshot.message : ""; color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Button {
        objectName: "discoveryConnect"
        text: notice.busy ? "Connecting…" : notice.canReconnect ? "Reconnect Plex" : "Connect / restore sign-in"
        enabled: !notice.busy
        onClicked: notice.canReconnect ? notice.reconnectRequested() : notice.signInRequested()
    }
}
