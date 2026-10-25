# Audio feasibility — throwaway embedded-engine experiment

Question: what player-side sample-preservation and asynchronous handoff behavior can the installed libmpv actually demonstrate with known PCM served over controllable HTTP?

The human selected **player-side digital capture first, leaving hardware fidelity unverified**. This harness uses only raw PCM file output and a timed null output. It does not open speakers, microphones, PipeWire monitors, ALSA hardware, a Plex account, or any desktop configuration.

## Run and inspect

Requirements: Linux with `libmpv`, `mpv`, `ffmpeg`, and Python's standard library. The tested package/API versions are recorded in the evidence.

```sh
python3 prototypes/audio-feasibility/run.py
```

The command creates deterministic synthetic fixtures, starts a loopback-only HTTP server with byte ranges and controlled response delays, drives **libmpv directly through its C API**, and produces:

- [Measurement summary](evidence/summary.md).
- [Full measurements and event traces](evidence/results.json).
- [Self-contained case inspector](evidence/report.html), opened directly in a browser.

Raw fixtures, captures and per-case working traces go into a new ignored `.scratch/audio-prototype-*` directory. The small report/evidence files live alongside the harness. Re-running replaces the recorded report with that run's evidence; no application state is used or persisted. No fixture audio is sent to a real output device.

## Measurement boundary

Known signed-integer PCM is generated independently by sample index, split into two 88,201-frame segments, and encoded as WAV/FLAC. Capture uses normalized signed 32-bit little-endian samples: 16-bit source values are shifted losslessly by 16 bits and 24-bit values by 8 bits. Exact equality means equality of these decoded sample values, frame count and order, including the split boundary; it does not mean equality of compressed bytes.

The PCM output is **untimed**. Matching dumps cannot establish absence of a real-time output underrun: a delayed transfer can still produce a perfect concatenated file. Timed-null runs add engine timing/control evidence, but they do not capture device samples or establish physical clock/DAC continuity. A rate-changing raw capture can preserve each source's values while reopening the output; it is not one fixed-rate audio stream.

The C API structures/function types follow the installed `mpv/client.h`; the evidence records client API version and observed engine state. This is a Python/ctypes experimental binding, not the chosen production Rust adapter. Config/user-script loading, OSC and URL-extractor integration are disabled for the measurements.

## Findings supported by the cases

- WAV and FLAC at 44.1 kHz/stereo preserve the synthetic samples exactly, including no added/removed/duplicated samples at the split, in the tested capture path. The 16-to-24-bit FLAC case also preserves the normalized sample values exactly.
- With strong gapless mode, 44.1 → 48 kHz retains 44.1 kHz output and converts the second source; stereo → mono retains stereo output and remixes. Weak mode changes the observed output rate/layout and preserves the per-source samples in concatenated raw capture. That says nothing about audible silence or muting during device reopening.
- Native integer-output ReplayGain matches mpv's **1/256-step gain model**, rather than exact nominal dB multiplication. The −6 dB fixture measures a factor of 0.5; −12 dB measures 0.25. The report retains error against the mathematical reference rather than calling that zero error.
- Per-file double-precision volume filters, with native ReplayGain disabled and unity output gain, match the rounded s32 mathematical reference exactly for the tested −6/−12 dB track/album fixtures. This is a candidate mechanism, not a selected production gain policy or proof of every rate/format/hardware path.
- Missing ReplayGain tags with zero fallback preserve unity samples; album-to-track fallback and clipping prevention are exercised. Known peak 0.125 with requested +24 dB is capped to a factor of 8 when clipping prevention is enabled.
- A response delay covered by prefetch does not appreciably extend timed-null completion. A delay longer than prefetch lead, or delayed media with prefetch disabled, does extend it. Network gaplessness is conditional on readiness and buffering.
- Removing B after it is prefetched but while A is current produces completion of A then C in the observed engine events. Near/after-boundary wall-clock probes record the last observed current occurrence before submission: when B has become current, removal stops it and advances to C. This observation is not an atomic check-and-edit transaction or proof of every application race.
- A time-position-conditioned late-removal probe is retained as evidence about observing transition boundaries. If its prerequisite is missed, `unexecuted_actions` records that explicitly and the summary marks it **PRECONDITION MISSED**, never passed.
- Pause/Stop cases observe beyond the delayed prefetch response. Pause retains the paused first entry; Stop retains idle state, rather than the late response restarting playback. This tests engine behavior, not a yet-unwritten application's generation/cancellation guards.

Every submitted command has its reply and observed state recorded. Check `all_commands_succeeded`, `timeout`, `unexecuted_actions`, end reasons and occurrence IDs when interpreting a case; the presence of a result row alone is not a blanket pass.

## Primary references and remaining limits

The integer-gain coefficient and floating-point distinction are established in [mpv v0.41.0 output processing](https://github.com/mpv-player/mpv/blob/v0.41.0/audio/out/ao.c#L572). [ReplayGain selection/clipping](https://github.com/mpv-player/mpv/blob/v0.41.0/player/audio.c#L119) explains the upstream gain request, and the [client header](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h) defines the asynchronous command/event contract. Measurements are primary evidence for this installed package set; references do not replace the measurements.

Lossy codec trimming, arbitrary gain values/peaks, float/DSD sources, real Plex URL/authentication behavior, the Rust FFI lifecycle, real-time PCM capture, PipeWire/ALSA negotiated hardware paths, exclusive device access and rate-change muting remain unmeasured. Per-file filters need production integration and transition verification before a precision guarantee is promised. None of these captures establishes speaker/DAC bit-perfect playback.

Decision context: [Validate gapless handoff and sample-preserving output](https://github.com/42lizard/qoda-player/issues/13). The subsequent [fidelity/ReplayGain decision](https://github.com/42lizard/qoda-player/issues/8) chooses the product promise from this evidence. Keep this prototype on `prototype/audio-feasibility`, with no PR or merge into main.
