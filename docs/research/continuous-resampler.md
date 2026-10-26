# Continuous resampling across tracks — #41

Research date: 2026-10-26. The research-agent source review used no playback, hardware, server, or credential access; implementation-owner PCM-file experiments are recorded separately below. mpv sources below are pinned to **v0.41.0**, commit `41f6a645068483470267271e1d09966ca3b9f413`. Qoda behavior and the approximately 35-LSB uninterrupted-reference deviation are inputs supplied by the implementation owner, not independently reproduced here.

## Finding

**No supported control found in the reviewed v0.41 public API and options preserves continuous DSP across independent native playlist entries while retaining their existing lifecycle semantics.** Keeping the audio device open preserves output buffering; it does not preserve instantiated audio filters. EDL offers a plausible continuous-filter experiment, but changes the input into one virtual file and therefore requires a different logical-track boundary adapter.

## Why independent playlist files reset DSP

At playback termination, `loadfile.c` unconditionally calls `uninit_audio_chain()` and later frees `filter_root`; only audio-output destruction is conditional on gapless mode. The audio-chain destructor frees the decoder and both filter objects. Identical filter configuration on the following file consequently creates new instances. [Playback teardown](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/player/loadfile.c#L1924-L1960), [audio-chain destruction](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/player/audio.c#L265-L297).

`gapless-audio=yes` keeps the initial device format, while `weak` can reopen on incompatible formats. The documented mechanism relies on output buffering while opening the next file. `audio-samplerate` inserts internal swresample when needed; selecting a common output rate does not change the teardown above. [Pinned options](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L2310-L2350).

The internal resampler drains pending output at EOF and forwards EOF; its reset closes/reinitializes the resampling context. Reconfiguration also drains/reinitializes. Separately, the lavfi wrapper frees its graph on reset/destruction and after recovery draining. A SoXR instance hosted inside that graph therefore cannot survive destruction of the graph. [swresample reset](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/filters/f_swresample.c#L313-L325), [draining/output](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/filters/f_swresample.c#L466-L609), [lavfi lifecycle](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/filters/f_lavfi.c#L760-L805).

**Inference:** independent resampler endpoint treatment is consistent with exact joins against per-file references yet different samples against one uninterrupted reference. This source review does not establish the numerical cause or bound of the observed deviation. Sample continuity and preservation of a resampler's history are separate qualifications.

## Candidate controls and input models

| Candidate | Verified mechanism and limitation |
| --- | --- |
| Gapless, persistent options, filter commands | Gapless retains AO as above. `reset-on-next-file` controls option values. `af-command` dispatches commands to an existing lavfi filter; neither changes teardown. [Option persistence](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L846-L862), [filter commands](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/input.rst#L1582-L1598). |
| EDL / `merge-files` | EDL represents source ranges as one virtual timeline, rather than playlist entries. Omitted segment lengths use estimated remaining duration. `merge-files` uses this machinery. [EDL contract](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/edl-mpv.rst#L1-L36), [merge-files](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L837-L840). |
| FFmpeg concat demuxer | Combines packets into one timeline; requires matching streams/codecs/time bases. Incorrect durations can cause artifacts, and differing stream lengths can introduce gaps. This is not evidence of sample-exact arbitrary Plex concatenation. [Official FFmpeg contract](https://ffmpeg.org/ffmpeg-formats.html#concat) (current documentation, not a pinned dependency version). |
| Byte concatenation | mpv's internal concat stream joins bytes. Its constructor takes an internal stream list through `special_arg`; plain `concat://` without it fails. This is not a public arbitrary-container playlist replacement. [Implementation](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/stream/stream_concat.c#L35-L57), [constructor](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/stream/stream_concat.c#L101-L189). |
| Continuous PCM input | Stream callbacks supply input bytes, not an audio-output/DSP callback; their header warns that the API is not stable and same-core libmpv calls from callbacks can deadlock. Raw-audio input declares channels, format, and rate. Producing one continuous PCM input shifts decoding and track-boundary accounting into the application. [Stream API](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/include/mpv/stream_cb.h#L25-L53), [raw audio](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/man/options.rst#L4135-L4146). |

Public-header review found no filter-state retention/export API or PCM output embedding API; the audio-reconfiguration event explicitly describes the absence of audio-output embedding. This is a bounded finding about the reviewed interface, not proof against every possible upstream modification. [Public client header](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/include/mpv/client.h#L1331-L1336).

## EDL nuance worth testing

At an EDL segment transition, the decoder wrapper drains/resets the decoder but suppresses the segment EOF frame downstream. This distinguishes it from whole-file teardown and makes downstream filter-history retention plausible when decoded parameters remain compatible. It does **not** prove uninterrupted SoXR output: format changes still reconfigure filters, and segment clipping/timestamps require measurement. [Suppressed EOF](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/filters/f_decoder_wrapper.c#L709-L718), [decoder transition](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/filters/f_decoder_wrapper.c#L1052-L1083).

Avoid shortcuts through `!no_clip`/`!mp4_dash`: their documentation describes internal, changeable behavior and compatibility restrictions. Delayed opening with multiple segments requires explicit offsets/durations and is documented as untested. [Special EDL headers](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/edl-mpv.rst#L104-L157), [delayed opening](https://github.com/mpv-player/mpv/blob/41f6a645068483470267271e1d09966ca3b9f413/DOCS/edl-mpv.rst#L261-L278).

## Recommended seam and qualification

Qoda design recommendation: keep the current native path honestly qualified as per-file DSP. Explore continuous processing behind a separate session-level input/processing adapter, with explicit sample-position boundaries and occurrence identities. EDL is the smallest experiment; application-produced PCM offers more control but substantially expands ownership. Neither is a transparent option switch: a single native file has aggregate time/lifecycle, while Qoda currently derives ownership from independent native entries. Per-occurrence gain scheduling and current/next authorization must remain explicit.

Current-path constraint supplied by the implementation owner: Qoda stages next only after current playback begins. A static EDL containing both would require preparing that timeline before loading current, or replacing/reloading the active virtual file. Existing independent playlist IDs and revocable current/next grants cannot simply become one virtual-file lifecycle. This limits a direct substitution in today's path; it does not rule out an EDL adapter with deliberately redesigned preparation, boundary attribution, and revocation semantics. Whether a supported mutable-input design avoids reloads remains an experiment, not a verified EDL feature.

Before implementation, test an uninterrupted reference against multiple split positions, including non-integral output phases; unequal gains; seeks, cancellation and queue replacement; format/rate changes; decoder padding/trimming; and current/next revocation. Define where gain acts relative to the continuous resampler and when lookahead commits next-track samples. Verify logical ownership/reporting independently of prefetched or filtered samples. An upstream mpv change preserving filters across compatible playlist transitions is another possible seam, but is outside the reviewed supported API and requires a maintained patch or upstream work.

## Implementation-owner experiment

`python3 tools/check-resampler-boundary.py --expect-reset` generates local 16-bit
stereo fixtures and compares four seconds of uninterrupted conversion against
two native playlist entries and an EDL virtual file. All output is PCM-file only.
Recorded output frames: 192000 in every successful mode. Join maximum/RMS error
against the uninterrupted FFmpeg reference: single-file **0/0 LSB**, native
playlist **34/2.986133 LSB**, EDL **0/0 LSB**. The default invocation fails on the
native reset; `--expect-reset` verifies this known current limitation.

The EDL run with Qoda's `access-references=no` failed format recognition; the
controlled EDL experiment required `access-references=yes` for these trusted local
fixtures. That is an additional production trust-boundary question, not permission
to relax Qoda's setting for provider media. No network, credentials, backend,
hardware samples or persistent desktop changes were used. Scratch files are
removed automatically.

This supports a continuous-input design investigation; it does not implement
queue ownership, mutable staging, seek/recovery, gain changes or revocation for
that input. #41 therefore stays open.
