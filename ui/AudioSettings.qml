import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: settings
    required property var snapshot
    required property bool connected
    property var output: null
    property var devices: []
    property string devicesJson: "[]"
    property string selectionJson: "initial"
    property int outputModeDraft: 0
    property string outputDeviceDraft: ""
    readonly property bool hasOutputChange: !!output && !!output.selection && (output.ready === false || (outputModeDraft === 0 ? output.selection.mode !== "desktop" : output.selection.mode !== "fidelity" || output.selection.device !== outputDeviceDraft))
    readonly property bool canApplyOutput: connected && !!output && output.available && hasOutputChange && (outputModeDraft === 0 || output.fidelity_available && devices.some(device => device.id === outputDeviceDraft))
    signal outputRequested(string mode, string device)
    signal refreshOutputsRequested()
    onOutputChanged: syncOutput()
    Component.onCompleted: syncOutput()
    function syncOutput() {
        const encoded = JSON.stringify(output ? output.devices || [] : []);
        if (encoded !== devicesJson) { devicesJson = encoded; devices = output ? output.devices || [] : []; }
        const selection = output ? output.selection : null;
        const key = JSON.stringify(selection);
        if (key !== selectionJson) {
            selectionJson = key;
            outputModeDraft = selection && selection.mode === "fidelity" ? 1 : 0;
            outputDeviceDraft = selection && selection.device ? selection.device : "";
        }
    }
    signal modeRequested(string mode)
    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: childrenRect.height
        spacing: 6
        ComboBox {
            objectName: "outputMode"
            width: Math.min(230, parent.width)
            model: ["Desktop · PipeWire", "Fidelity · direct ALSA"]
            currentIndex: settings.outputModeDraft
            enabled: settings.connected && !!settings.output && settings.output.available
            onActivated: index => settings.outputModeDraft = index
            Accessible.name: "Output mode"
        }
        ComboBox {
            id: outputDevices
            objectName: "outputDevice"
            visible: settings.outputModeDraft === 1
            width: Math.min(340, parent.width)
            model: settings.devices
            textRole: "name"
            currentIndex: settings.devices.findIndex(device => device.id === settings.outputDeviceDraft)
            displayText: currentIndex < 0 ? "Choose a direct ALSA device" : currentText
            enabled: settings.connected && !!settings.output && settings.output.available
            onActivated: index => settings.outputDeviceDraft = settings.devices[index].id
            Accessible.name: "Direct ALSA device"
            contentItem: Text { text: outputDevices.displayText; color: "#e8eee8"; font: outputDevices.font; textFormat: Text.PlainText; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            delegate: ItemDelegate {
                required property var modelData
                width: ListView.view.width
                contentItem: Text { text: modelData.name; color: "#e8eee8"; textFormat: Text.PlainText; elide: Text.ElideRight }
            }
        }
        Button { objectName: "applyOutput"; text: "Apply output"; enabled: settings.canApplyOutput; onClicked: settings.outputRequested(settings.outputModeDraft === 0 ? "desktop" : "fidelity", settings.outputDeviceDraft) }
        Button { text: "Refresh devices"; enabled: settings.connected; onClicked: settings.refreshOutputsRequested() }
    }
    Label { text: settings.output ? settings.output.message : "Output settings unavailable"; color: "#a4b0a7"; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && settings.outputModeDraft === 1 && !settings.output.fidelity_available; text: "Fidelity unavailable: the ALSA submission guard could not be verified."; color: "#f2a89b"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.source; text: settings.output && settings.output.source ? "Source header · " + settings.output.source.rate + " Hz · stereo · " + settings.output.source.bits + " bit" : ""; color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.observed; text: { const observed = settings.output ? settings.output.observed : null; return observed ? "Observed ALSA · " + observed.rate + " Hz · " + observed.format + " · " + observed.valid_bits + " valid bits · " + (observed.stereo_layout ? "FL/FR" : "mapping pending") : ""; } color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.selection && settings.output.selection.mode === "fidelity"; text: "Configured fidelity · final PCM/device qualification pending"; color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
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
