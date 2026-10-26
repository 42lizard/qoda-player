# Continuous conversion: short-track lookahead boundary

Accepted recommended default for #41, 2026-10-26, under the human's standing
delegation to accept recommended planning defaults. Retain one successor and use
explicit short/unknown/incompatible-input fallback. The production adapter now applies this boundary; implementation and digital/null
qualification are recorded in the [continuous-input evidence](research/continuous-pcm-input.md).
The expanded shared-core matrix matches 40 uninterrupted PCM references on the
measured stack, including unequal gains. Native adapter tests separately cover
rolling nominal handoffs, cancellation/reset and final graph output.

A paced-null follow-up supplies a one-second current source and only a **one-frame
successor**, with neither final EOF nor fabricated silence. While a third source
is withheld, the largest observed nominal position is **0.9157916667 seconds**,
below the one-second ownership boundary. Releasing the third input allows normal
completion. Command: `cargo run -p qoda-backend --example continuous_pcm -- --horizon`.
This is measured only for 44.1→48 kHz, SoXR precision 28 on the current stack;
it is not a universal latency bound or a physical-output result.

Continuous SRC needs future samples before it can produce the boundary. With a
sufficiently short successor, keeping exactly current plus one future grant and
waiting for nominal handoff before preparing another source can therefore stall.
Silently ending the stream or inserting silence would defeat the continuity claim.

## Decision: retain the existing one-successor access boundary

Use continuous conversion only for verified compatible source/output formats
with enough frames for the qualified lookahead horizon. The initial adapter uses
two seconds for current and next, qualified by rolling paced-null handoffs across
the supported rate directions and precision-only conversion. A shorter, unknown-length or incompatible next
source explicitly ends the continuous group and uses the ordinary independent
load. Show that fallback in the gapless status; do not claim continuous conversion
across that boundary. There is no additional provider access beyond current/next.
Seek, recovery, queue edits affecting committed input and output changes remain
explicit reset/rebuild boundaries with preserved occurrence/visit and pause fences.

## Considered alternative: broaden authorized lookahead

Prepare as many very short future occurrences as needed, subject to explicit
frame/occurrence/byte/time caps and account/source/queue revision fences. This
changes the existing one-successor grant policy and requires a multiple-boundary
ledger, cancellation/rebuild semantics for the entire admitted horizon, additional
provider access and tests. Hitting the cap still needs an explicit fallback; it
does not justify promising arbitrary continuous short-track runs.

Both choices retain the same SoXR quality/gain policy, exact-source preference,
experimental hardware labels and no automatic speaker playback. Neither marks
#41 complete before production integration and qualification.
