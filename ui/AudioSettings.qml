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
    property bool allowConversionDraft: false
    property bool assumeStereoDraft: false
    readonly property string requestedOutputMode: outputModeDraft === 0 ? "desktop" : allowConversionDraft ? "fidelity_resampled" : "fidelity"
    property string outputDeviceDraft: ""
    readonly property bool hasOutputChange: !!output && !!output.selection && (output.ready === false || output.selection.mode !== requestedOutputMode || outputModeDraft === 1 && output.selection.device !== outputDeviceDraft || requestedOutputMode === "fidelity_resampled" && !!output.selection.assume_stereo !== assumeStereoDraft)
    readonly property bool canApplyOutput: connected && !!output && output.available && hasOutputChange && (outputModeDraft === 0 || output.fidelity_available && devices.some(device => device.id === outputDeviceDraft))
    signal outputRequested(string mode, string device, bool assumeStereo)
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
            outputModeDraft = selection && ["fidelity", "fidelity_resampled"].includes(selection.mode) ? 1 : 0;
            allowConversionDraft = !!selection && selection.mode === "fidelity_resampled";
            assumeStereoDraft = !!selection && !!selection.assume_stereo;
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
        Button { objectName: "applyOutput"; text: "Apply output"; enabled: settings.canApplyOutput; onClicked: settings.outputRequested(settings.requestedOutputMode, settings.outputDeviceDraft, settings.requestedOutputMode === "fidelity_resampled" && settings.assumeStereoDraft) }
        Button { text: "Refresh devices"; enabled: settings.connected; onClicked: settings.refreshOutputsRequested() }
    }
    CheckBox {
        objectName: "allowHqConversion"
        visible: settings.outputModeDraft === 1
        text: "Allow HQ conversion when required"
        checked: settings.allowConversionDraft
        enabled: settings.connected && !!settings.output && settings.output.available && settings.output.fidelity_available
        onToggled: settings.allowConversionDraft = checked
    }
    CheckBox {
        objectName: "assumeStereo"
        visible: settings.outputModeDraft === 1 && settings.allowConversionDraft
        text: "Assume standard stereo if mapping is unreported"
        checked: settings.assumeStereoDraft
        enabled: settings.connected && !!settings.output && settings.output.available && settings.output.fidelity_available
        onToggled: settings.assumeStereoDraft = checked
        contentItem: Text { text: parent.text; font: parent.font; color: "#e8eee8"; wrapMode: Text.Wrap; leftPadding: parent.indicator.width + parent.spacing; verticalAlignment: Text.AlignVCenter }
        Layout.fillWidth: true
    }
    Label {
        visible: settings.outputModeDraft === 1
        text: settings.allowConversionDraft ? "Exact format first; otherwise SoXR VHQ resampling and dithered precision reduction. Converted output is not sample-perfect." : "Strict: refuse unsupported rates and precision."
        color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true
    }
    Label {
        objectName: "outputProcessing"
        visible: !!settings.output && !!settings.output.processing
        text: {
            const plan = settings.output ? settings.output.processing : null;
            if (!plan) return "";
            const resampled = plan.source.rate !== plan.target.rate;
            const reduced = plan.source.bits > plan.target.bits;
            return "Processing plan · " + plan.source.rate + " → " + plan.target.rate + " Hz · " + plan.source.bits + " → " + plan.target.bits + " bit · " + (resampled ? "SoXR VHQ" : "exact rate") + (resampled || reduced ? " · TPDF dither" : "") + (resampled || reduced ? " · converted" : " · no format conversion");
        }
        color: "#a4b0a7"; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true
    }
    Label { text: settings.output ? settings.output.message || "" : "Output settings unavailable"; color: "#a4b0a7"; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && settings.outputModeDraft === 1 && !settings.output.fidelity_available; text: "Fidelity unavailable: the ALSA submission guard could not be verified."; color: "#f2a89b"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.source; text: settings.output && settings.output.source ? "Source header · " + settings.output.source.rate + " Hz · stereo · " + settings.output.source.bits + " bit" : ""; color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.observed; text: { const observed = settings.output ? settings.output.observed : null; return observed ? "Observed ALSA · " + observed.rate + " Hz · " + observed.format + " · " + observed.valid_bits + " valid bits · " + (observed.stereo_layout ? "FL/FR reported" : observed.stereo_assumed ? "stereo assumed; unverified" : "mapping pending") : ""; } color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.selection && ["fidelity", "fidelity_resampled"].includes(settings.output.selection.mode); text: "Configured fidelity · final PCM/device qualification pending"; color: "#a4b0a7"; wrapMode: Text.Wrap; Layout.fillWidth: true }
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
