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
        text: controls.snapshot ? I18n.message(controls.snapshot.message) : ""
        color: Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText
    }
    Button {
        objectName: "retryDesktopControls"
        text: controls.snapshot && controls.snapshot.state === "connecting" ? I18n.t("Connecting desktop controls…") : I18n.t("Retry desktop controls")
        enabled: controls.connected && !!controls.snapshot && controls.snapshot.state === "unavailable"
        onClicked: controls.retryRequested()
    }
}
