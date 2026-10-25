# Native ALSA handoff and precision-aware gain qualification

**Throwaway evidence, not production player code.** The selected prototype path qualifies a narrow **digital PCM handoff** on real hardware. It does not qualify speaker/DSP/transport/DAC/analog fidelity, other devices, or the future production Rust application automatically.

## Measured profile

| Field | Observed |
| --- | --- |
| Explicit route | `hw:CARD=avsmax98927,DEV=1` — built-in speaker PCM |
| ALSA PCM type | `HW` (0), not a conversion/mixing plugin |
| Access | `RW_INTERLEAVED` (3); real `snd_pcm_writei` submissions |
| Rate / channels | 48,000 Hz, stereo |
| Container / valid precision | `S16_LE`, 16 bits |
| Kernel PCM parameters | period 1,200 frames, buffer 4,800 frames |
| Source fixtures | Synthetic low-level integer WAV/FLAC |
| Capture boundary | Buffer copied before the real ALSA write; only positively accepted frames retained after return |

See `evidence/environment.json` for the stable card ID, physical path, kernel and exact package versions. Card index 1 is the measurement location on this run; routing uses the ALSA card ID. Preflight installed only the 48 kHz/S16/stereo combination. 44.1/96 kHz and the tested 24/32-bit containers were rejected. Unsupported source rate, precision/layout or misleading preparation hints produce no PCM, rather than a nearest-rate/format conversion.

The Rust shim forwards real ALSA calls. It does not emulate a hardware sink, replace the route with null/file/loopback, modify volume, or substitute a PCM dump for native playback. The installed mpv 0.41 ALSA driver uses read/write transfers; this profile exercises `writei`. A `writen` hook exists but is not qualified here; MMAP is not used/qualified by this profile.

## Run

```sh
python3 prototypes/alsa-gain/qualify.py
```

This emits short, low-level synthetic sound from the selected built-in speakers. Source peaks are approximately -24 dBFS; the positive-gain fixture remains about -18 dBFS peak. It never changes hardware mixers, suspends/kills PipeWire, opens a microphone, or modifies desktop/system audio configuration. A busy/unavailable selected device stops qualification. No root access, installs, Plex credential or personal music is used.

Requirements: Rust/Cargo, Python 3, distro libmpv/libasound, ffmpeg/ffprobe, ALSA utilities and the user session bus. The narrow control fixture also uses Python `dbus` and `gi`. Cargo builds a dependency-free Rust `cdylib` tap; `LD_PRELOAD` applies only to each child experiment. Fixtures and captures live in private `/tmp/qoda-*` directories; only synthetic outcomes/configuration are committed. Raw captures can be inspected locally and the scratch directories removed afterwards.

Individual stages: `preflight.py`, `run.py`, `followups.py`, `confirmation.py`, `source_guard.py`, `review.py`, `report.py`. Review/report only classify already captured data; they do not play audio. The full launcher remeasures instead of replaying old qualified labels. Changed device mapping/packages/settings require new qualification.

## Qualified observations

`evidence/review.json` names each scoped predicate. The current reviewed record has **32 passing scoped checks**, with explicit nonqualifying observations and failures retained separately. It is not an all-pass claim about every observation or the whole application.

