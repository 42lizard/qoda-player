import QtQuick
import QtTest
import "../../ui" as Qoda

TestCase {
    name: "OutputPresentation"
    Qoda.OutputPresentation { id: output; output: null }
    function test_adaptive_output_never_looks_like_desktop_or_sample_perfect() {
        output.output = {available: true, ready: true, selection: {mode: "fidelity_resampled"}};
        verify(output.fidelity); verify(output.text.includes("HQ fallback enabled"));
        output.output = {available: true, ready: true, selection: {mode: "fidelity_resampled"}, observed: {rate: 48000, stereo_assumed: true}, processing: {source: {rate: 44100, bits: 24}, target: {rate: 48000, bits: 16}}};
        verify(output.text.includes("converted")); verify(output.text.includes("SoXR VHQ")); verify(output.text.includes("44.1 → 48")); verify(output.text.includes("TPDF")); verify(output.text.includes("stereo assumed"));
        output.output = {available: true, ready: true, selection: {mode: "fidelity_resampled"}, observed: {rate: 48000}, processing: {source: {rate: 48000, bits: 16}, target: {rate: 48000, bits: 32}}};
        verify(output.text.includes("exact format")); verify(!output.text.includes("converted"));
        output.output = {available: true, ready: false, selection: {mode: "fidelity_resampled"}};
        verify(output.text.includes("unavailable"));
        output.output = {available: true, selection: {mode: "desktop"}};
        verify(!output.fidelity); compare(output.text, "Desktop · PipeWire");
    }
}
