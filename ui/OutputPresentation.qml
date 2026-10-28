import QtQuick

QtObject {
    id: view
    required property var output
    readonly property bool fidelity: !!output && !!output.selection && ["fidelity", "fidelity_resampled"].includes(output.selection.mode)
    readonly property string text: (output && output.transcoded ? "Plex transcoded · " + (output.decoder_codec || "audio") + " · " : "") + routeText
    readonly property string routeText: {
        if (!output || !output.available) return "Output unavailable";
        if (!fidelity) return "Desktop · PipeWire";
        if (output.ready === false) return "Fidelity · unavailable";
        const plan = output.processing;
        const stereo = output.observed && output.observed.stereo_assumed ? " · stereo assumed" : "";
        if (plan && output.observed) {
            const resampled = plan.source.rate !== plan.target.rate;
            const reduced = plan.source.bits > plan.target.bits || plan.source.sample_type === "float" || plan.source.stereo === false || plan.source.channels !== undefined && plan.source.channels !== 2;
            if (resampled || reduced) return "Fidelity · converted · " + (resampled ? "SoXR VHQ · " + rate(plan.source.rate) + " → " + rate(plan.target.rate) + " kHz · " : "") + plan.target.bits + " bit · TPDF" + stereo;
            return "Fidelity · exact format" + stereo;
        }
        return output.selection.mode === "fidelity_resampled" ? "Fidelity · HQ fallback enabled" : "Fidelity · configured";
    }
    function rate(value) { return value % 1000 === 0 ? String(value / 1000) : (value / 1000).toFixed(1); }
}
