# Continuous PCM input adapter — #41 follow-up

2026-10-26; source investigation only. No playback or system/account changes. mpv is pinned to **v0.41.0 / `41f6a645068483470267271e1d09966ca3b9f413`**. SoXR is pinned to **0.1.3 / `945b592b70470e29f917f4de89b4281fbbd540c0`**, matching the locally installed `/usr/include/soxr.h` version and the upstream SourceForge tree; GitHub source links below are the matching mirror. Existing EDL results and Qoda's grant/ownership requirements were supplied by the implementation owner, not reproduced here.

## Assessment

**A continuous PCM input is a viable experiment seam, not a verified replacement.** It can preserve one final filter graph while independently decoding authorized current/next sources. It also makes Rust responsible for framing, bounded admission, discontinuities, and logical occurrence attribution. Two important limits remain: mpv's PCM AO is a file writer with blocking, uncancellable writes, and public playback properties are not sample-exact boundary callbacks.

**Smaller first mechanism:** current/next decoder → bounded raw PCM → Rust gain → one final libmpv `rawaudio` stream → existing persistent lavfi/SoXR → device. Keeping SoXR in the final mpv removes direct SoXR FFI, a second quality-configuration implementation, and an intermediate integer quantization. The input must remain one file, with no per-track EOF. Direct Rust-owned SoXR before final mpv is an alternative if explicit resampler state/delay control is needed; neither retains native per-track playlist events automatically. See the [previous lifecycle research](continuous-resampler.md).

## Verified mpv interfaces

### Decoder output

