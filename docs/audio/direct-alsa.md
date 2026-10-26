# Direct ALSA output

Desktop/PipeWire remains the default. Menu → Audio settings offers an explicit
Fidelity mode and a discovered direct hardware PCM. Choosing a mode/device is a
draft until **Apply output**. Apply pauses playback, revokes prepared streams,
releases the old native instance, retains the queue and nominal position, and
requires explicit Resume. Restart restores the output selection while paused.
No global PipeWire configuration or hardware mixer volume is changed.

Discovery reads ALSA control and sysfs metadata; it does not open playback PCMs.
The persisted device identity is a SHA-256 fingerprint of physical sysfs topology,
card/PCM identity and available hardware attributes, rather than an ALSA index.
Changing ports/topology may require explicitly selecting the device again. The
private `$XDG_CONFIG_HOME/qoda/output.json` file uses version 1. Invalid, unsafe or
newer settings are preserved and disable playback until repaired; they do not
select Desktop as a fallback. Device names/serial attributes are not published.

## Source and negotiation gates

The initial strict source scope is native FLAC or PCM WAV, stereo integer 16/24
bit at 44.1, 48 or 96 kHz. A bounded 64 KiB header read (five-second timeout)
validates format before playback. WAV extensible requires a FL/FR mask and PCM
subtype; float, mono, other rates/precisions and unknown formats pause with a
visible error. This is a format declaration check, not a content-integrity hash.
Unsupported Fidelity sources remain playable through an explicit Desktop choice;
they are not marked unavailable or automatically skipped in the queue.

On explicit load/Resume, the old native handle is destroyed. Preflight opens only
the selected `hw:CARD=...,DEV=...,SUBDEV=0` route in nonblocking mode, installs
exact rate/stereo/integer/interleaved constraints without writing frames, checks
actual identity, precision and FL/FR mapping, then closes. S32 is preferred; S16
is considered only for a 16-bit source. No packed S24, plugin conversion, mmap or
non-interleaved path is qualified. A new libmpv instance then opens the route
under the same checks again. Busy/lost/ambiguous devices remain paused.

The executable exports a process-local ALSA submission guard. It resolves real
ALSA functions through checked `RTLD_NEXT` lookups, catches Rust panics at each
C ABI boundary and never reads or retains sample buffers. Before forwarding a
write it requires the selected hardware PCM, installed exact rate, adequate
valid precision, and observed FL/FR mapping. Unknown handles and incompatible
negotiation cannot submit frames. Partial/failed writes latch a failure and
require explicit Resume because buffered position may be uncertain. Hardware
mixer write APIs are blocked in Fidelity. No `LD_PRELOAD` library is installed
or injected into any process.

Native ReplayGain, dither, volume gain and speed changes remain disabled. Qoda's
explicit ReplayGain filter is the intentional gain adjustment. A staged successor
must fit the active rate, integer format and valid precision; otherwise it loads
normally after EOF rather than silently resampling to maintain chaining.
Generation and acknowledged-output fences prevent stale preparation from loading
on another route. Applying output never implicitly resumes.

## Evidence and limits

Automated checks cover bounded source-header validation, preserved preferences,
missing/raw device rejection, exact parameter gates, mapping refusal, partial
transfer failures, mixer/mmap refusal, real symbol interposition, checked loader
failure, panic containment and a real libmpv indirect-route rejection. Null-output
checks cover output acknowledgement, paused switching, missing source descriptors,
stale loads and pause intent. Native private-session checks exercise discovered
selection, paused restart and retained queue/position without opening a PCM.

These checks establish submission policy, not hardware fidelity. The UI reports
**configured/unverified**, distinguishes observed negotiation from qualification,
and never labels a discovered device bit-perfect. The player-side PCM matrix in
[ReplayGain measurements](replaygain.md) remains separate from final ALSA-boundary
capture. Issue #27 retains physical PCM/device qualification, per-profile rate,
precision, channel ordering, gain and gapless boundary measurements. No physical
PCM was opened or sound played for the implementation checks.
