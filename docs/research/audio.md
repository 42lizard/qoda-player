# mpv audio and gapless feasibility

Evidence checked: 2026-10-25. Planning evidence for [Establish mpv audio and gapless feasibility](https://github.com/42lizard/qoda-player/issues/3). No audio device, desktop configuration, private Plex server, or credentials were accessed; no playback measurements were performed.

## Finding

mpv supplies useful mechanisms for both desktop playback and a fidelity-oriented path, but documentation does not establish an unconditional combination of remote gapless playback, automatic native-rate switching, and sample preservation. Treat gapless transitions, source selection, gain processing, and the system/device path as separate acceptance gates. The precise promise belongs in [Choose verifiable fidelity and ReplayGain guarantees](https://github.com/42lizard/qoda-player/issues/8); this report does not choose the runtime architecture.

## Gapless and remote-media constraints

The pinned [mpv v0.41.0 options reference](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/options.rst) specifies:

- `gapless-audio=yes` retains the first output format, potentially resampling subsequent tracks.
- `gapless-audio=weak` normally preserves the output device for compatible formats, reopening it on incompatible changes; compatibility details are implementation-dependent.
- Network startup can outlast buffered output and create a gap.
- `prefetch-playlist=yes` opens the next URL after the current one is completely read. It is off by default and assumes the playlist is not edited; it is not an unconditional next-track readiness guarantee.

The [player implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/player/audio.c) separately checks weak-format compatibility and strong gapless retention. This supports using a persistent playback core and an actual upcoming playlist entry for investigation, rather than starting a new player on every end-of-file event. That is an architectural implication, not a selected design.

**Inference:** promising uninterrupted transitions between every pair of sample rates while also forbidding resampling is unjustified. Hardware renegotiation can be required. A defensible candidate contract is sample-contiguous transitions for a tested set of same-rate compatible formats, with an explicitly described interruption at incompatible device-format changes. The human must choose that contract.

Original-media URLs should be prepared before handoff, including valid authorization and sufficient buffering. Test queue edits after prefetch, redirects, slow range requests, network stalls and URL expiry. The minimum useful prebuffer and invalidation behavior are unknown until measured against a representative server and network.

## Gain and decoded-sample fidelity

The [current mpv manual](https://mpv.io/manual/stable/#audio) documents software volume at `100` as unity, `replaygain=no|track|album`, album-to-track fallback, preamp, missing-tag fallback gain, and clipping prevention via gain reduction when `replaygain-clip=no`. It also documents that forcing a different output sample rate inserts resampling, and mismatched channel layouts can insert conversion. `audio-exclusive=yes` is supported by some outputs, including PipeWire; it is not a universal device guarantee.

In the [v0.41.0 playback source](https://github.com/mpv-player/mpv/blob/v0.41.0/player/audio.c), the effective gain combines software volume, ReplayGain, additional volume gain and mute. The [demuxer](https://github.com/mpv-player/mpv/blob/v0.41.0/demux/demux_lavf.c) imports FFmpeg ReplayGain side data, including gain and peak values, into the selected audio stream. Provider JSON fields do not automatically become that stream metadata; a Plex-only loudness source requires an explicit bridge and correct per-track timing. This bridge remains unproven.

**Proposed validation conditions, not verified guarantees:** original media without server transcoding; normal speed and pitch; no EQ, crossfade, normalization, channel remix or other modifying filters; unity software and downstream gain except the agreed ReplayGain operation; source-compatible rate, channel layout and sample precision; no silent output fallback. Check actual negotiated output, not just requested options. Isolate configuration and user scripts so a personal mpv profile cannot silently change the signal path.

“Bit-perfect” needs an endpoint: compare decoded PCM sample values at a specified digital boundary, allowing lossless representation changes if the contract says so. It does not mean compressed file bytes equal device PCM. With ReplayGain enabled, multiplication and output quantization necessarily belong to the permitted transformation; a byte-identical claim no longer applies to the ungained reference. Lossy inputs require a defined decoded reference, not comparison against a nonexistent original master.

## PipeWire and ALSA paths

| Path | Verified mechanism | Limit on a fidelity claim |
| --- | --- | --- |
| Native PipeWire | [mpv v0.41.0 source](https://github.com/mpv-player/mpv/blob/v0.41.0/audio/out/ao_pipewire.c) submits a stream rate, format and channels, suggests `node.rate`, and maps the exclusive option to `PW_STREAM_FLAG_EXCLUSIVE`. | Submission is not proof of the device's final PCM values or physical rate. |
| PipeWire exclusive | The [PipeWire stream API](https://docs.pipewire.org/group__pw__stream.html) defines exclusive access separately from `PW_STREAM_FLAG_NO_CONVERT`. mpv's inspected connection flags do not include `NO_CONVERT`. | Exclusivity must not be presented as proof that format conversion or resampling is absent. |
| Direct ALSA hardware PCM | The [ALSA plugin reference](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm_plugins.html) describes `hw` as direct kernel communication without conversion; `plug` converts and `dmix` mixes streams. | An arbitrary ALSA device name such as `default` is not equivalent to a verified hardware path. Device access and downstream hardware still require testing. |
| mpv ALSA negotiation | [mpv's ALSA implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/audio/out/ao_alsa.c) negotiates supported formats, uses a nearest sample-rate request, and can adjust channels. | Direct hardware selection alone does not prevent mpv from negotiating a different rate/format and adapting its input. Compare actual parameters to the contract and reject or disclose a mismatch. |

PipeWire's [property reference](https://docs.pipewire.org/page_man_pipewire-props_7.html) distinguishes stream format, graph rate and device behavior. `node.rate` is a suggestion applied when allowed and non-disruptive; `node.force-rate` can force the graph rate; `resample.disable` prevents incompatible rate negotiation. Internal graph processing uses float32. Thus matching the application's requested rate is insufficient evidence of end-to-end sample equality. Gain, routing, conversion, clock adaptation and device negotiation need inspection.

The [server configuration reference](https://docs.pipewire.org/page_man_pipewire_conf_5.html) describes allowed alternative graph rates and changes when devices are idle. Changing global rate policy is not an application-local guarantee. For this project, investigate whether a user-scoped per-player path can satisfy the agreed mode without unexpectedly interrupting desktop audio; no policy or configuration change is selected here.

The [mpv output-driver reference](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/ao.rst) warns that default output selection can change and documents ALSA resampling as a separate option. Select and observe the intended path explicitly in the feasibility test. Switching modes must test release/reacquisition, competing playback, unsupported rates and device removal; do not silently call a fallback path “fidelity.”

## libmpv versus a controlled mpv process

Both expose the mpv playback engine; neither boundary changes these audio constraints by itself. The [libmpv client header](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h) supports commands, observed properties and events, with asynchronous calls and event-loop wakeup integration. Synchronous operations can wait indefinitely on a slow network, so any GUI integration needs an asynchronous boundary and event handling. Playback continues independently of event polling, although an unserviced client event queue can congest.

The [JSON IPC reference](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/ipc.rst) supports local command/reply/event control over a Unix socket. It supplies no authentication or encryption and exposes powerful commands; a companion-process design must keep its socket private. Process ownership, reconnection and state reconciliation are separate design obligations. Embedding instead requires a native binding and a safe lifecycle around the GUI/runtime. These are tradeoffs for [Choose runtime architecture and module boundaries](https://github.com/42lizard/qoda-player/issues/9), not an audio-quality verdict.

## Focused feasibility prototype and acceptance evidence

Before promising gapless or sample preservation, use a throwaway harness with the exact packaged mpv/libmpv, FFmpeg, PipeWire/session-manager versions and chosen devices. The following is a proposed investigation, not work performed here:

1. Generate a known continuous PCM reference, split it at a sample boundary into separately served tracks, and deliver them over controllable HTTP. Compare concatenated output against the unsplit reference for added/removed/duplicated samples. Include common lossless formats and correctly trimmed lossy fixtures separately.
2. Exercise same-rate compatible transitions, bit-depth changes, 44.1/48/96 kHz transitions, channel changes, seek/skip, queue mutations around prefetch, slow starts and stalls. Record whether a failure is an underrun, device reopen, source issue or queue-control race.
3. Capture mpv output PCM independently of the real sound path. The [PCM output implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/audio/out/ao_pcm.c) supports writing output samples, but its format negotiation and untimed behavior differ from a device. A successful dump comparison validates only the tested player-side path, not actual hardware continuity.
4. Measure the selected device path through an appropriate digital capture/loopback or hardware-supported verification, recording actual rates/formats, routing and gain. A virtual sink or analog loopback alone cannot certify every downstream digital sample. Check start/end truncation and hardware rate-change muting too.
5. Repeat with ReplayGain disabled, known track/album gains and peaks, missing/partial tags, clipping prevention and provider-only metadata. Compare against an explicitly defined numerical reference and tolerance/quantization rule, and verify the gain changes at the correct boundary without double application.

Pending decisions: define the fidelity endpoint and supported device class; choose behavior for incompatible rates or unsupported formats; settle gain source/precedence and volume control; set a bounded same-format gapless contract; choose fail-versus-disclosed-fallback behavior; decide how prefetch and queue edits coordinate. A hardware test task may be needed once those choices make a concrete experiment possible. No first-release guarantee should imply that this documentation research already measured the user's system.
