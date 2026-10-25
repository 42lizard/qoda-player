# Proposed playback timing contract

Decision evidence for [Choose playback occurrence timing and buffered-edit behavior](https://github.com/42lizard/qoda-player/issues/20). The pure model in `index.html` has sixteen guided cases. This is a proposed Rust Playback/Session seam, not a production adapter or a new audio engine.

## Choice

Use **nominal presentation** for current-occurrence presentation, Previous's position input, MPRIS metadata and semantic reporting. Keep engine selection/preparation separate. A decoder/file event does not prove presentation. A nominal driver clock is still an estimate, not evidence at a DAC or speaker.

The backend remains authoritative. Session owns membership/order, Previous navigation, visit identity and intent. Playback owns libmpv and translates its events into qualified clock/barrier facts. Providers receive semantic reporting events; neither QML nor a Plex callback advances current playback.

A queue occurrence is session membership. A **visit** is a presentation pass through that occurrence. Repeat and explicit replay/navigation produce a new reporting visit; seek or output recovery within the same pass keeps its visit. Session's existing Previous cursor is separate: navigating backward must not append visits to its forward-navigation frontier or resurrect removed membership.

## Facts and clock evidence

| Input | What it establishes | What it does not establish |
|---|---|---|
| Prepared playback | Access, selected media version and immutable per-visit gain plan ready | Playback/listening |
| `start-file`, `file-loaded`, playlist entry identity | Engine selected/loaded a particular mpv entry | Which buffered file is presenting |
| `playback-restart` | Engine restart/readiness after load/seek | First sample at the device/speaker |
| Natural `end-file` | Decoder/file work ended without its reported error | Output drain or a listening completion |
| Accepted PCM / optional output diagnostics | Bytes accepted at the named output boundary | Bytes heard; buffers may later be discarded |
| Coherent `audio-pts` | Nominal file-relative position including mpv's reported AO delay | A generic verified hardware clock |
| Matched natural EOF plus nominal output drain | Nominal end of the intact output plan | Analog completion or proof that every previously unconfirmed short visit was heard |
| Seek/reset/drop/device failure | Discontinuity; old accepted buffers may be lost | Listening credit for discarded data |

The pinned mpv 0.41.0 [property implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/player/command.c) makes `audio-pts` unavailable outside its playing/draining states. Its [audio implementation](https://github.com/mpv-player/mpv/blob/v0.41.0/player/audio.c) obtains that position from written PTS minus AO delay. [Pinned property documentation](https://github.com/mpv-player/mpv/blob/v0.41.0/DOCS/man/input.rst) allows negative values and identifies `playback-time` as an alias of `time-pos`; the alias is not an independent clock.

Sample on the owner at a 20 ms target cadence. Read/bracket the mpv playing entry ID around the clock and transport flags; discard an inconsistent sample. Key by mpv entry ID mapped to owner visit/attempt, never path/title or mutable playlist index. Observe seek/restart/EOF/idle events as well. Property callbacks may be delayed/coalesced and are not a sample clock.

A positive, coherent, in-range nominal position while output is running can confirm its visit. Zero alone does not establish a first listening observation. Keep pause/cache/seek status with the sample; do not credit time merely because desired intent is Playing.

A negative position may point into earlier buffered visits. Retain an ordered output ledger. Walk backward only through predecessor spans whose exact source-frame lengths, offsets, format/processing conditions and continuity are established for that prepared version. Convert the resulting position to that span's source coordinates. A gap, missing span, changed output epoch or estimated length prevents the mapping.

The fixture knows its WAV lengths from generated PCM. Production must not turn Plex milliseconds or an unqualified mpv duration hint into an exact span. Codec/header evidence needs provenance, immutable-version binding, valid completion and qualification for the decoder/processing path; truncation, unsupported trimming or decode errors invalidate it. Exact-length inference is optional: long-file positive witnesses still work without it. Real Plex timing metadata remains a live-provider acceptance item.

Missing clocks cannot be reconstructed from exact lengths alone. The tiny-chain measurements include complete chains without any usable positive clock, and sometimes no clock at all. The default is **explicit incomplete attribution**, not a timer-based invented history: show Transitioning / position unavailable, retain last confirmed recovery information, and omit unconfirmed per-visit reports. The queue's retained membership still permits an explicit content selection.

Do not free-run through an unavailable clock, pause, buffering, seek or route reset. Treat non-advancing/stale clock evidence as uncertain after 250 ms of expected progress; a 3 s bounded output-stall deadline pauses with a recovery error. A known paused/cache state freezes the relevant progress clock instead of triggering that watchdog. These are recommended control defaults, not measured universal latency bounds.

## Interface shape

All 64-bit counters/timestamps crossing JSON use decimal strings, as already agreed. Native ownership types are Rust values; the HTML uses small fixtures and is not the production wire representation.

```text
PreparedVisit {
  session_id, occurrence_id, visit_id, media_version,
  preparation_id, access_generation, intent_generation,
  gain_binding, duration_evidence?, source_seekability
}
OutputPlan {
  render_epoch, queue_revision, intent_generation,
  visits: [PreparedVisit + source_start + source_end?], desired_intent
}
ClockWitness {
  render_epoch, attempt_id, adapter_sequence, sample_monotonic,
  engine_entry_id, visit_id, file_position, transport_flags,
  bracket_consistent, clock_provenance, span_mapping_evidence?
}
Presentation {
  current_visit?, source_position?, confidence,
  last_confirmed?, unconfirmed_visits, reporting_status
}
BarrierResult {
  operation_id, frozen_snapshot, old_epoch_invalidated,
  discarded_output_evidence, downstream_retractability,
  classification: future | current | uncertain,
  new_plan_installed, actual_recovery_position?, acknowledgement
}
```

Events distinguish Prepared, EngineSelected, DecoderEnded, ClockWitness, PauseAcknowledged, OutputInvalidated, PlanInstalled, NominalDrained and Failed. Optional submitted-frame/delay diagnostics are evidence at their declared boundary; standard libmpv does not expose a general per-occurrence PCM-acceptance callback. The LD_PRELOAD ALSA tap is qualification instrumentation, not a production dependency or a portable clock API.

Session identity, visit identity, preparation identity, **intent generation** and **render epoch** have different jobs. Pause invalidates automatic-resume authority without pretending all actual clock evidence ceased immediately. A seek, flush/rebuild, stop, source replacement or output discontinuity invalidates the render epoch. A delayed preparation needs current membership/version/gain and resume authority; a clock sample needs its actual output epoch/attempt and ordered freshness. Do not reject real pre-pause presentation evidence merely because a newer Pause intent exists.

Duplicate/older sequences and old epochs are rejected. A missing sequence or unexplainable clock regression loses presentation certainty and requests an owner snapshot; do not apply a later event while silently assuming no intervening reset. A new native handle or entry is remapped explicitly to the same recovery visit or a new repeat/replay visit. Filename equality is insufficient, especially for repeated occurrences.

## Queue edits and control barriers

Serialize edits with output facts on the Rust owner. The UI shows a pending edit until the native operation and any required reset are acknowledged.

1. An untouched future entry can be removed with a synchronous native mutation and a confirming owner snapshot. If the engine advanced during that operation, reclassify it; a removed playlist row does not by itself prove mpv stopped rendering it.
2. If affected future data is already decoded/buffered, freeze output and acquire a coherent final nominal snapshot. Classify at that barrier, not at button-click time. A small race may turn a requested future removal into a current removal.
3. For confirmed A with B still future, invalidate the old render epoch, stop/flush and rebuild A at the sampled confirmed position (or explicitly labelled last-known recovery point), followed by the accepted remaining queue. Preserve A's visit and locked gain plan. Removed B may not be reintroduced by stale preparation or repeat. Prefer a gap/short replay to removed future output.
4. If B is already confirmed current, record that real transition and apply the existing current-removal rule: advance to the next eligible occurrence, preserving the latest playing/paused intent. Do not replay A or rewrite history to disguise B.
5. A flush acknowledgement must name its boundary and retractability evidence. Native ALSA reset observations do not prove that a PipeWire shared sink/DAC can retract audio already beyond the app's control. Never promise to retract downstream audio or call an unqualified reset a verified silence boundary. Where that distinction prevents a strong future/current classification, return **removed after reset; earlier presentation uncertain**, retain the uncertainty and leave playback paused for review/reselection. Do not acknowledge “B never played.”
6. Resume a rebuilt plan only if its preparation/operation/intent generation still matches the latest intent. Pause during recovery wins. Freeze and reset are not permission to autoplay. If native reset completion is uncertain, keep the edit pending/failed and stop; a bounded teardown/recreate of the old owner is the fallback, not accepting late old audio into the new plan.

The native accepted-removal case observed 73,200 accepted frames, 3,936 reported pending frames and an A boundary at 72,000: 1,200 B frames were accepted while the reported cursor was still in A. Reset followed about 1.8 ms after the sampled command boundary. Post-reset PCM exactly matched A's recovered tail then C. This is a scoped reported-ALSA-boundary observation, not proof at a DAC. PipeWire measurements establish event/control behavior; their post-reset downstream waveform/retractability is not qualified here, so the uncertainty branch applies where needed.

Recovery must report the observed position. An earlier reconstruction started one frame before the rounded requested point; the strict requested-point comparison remains false in evidence. Its suffix exactly matches the source from that observed point. This does not qualify generic sample-exact seeking or erase a deliberate discontinuity from output accounting.

## Current, Previous, history and MPRIS

UI and backend MPRIS derive current metadata from the same Presentation snapshot. While uncertain, show an explicit transition/unknown state and last-confirmed context; do not label engine-selected B as heard/current. MPRIS can expose NoTrack metadata when no current visit is established; its limited status vocabulary does not carry the richer QML certainty state. Playing/Paused/Stopped follow the owner transport/intent policy, never a competing UI player.

Previous's three-second decision uses current nominal **source position**, not a decoder's reset `time-pos` for B. Removed membership stays excluded. Session retains the existing history-navigation cursor; seeks/rebuilds do not append history and repeat/replay reporting identities do not make Previous bounce. If the relevant current position/boundary is still unknown, freeze/recover first; if it cannot be established, remain paused with a clear selection/recovery action rather than guessing the threshold. MPRIS seek/previous capability may be temporarily unavailable.

Pause changes desired intent immediately; its output acknowledgement determines the transport boundary. Already-rendered audio observed before that acknowledgement can legitimately establish a visit. Preparation alone never does. Seek preserves the visit, invalidates old output and confirms the actual returned position before reporting it. A source-position jump is not elapsed listening time. Repeat schedules a new explicit visit/native entry; do not rely on an indistinguishable implicit `loop-file` rewind for reporting identity.

EOF retains a confirmed current while its tail drains. A matched final nominal drain may close that confirmed visit even when `audio-pts` has become unavailable at EOF. It cannot manufacture visits for an entirely unobserved tiny chain. Stop, removal, seek/reset, missing events or known output errors are not natural completion. End of queue retains the session without implicit autoplay; stop/clear and Quit retain their separate Session/lifecycle policies.

## Mock reporting contract

The model emits semantic nominal-start/position/pause/seek/stop/completion events only for confirmed visits. It does not contact Plex or call a manual scrobble endpoint. Preparation, decoding, queue insertion, zero-only clocks and discarded accepted buffers do not count as listening. Keep unconfirmed intervals/visits visible as reporting-incomplete.

Bind reports to account/source/session/occurrence/visit and preparation/media version, with local monotonic ordering. Position is media-relative. Conservative nominal listening credit accumulates only coherent advancing observations in the same epoch/visit with no pause/cache/seek/gap; never credit a seek jump or guessed time while a clock is unavailable. Completion is a nominal outcome, not a claim about human hearing.

Use an immediate semantic event for start/pause/confirmed seek/stop and a 10 s progress-coalescing default. The HTML displays semantic observations, not ten-millisecond HTTP requests. Serialize reporting per provider playback session; coalesce obsolete progress, fence credential/session changes, stop obsolete plays from superseding newer intent, and retain uncertain remote outcomes. Shutdown uses the existing bounded policy. Do not claim exactly-once reporting from local sequence numbers.

[Verify live Plex authentication and server contracts](https://github.com/42lizard/qoda-player/issues/16) must validate actual timeline/scrobble effects, session identifiers, seeks, ordering/timeouts, shared permissions and server-version behavior with a designated controlled track. Plex may derive played state itself; nominal position is not permission to assert its server-side scrobble policy or to test on arbitrary music/history.

## Supported scope and measurements

Both Desktop/PipeWire and the selected direct-ALSA path use the same confidence/epoch contract. They do not share an assumed physical clock or reset guarantee. Route/format/device changes invalidate affected continuity and profile evidence; no fidelity fallback to a different output is implied.

`evidence/summary.json` distinguishes healthy captures, a native start stall, unavailable clocks, the corrected early-edit wait oracle and the one-frame reconstruction offset. Raw complete traces are retained with their clocks, commands and scoped ALSA observations. Sixteen guided model cases include long/short/tiny transitions, early/buffered/late/uncertain removal, seek, pause during preparation, repeat, Previous, EOF/drain, reordered/gain-mismatched events and a stalled output.

Actual implementation and physical-device/PipeWire downstream qualification remain acceptance obligations. These measurements are not a production Rust adapter, live Plex reporting, universal gapless timing or physical audio verification.
