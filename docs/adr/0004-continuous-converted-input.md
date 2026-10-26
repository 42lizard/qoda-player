# Keep compatible converted tracks in one input timeline

Status: accepted 2026-10-26; production adapter implemented 2026-10-27.
Independent mpv playlist entries reset the final DSP, while
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
short/unknown/incompatible input instead of widening provider access. The initial
adapter uses verified source descriptors and a two-second minimum,
qualified by rolling paced-null tests across the supported rate directions.
Seeks/recovery/output changes are explicit discontinuities; retain pause/visit and
pinned-version rules. The [synthetic/paced experiments](../research/continuous-pcm-input.md)
qualify the shared core and production post-negotiation adapter within their
documented digital/null scope; they do not qualify a physical output.
