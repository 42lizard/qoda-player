import QtQuick

QtObject {
    id: view
    required property var output
    readonly property bool fidelity: !!output && !!output.selection && ["fidelity", "fidelity_resampled"].includes(output.selection.mode)
    readonly property string text: output && output.transcoded ? I18n.t("Plex transcoded · %1 · %2", [output.decoder_codec || I18n.t("Audio"), routeText]) : routeText
    readonly property string routeText: {
        if (!output || !output.available) return I18n.t("Output unavailable");
        if (!fidelity) return I18n.t("Desktop · PipeWire");
        if (output.ready === false) return I18n.t("Fidelity · unavailable");
        const plan = output.processing;
        let result;
        if (plan && output.observed) {
            const resampled = plan.source.rate !== plan.target.rate;
            const reduced = plan.source.bits > plan.target.bits || plan.source.sample_type === "float" || plan.source.stereo === false || plan.source.channels !== undefined && plan.source.channels !== 2;
            result = resampled ? I18n.t("Fidelity · converted · SoXR VHQ · %1 → %2 kHz · %3 bit · TPDF", [rate(plan.source.rate), rate(plan.target.rate), I18n.number(plan.target.bits)]) : reduced ? I18n.t("Fidelity · converted · %1 bit · TPDF", [I18n.number(plan.target.bits)]) : I18n.t("Fidelity · exact format");
        } else {
            result = output.selection.mode === "fidelity_resampled" ? I18n.t("Fidelity · HQ fallback enabled") : I18n.t("Fidelity · configured");
        }
        return output.observed && output.observed.stereo_assumed ? I18n.t("%1 · stereo assumed", [result]) : result;
    }
    function rate(value) { return I18n.number(value / 1000, value % 1000 === 0 ? 0 : 1); }
}
