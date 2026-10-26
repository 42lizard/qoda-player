import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: controls
    required property var snapshot
    property bool connected: false
    signal retryRequested()
    visible: !!snapshot && snapshot.state !== "available"
    Label {
        Layout.fillWidth: true
        text: controls.snapshot ? controls.snapshot.message : ""
        color: "#a4b0a7"; wrapMode: Text.Wrap; textFormat: Text.PlainText
    }
    Button {
        objectName: "retryDesktopControls"
        text: controls.snapshot && controls.snapshot.state === "connecting" ? "Connecting desktop controls…" : "Retry desktop controls"
        enabled: controls.connected && !!controls.snapshot && controls.snapshot.state === "unavailable"
        onClicked: controls.retryRequested()
    }
}
