# Keep compatible converted tracks in one input timeline

Status: accepted integration direction, 2026-10-26. #41 is not production-enabled
by this decision. Independent mpv playlist entries reset the final DSP, while
EDL requires a different trust/ownership model. Use independently owned source
decoders feeding bounded double PCM with per-occurrence gain into one final
libmpv/SoXR input, so the existing conversion policy stays in one persistent graph.

An isolated decoder process provides a kill/reap boundary for mpv's blocking PCM
writer; input and nominal timeline ownership remain Rust responsibilities. Retain
current plus one successor, integer source-frame spans, and the existing nominal
visit/history/reporting fences. Admission is conservative commitment, not listening;
canceling committed future input requires stop/reset and review/recovery rather
than pretending bytes already in the final engine were retracted.

The [measured short-track horizon](../continuous-conversion-boundary-proposal.md)
uses the user's delegated recommended default: explicitly fall back for insufficient
short/unknown/incompatible input instead of widening provider access. Qualify the
proposed conservative horizon and source descriptors before production enablement.
Seeks/recovery/output changes are explicit discontinuities; retain pause/visit and
pinned-version rules. The [synthetic/paced experiments](../research/continuous-pcm-input.md)
do not qualify a physical output or the remaining production adapter.
