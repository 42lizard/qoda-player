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
    function test_float_conversion_and_server_transcoding_are_visible() {
        output.output = {available: true, ready: true, transcoded: true, decoder_codec: "flac", selection: {mode: "fidelity_resampled"}, observed: {rate: 48000}, processing: {source: {rate: 48000, bits: 32, sample_type: "float", channels: 2, stereo: true}, target: {rate: 48000, bits: 32}}};
        verify(output.text.includes("Plex transcoded")); verify(output.text.includes("flac"));
        verify(output.text.includes("converted")); verify(output.text.includes("TPDF"));
        verify(!output.text.includes("exact format"));
        output.output = {available: true, transcoded: true, decoder_codec: "mp3", selection: {mode: "desktop"}};
        verify(output.text.includes("Plex transcoded")); verify(output.text.includes("mp3")); verify(output.text.includes("Desktop"));
    }

}
