import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: settings
    required property var snapshot
    required property bool connected
    signal modeRequested(string mode)
    RowLayout {
        Label { text: "ReplayGain"; color: "#e8eee8"; font.bold: true }
        ComboBox {
            objectName: "replaygainMode"
            model: ["Off", "Track", "Album", "Auto"]
            currentIndex: settings.snapshot ? ["off", "track", "album", "auto"].indexOf(settings.snapshot.mode) : 3
            enabled: settings.connected && !!settings.snapshot && settings.snapshot.available
            onActivated: index => settings.modeRequested(["off", "track", "album", "auto"][index])
            Accessible.name: "ReplayGain mode"
        }
        Label { text: "Preamp 0 dB · clipping prevention"; color: "#a4b0a7"; visible: settings.width > 500 }
    }
    Label { text: "Auto uses album gain for unshuffled album sessions; otherwise track gain. Missing album gain falls back to track, then unity."; color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: settings.snapshot ? settings.snapshot.message : "Audio settings unavailable"; color: "#a4b0a7"; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: settings.snapshot ? settings.snapshot.metadata_scope : ""; color: "#a4b0a7"; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
}
