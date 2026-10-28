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
            model: [I18n.t("Desktop · PipeWire"), I18n.t("Fidelity · direct ALSA")]
            currentIndex: settings.outputModeDraft
            enabled: settings.connected && !!settings.output && settings.output.available
            onActivated: index => settings.outputModeDraft = index
            Accessible.name: I18n.t("Output mode")
        }
        ComboBox {
            id: outputDevices
            objectName: "outputDevice"
            visible: settings.outputModeDraft === 1
            width: Math.min(340, parent.width)
            model: settings.devices
            textRole: "name"
            currentIndex: settings.devices.findIndex(device => device.id === settings.outputDeviceDraft)
            displayText: currentIndex < 0 ? I18n.t("Choose a direct ALSA device") : currentText
            enabled: settings.connected && !!settings.output && settings.output.available
            onActivated: index => settings.outputDeviceDraft = settings.devices[index].id
            Accessible.name: I18n.t("Direct ALSA device")
            contentItem: Text { text: outputDevices.displayText; color: Theme.foreground; font: outputDevices.font; textFormat: Text.PlainText; elide: Text.ElideRight; verticalAlignment: Text.AlignVCenter }
            delegate: ItemDelegate {
                required property var modelData
                width: ListView.view.width
                contentItem: Text { text: modelData.name; color: Theme.foreground; textFormat: Text.PlainText; elide: Text.ElideRight }
            }
        }
        Button { objectName: "applyOutput"; text: I18n.t("Apply output"); enabled: settings.canApplyOutput; onClicked: settings.outputRequested(settings.requestedOutputMode, settings.outputDeviceDraft, settings.requestedOutputMode === "fidelity_resampled" && settings.assumeStereoDraft) }
        Button { text: I18n.t("Refresh devices"); enabled: settings.connected; onClicked: settings.refreshOutputsRequested() }
    }
    CheckBox {
        objectName: "allowHqConversion"
        visible: settings.outputModeDraft === 1
        text: I18n.t("Allow HQ conversion when required")
        checked: settings.allowConversionDraft
        enabled: settings.connected && !!settings.output && settings.output.available && settings.output.fidelity_available
        onToggled: settings.allowConversionDraft = checked
    }
    CheckBox {
        objectName: "assumeStereo"
        visible: settings.outputModeDraft === 1 && settings.allowConversionDraft
        text: I18n.t("Assume standard stereo if mapping is unreported")
        checked: settings.assumeStereoDraft
        enabled: settings.connected && !!settings.output && settings.output.available && settings.output.fidelity_available
        onToggled: settings.assumeStereoDraft = checked
        contentItem: Text { text: parent.text; font: parent.font; color: Theme.foreground; wrapMode: Text.Wrap; leftPadding: parent.indicator.width + parent.spacing; verticalAlignment: Text.AlignVCenter }
        Layout.fillWidth: true
    }
    Label {
        visible: settings.outputModeDraft === 1
        text: settings.allowConversionDraft ? I18n.t("Exact format first; otherwise SoXR VHQ rate conversion, dithered PCM conversion and stereo mixing as needed. Converted output is not sample-perfect.") : I18n.t("Strict: refuse required rate, representation, precision or channel conversion.")
        color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true
    }
    Label {
        objectName: "outputProcessing"
        visible: !!settings.output && !!settings.output.processing
        text: {
            const plan = settings.output ? settings.output.processing : null;
            if (!plan) return "";
            const resampled = plan.source.rate !== plan.target.rate;
            const reduced = plan.source.bits > plan.target.bits || plan.source.sample_type === "float" || plan.source.stereo === false || plan.source.channels !== undefined && plan.source.channels !== 2;
            return I18n.t("Processing plan · %1 → %2 Hz · %3 → %4 bit · %5%6%7", [I18n.number(plan.source.rate), I18n.number(plan.target.rate), I18n.number(plan.source.bits), I18n.number(plan.target.bits), resampled ? "SoXR VHQ" : I18n.t("exact rate"), resampled || reduced ? I18n.t(" · TPDF dither") : "", resampled || reduced ? I18n.t(" · converted") : I18n.t(" · no format conversion")]);
        }
        color: Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true
    }
    Label {
        objectName: "outputContinuity"
        visible: !!settings.output && (!!settings.output.continuous || !!settings.output.chain_fallback)
        text: settings.output ? I18n.message(settings.output.chain_fallback) || I18n.t("Continuous conversion · compatible tracks share one converter") : ""
        color: Theme.muted; wrapMode: Text.Wrap; textFormat: Text.PlainText; Layout.fillWidth: true
    }
    Label { text: settings.output ? I18n.message(settings.output.message) || "" : I18n.t("Output settings unavailable"); color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && settings.outputModeDraft === 1 && !settings.output.fidelity_available; text: I18n.t("Fidelity unavailable: the ALSA submission guard could not be verified."); color: Theme.danger; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.transcoded; text: settings.output ? I18n.t("Plex server transcoding · %1 · converted representation", [settings.output.decoder_codec || I18n.t("codec pending")]) : ""; color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.source; text: settings.output && settings.output.source ? I18n.t("Decoded PCM · %1 Hz · %2 channels%3 · %4 bit %5", [I18n.number(settings.output.source.rate), I18n.number(settings.output.source.channels || 2), settings.output.source_layout ? " (" + settings.output.source_layout + ")" : "", I18n.number(settings.output.source.bits), I18n.state(settings.output.source.sample_type || "integer")]) : ""; color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.observed; text: { const observed = settings.output ? settings.output.observed : null; return observed ? I18n.t("Observed ALSA · %1 Hz · %2 · %3 valid bits · %4", [I18n.number(observed.rate), observed.format, I18n.number(observed.valid_bits), observed.stereo_layout ? I18n.t("FL/FR reported") : observed.stereo_assumed ? I18n.t("stereo assumed; unverified") : I18n.t("mapping pending")]) : ""; } color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { visible: !!settings.output && !!settings.output.selection && ["fidelity", "fidelity_resampled"].includes(settings.output.selection.mode); text: I18n.t("Configured fidelity · final PCM/device qualification pending"); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    RowLayout {
        Label { text: "ReplayGain"; color: Theme.foreground; font.bold: true }
        ComboBox {
            objectName: "replaygainMode"
            model: [I18n.t("Off"), I18n.t("Track"), I18n.t("Album"), I18n.t("Auto")]
            currentIndex: settings.snapshot ? ["off", "track", "album", "auto"].indexOf(settings.snapshot.mode) : 3
            enabled: settings.connected && !!settings.snapshot && settings.snapshot.available
            onActivated: index => settings.modeRequested(["off", "track", "album", "auto"][index])
            Accessible.name: I18n.t("ReplayGain mode")
        }
        Label { text: I18n.t("Preamp 0 dB · clipping prevention"); color: Theme.muted; visible: settings.width > 500 }
    }
    Label { text: I18n.t("Auto uses album gain for unshuffled album sessions; otherwise track gain. Missing album gain falls back to track, then unity."); color: Theme.muted; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: settings.snapshot ? I18n.message(settings.snapshot.message) : I18n.t("Audio settings unavailable"); color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
    Label { text: settings.snapshot ? I18n.message(settings.snapshot.metadata_scope) : ""; color: Theme.muted; textFormat: Text.PlainText; wrapMode: Text.Wrap; Layout.fillWidth: true }
}