- Gain-off WAV/FLAC and ready same-format transitions matched source samples and counts exactly, including an unaligned boundary.
- Track/Album/Auto selection, album-to-track fallback, missing-data unity, unknown-peak boost cap and conservative declared-peak cap matched the chosen static reference at the native handoff. A gain → missing-data unity → gain sequence proves filter removal/replacement rather than carrying an old gain forward.
- The filter uses double precision with native ReplayGain off, native volume 100, volume-gain 0 and explicit no-dither conversion intent. Measured gain-on comparisons had **zero output-LSB error**. The hypothetical 1/256 coefficient path differs by 2–3 LSB in representative gain cases; it is not the captured result.
- Repeat submitted two expected copies with the same gain. Seek comparisons use separate drop-delimited epochs: pre-seek accepted buffers and the exact requested post-seek tail each matched the gain reference. Previously accepted buffers can be discarded; they are not claimed as heard.
- Early removal with a long current occurrence submitted A then C, excluding removed upcoming B. The hardware simple-mixer readout was unchanged before/after this targeted run; values are not exported.
- A held **own** PCM handle produced EBUSY before playback. Missing output selection, mode/device switch and injected ENODEV exercised pause/release/retained-position intent without fallback. Internal speakers were not physically unplugged; Desktop route playback on switching was not exercised.
- App and real DBus MPRIS Volume writes 0, 0.5 and 2 were rejected; unity was accepted and reported while native volume remained 100. A valid preparation completion was accepted; a stale one after Pause was rejected and did not undo Pause. This is a disposable owner-thread **Python guard fixture** controlling the real libmpv process; its integration into the production Rust backend still needs acceptance.
- Source descriptors are probed before ALSA opening. Actual 24-bit media falsely hinted as 16-bit and mono media were rejected without PCM; valid native WAV/FLAC passed.

Static gain tags are extracted from the synthetic FLAC files before preparation. The conservative clipping-cap fixture deliberately declares a peak above its actual low-level waveform peak; it tests the metadata-based cap without emitting full-scale sound. It is not a full-scale hardware clipping measurement. Plex-derived gain units/provenance remain unqualified, and no Plex gain value is used here.

## Timing and failure limits

The short original queue-edit fixture was **not a valid early-edit oracle**: libmpv had already advanced internally before its time-position threshold fired. Its full output/mismatched comparison is retained. A separate long-current fixture reached the intended early state and passed. Decoder EOF/start/restart, accepted PCM and nominal presentation are distinct; [Choose playback occurrence timing and buffered-edit behavior](https://github.com/42lizard/qoda-player/issues/20) now owns the current/history/Plex-reporting clock contract. Do not turn early decoder events into listening/scrobble events.

Driver-state hooks observed XRUN after final content submission/drain. The confirmation check found no XRUN before the final submission; it does **not** declare there were no XRUNs or prove unconditional real-time/network/hardware gaplessness. Exact submitted sample concatenation is the qualified boundary. Kernel consumption, device DSP and downstream outputs are not captured.

Other retained development observations:

- The first tap load aborted before PCM because locally loaded libasound was outside `RTLD_NEXT` visibility; global library visibility fixed the instrumentation. This is not a successful audio case.
- Follow-up analysis initially reran ffprobe per sample; it was stopped and changed to once per file. Existing captures were reused. Original child exit/stderr were not separately retained for those reused cases and are explicitly unknown, rather than guessed.
- A startup control poll initially compared a null/unavailable position with a number. It now waits for real progression; the failed poll is retained and not counted as control acceptance.
- A whole-file comparison is inapplicable after an explicit seek. Review uses the captured drop-delimited epochs rather than hiding the intentionally changed sequence.

Only the measured device/route/formats/package/filter combination can inherit this prototype evidence. The future application must requalify its actual Rust Playback/control integration before displaying a verified profile. Actual hot-unplug, cross-rate native switching, broader formats/precision, network failure recovery and downstream fidelity remain unqualified. The device cannot natively provide the rejected combinations just because software could convert them.

## References

- [ALSA PCM interface](https://www.alsa-project.org/alsa-doc/alsa-lib/pcm.html) and [transfer API](https://www.alsa-project.org/alsa-doc/alsa-lib/group___p_c_m.html): transfer/negotiation semantics.
- [Pinned mpv ALSA output implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/audio/out/ao_alsa.c): the read/write path measured here.
- [mpv options](https://mpv.io/manual/stable/) and [MPRIS Volume](https://specifications.freedesktop.org/mpris/latest/Player_Interface.html): configuration/control contracts, not substitute measurement evidence.

Canonical qualification: [Qualify ALSA handoff and precision-aware ReplayGain](https://github.com/42lizard/qoda-player/issues/17). Existing fidelity/session contracts remain authoritative; production lifecycle and presentation-timing gates remain separate.