`ao=pcm`, `ao-pcm-waveheader=no`, and `ao-pcm-file=<path>` select raw PCM output. `ao-pcm-append` is available but is unnecessary for separately owned decoder streams. [PCM AO options](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/ao.rst#L284-L299).

With headers disabled, PCM AO accepts the negotiated native-endian format, converts planar layout to interleaved, opens its path with `fopen`, and emits through `fwrite`. `audio-format=double` means 64-bit floating point; `s32` means signed 32-bit. The writer is untimed, so decoding can run ahead; its reported queue/delay are zero. A pipe supplies backpressure, not playback pacing. `audio_write` ignores `fwrite`'s return value. Headered mode would reject/fallback from double and attempts to seek back on close, so it must be disabled. [AO implementation](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/out/ao_pcm.c#L109-L225), [format definitions](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/format.c#L28-L39), [format names](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/format.c#L115-L128).

**Cancellation hazard inferred from source:** a full pipe can block that writer; AO teardown joins the writer thread before driver destruction. Cancelling final-input reads does not unblock a separate decoder's AO write. [Thread join](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/out/buffer.c#L511-L526), [driver write](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/out/buffer.c#L671-L675).

Design consequence: an in-process decoder needs a dedicated pipe reader that switches to bounded discard/drain during stop and remains alive until decoder teardown completes. Do not wait for termination while the reader is waiting on a full downstream ring. FIFO opening can also block; open-order/startup cancellation needs a test. A subprocess decoder provides a stronger kill boundary if this cannot be bounded reliably, but changes process ownership. A regular file avoids pipe blocking but is not a streaming solution unless spool size is capped; no public PCM-output callback was found.

Per-source decoder recommendation: `gapless-audio=no`, so file termination actually destroys its AO and closes/flushes stdio. The writer does not flush each write; a retained AO can hold the final partial stdio buffer and keep the pipe open after the source finishes. Treat complete pipe bytes plus successful decoder completion as the handoff condition, not `END_FILE` alone, and verify expected frame counts because ignored write failures can otherwise look successful. [Conditional AO teardown](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/player/loadfile.c#L1939-L1952).

### Final raw-audio input

Use `demuxer=rawaudio`, `demuxer-rawaudio-channels=stereo`, `demuxer-rawaudio-rate=<R>`, and `demuxer-rawaudio-format=doublele`/`doublebe` or `s32le`/`s32be`. Native-endian aliases `double` and `s32` also exist. Rates accepted by this version are 1000–384000 Hz. Frame size is sample bytes × channels; packet timestamps derive from byte position/frame size/rate. Packets normally request `R/8` frames, so short callback writes do not establish logical-track packet boundaries. [Accepted formats](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/demux/demux_raw.c#L40-L76), [audio framing](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/demux/demux_raw.c#L145-L190), [packet timestamps](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/demux/demux_raw.c#L269-L310).

Recommendation: admit only validated same-rate stereo WAV/FLAC initially. Explicitly neutralize decoder gain, ReplayGain, speed/pitch processing, filters, channel conversion, and rate conversion; inspect actual output parameters. Use f64 through gain/SRC. Converting an already correctly scaled signed s32 value to f64 with division by `2^31` is exact; whether mpv's complete decode/conversion path supplies the required values remains a synthetic-fixture qualification. Output format alone does not prove source scaling or fidelity. mpv's AO gain code bypasses multiplication at unity. [Unity bypass](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/audio/out/ao.c#L629-L649).

### Custom input callbacks

`mpv_stream_cb_add_ro` registers an input protocol. Mandatory callbacks are read/close; seek/size may be absent (unsupported). Reads block when empty, may be short, return **0 only for final EOF**, and -1 for failure. A seek callback is probed at byte zero. `cancel_fn` (API 1.106+) interrupts current/future reads/seeks from another thread and must not block. Callbacks must not call the same libmpv core. Only libmpv closes a stream; resources/callbacks must survive until close and protocol registrations until core destruction. The header labels this API unstable. [Pinned callback contract](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/include/mpv/stream_cb.h#L25-L216).

Rust design: one pinned/refcounted cookie per generation; catch unwinding at FFI; validate lengths; block with a cancellation-aware condition variable; cancel sets a flag/wakes immediately; no network/decode/control calls under callback locks. Support arbitrary byte-sized reads from serialized complete frames without inventing EOF when a request is smaller than one frame. Open should complete promptly because cancellation registration follows successful open. mpv marks this stream origin unsafe; registration does not justify relaxing access-reference restrictions. [Open/cancel wiring and origin](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/stream/stream_cb.c#L57-L117).

Prefer this local callback experiment to adding HTTP framing/routes. A loopback HTTP transport remains possible, but requires independently tested disconnect, EOF, range, read-ahead, and authorization behavior; those contracts were not investigated here.

## Direct SoXR alternative

`soxr_create` configures rates/channels, I/O types, quality, and runtime; use explicit `SOXR_FLOAT64_I` input/output. Lengths and `idone`/`odone` count **frames per channel**, not scalar interleaved elements. `soxr_process` can partially consume/produce: retain unconsumed input. Non-NULL zero-length input is not EOF; NULL input means no more input will ever arrive. `soxr_delay` reports current delay in output samples; `soxr_clear` starts a fresh signal and loses continuity. [0.1.3 header](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/src/soxr.h#L58-L178), [I/O types](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/src/soxr.h#L208-L242).

At ordinary track boundaries, retain the same handle and feed the next gain-adjusted frames without NULL, clear, or re-create. On temporary starvation, wait; do not flush or fabricate silence. At true session EOF, signal EOF and drain until no output remains. The official streaming example demonstrates final draining; source makes flushing sticky. [Streaming example](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/examples/2-stream.C#L43-L76), [process implementation](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/src/soxr.c#L752-L805).

Serialize all access to each handle: no concurrent-use guarantee was found. Preserve buffers until each call returns; check every error/count and non-finite sample. Defaults are not a promise to match the existing FFmpeg profile. Pin quality, phase, precision, and runtime settings and compare output empirically. Integer output introduces clipping/rounding; float64 avoids that intermediate quantization. SoXR's integer full scales are `2^31`/`2^15` versus float unity. [Scaling](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/src/soxr.c#L363-L430).

## Time, trust, and recovery

`audio-pts` is playback position with driver delay accounted for and may be negative. Internally it is last written PTS minus speed × AO delay. Property observations are coalesced and some changes may not generate events. Thus crossing a threshold can drive nominal ownership, but is not a sample-exact callback or proof of hardware emission. [Property](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/input.rst#L2280-L2290), [clock calculation](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/player/audio.c#L615-L631), [observation limits](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/include/mpv/client.h#L1173-L1185).

Design recommendation: keep integer cumulative source-frame boundaries. With final-mpv SRC, nominal time is source frames / source rate; with pre-resampled PCM it is on the output-frame timeline. Define rational boundary rounding explicitly and qualify against actual flushed counts. Do not subtract `soxr_delay` from final `audio-pts` again: resampler buffering is not an extra audible-time offset. SRC impulse responses mix adjacent-track samples around a nominal boundary; gain must be applied to each source before SRC, and lookahead is not ownership/reporting.

Keep exactly current plus one speculative next authorization. Fence source fetch, decoder output, gain admission, resampler input, callback bytes, and clock events by generation/occurrence. Admission into persistent SRC/final-mpv buffers is a commit point: revoking a URL cannot retract admitted samples. For the first adapter, handle cancelled/replaced committed next with a deliberate discontinuity: stop/recreate the final session and resampler, discard stale buffers/events, and recover from the authoritative position. More advanced rollback/reconstruction is a separate design. Decide and test this policy before production integration.

Bound every ring and worker allocation, plus decoder/final mpv buffering. `demuxer-max-bytes` can be exceeded slightly; readahead is approximate and cache can override it. `stream-buffer-size` and AO buffering also count. These options limit buffering; none promises immediate revocation of audio already queued. [Demux limits](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L4175-L4198), [readahead](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L4265-L4285), [stream buffer](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L5485-L5496).

## Experiment gates and distribution

First build a synthetic, device-free experiment with exact 16/24-bit stereo WAV/FLAC scaling fixtures, unity decoder gain, split positions at non-integral output phases, unequal per-track gains, and chunk-size variation. Compare one continuous reference, native files, and joined PCM. Measure all buffered frames and clock attribution. Force empty/full pipe/ring states, cancellation during open/read/write/drain, stale next replacement before/after admission, stop/seek/recovery, truncated frames, and decoder errors. Require bounded teardown and zero stale-generation output. Rate/layout changes remain unsupported in the first profile and must end/restart the session explicitly.

SoXR source is LGPL-2.1-or-later; bundled PFFFT has additional retained-notice terms. mpv is GPL-2.0-or-later by default, with an LGPL build path and dependency-dependent licensing. A Rust wrapper's license does not replace those library obligations; audit the actual distributed builds rather than assuming any installed libmpv is LGPL. No licensing conclusion for a new distribution is asserted here. [SoXR licence](https://sourceforge.net/p/soxr/code/ci/945b592b70470e29f917f4de89b4281fbbd540c0/tree/LICENCE), [PFFFT notice](https://github.com/chirlu/soxr/blob/945b592b70470e29f917f4de89b4281fbbd540c0/src/pffft.c#L14-L49), [mpv licensing](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/Copyright#L3-L24).


## Implementation-owner experiment: isolated decoder and persistent final graph

The Rust example `crates/qoda-backend/examples/continuous_pcm.rs` is a local,
synthetic-only experiment, **not enabled in the production player**. It uses an
owned mpv subprocess per source, a private socket for load/property commands,
64-bit float PCM, Rust per-source gain, a 64 KiB bounded ring, and a single final
libmpv/SoXR graph with PCM-file output. Every decoder is killed/reaped by its
owner on failure/cancellation. Paused startup verifies actual decoded/output
rate, channel map and double format before unpausing. Property requests during
active pipe output can deadlock against a blocked PCM writer; paused verification
avoids that measured startup failure. Source identifiers never enter argv.

Callback storage survives until native destruction/close; cancellation uses an
atomic flag and condition-variable wake, with a 20 ms timed wait covering a
lost-notification race. Empty input waits rather than returning EOF. Only the
producer's final seal returns EOF; ordinary source boundaries emit neither EOF
nor silence. Reads may be byte-sized. Partial frames, non-finite samples, excess
or missing decoded frames and rate changes are refused. Current tests supply
expected frame counts independently from the generated fixtures; a production
adapter still needs a bounded verified-header frame-count descriptor.

**Raw demuxer finding:** on this installed mpv 0.41.0 / FFmpeg 9 stack, mpv's
`demuxer=rawaudio` emitted 101991 output frames for 88313 input frames at
44.1→48 kHz, versus the reference's 96123 frames. The same excess tail occurred
with the mpv CLI reading the identical regular f64 file, independently of the
custom callbacks. The difference corresponds to rounding the input up to the
raw demuxer's 5512-frame packet size. This establishes a measured wrong tail on
this stack, not an upstream root-cause attribution. The example instead forces
FFmpeg's `f64le`/`f64be` input through lavf with explicit sample rate/stereo options;
it returns the expected tail and exact samples in the measured matrix. Lavf warns
that raw input lacks container timestamps, so nominal clock mapping remains a
separate required qualification. No seek support is claimed by the raw transport.

Run `cargo build -p qoda-backend --example continuous_pcm --locked`, then
`python3 tools/check-continuous-pcm.py --example target/debug/examples/continuous_pcm`
(adjust the path for CARGO_TARGET_DIR). These checks now also run in tools/check.sh.
Nine original-fixture comparisons pass:

- 44.1→48 kHz 16-bit WAV and 24-bit FLAC, split at 44117/44099 source frames.
- 96→48 kHz 24-bit WAV and 16-bit FLAC, split at 95999/96007 source frames.
- 48→48 kHz 24-bit FLAC, split at 48001 frames.
- Exact 16/24-bit WAV/FLAC scaling into signed 32-bit output, including half-gain.

Converted cases match the **whole uninterrupted** f64/FFmpeg/SoXR reference at
0 output LSB across the complete output and the join, including unequal gains.
Frame counts are 96123, 96057 or 96113 as appropriate; all exact-scaling cases
match at 0 signed-32-bit LSB. Every case reaches but never exceeds the 65536-byte
application ring bound. This is not a bound on all subprocess/native buffering.
Five fault cases reject mismatched frame counts, actual rate, invalid gain and
rate changes. Six Rust tests additionally cover starvation, byte reads, sealed
input, non-finite frames, full-ring/empty-reader cancellation, blocked IPC,
blocked decoder pipe kill/reap and native callback lifetime after cancellation.

Still required before production: current-plus-one-future admission/revocation,
header-derived exact frame counts, a mutable producer that does not seal on a
temporary absence of next, nominal clock/ownership with actual pacing, pause,
queue replacement, seeks, recovery, changed output plans and current/next grants.
Very short tracks may require more lookahead than one authorized successor can
supply; the integration must explicitly fall back or resolve that horizon rather
than invent EOF/silence or authorize arbitrary future tracks. Physical output
remains unmeasured. #41 stays open.


### Paced short-successor horizon measurement

The follow-up command `cargo run -p qoda-backend --example continuous_pcm -- --horizon`
uses timed `ao=null` (no sound device), one 44100-frame current and only a
one-frame successor, with a third input withheld and no final EOF. In a 2.3-second
window the maximum positive nominal `audio-pts` was **0.9157916667 s**, below the
1.0 s handoff boundary; releasing the third input allowed completion. No silence
was injected. This demonstrates a real lookahead/ownership stall for this
44.1→48 kHz profile, not a universal latency bound. PCM admission cannot stand in
for nominal ownership to sidestep the authorization horizon. The
[concrete integration choices](../continuous-conversion-boundary-proposal.md)
record the already delegated recommended default: retain one successor and
explicitly fall back when the qualified horizon is unavailable. This experiment
changes no production grant policy or playback route.
