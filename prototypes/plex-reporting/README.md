# Throwaway live timeline validation

Validation branch only: **do not merge or install this build**. It changes the
backend startup driver to `ao=null`, enables both reporting gates, and adds
sanitized request/result instrumentation. Production main remains gated off.
The harness starts the real coordinator/provider/libmpv on private temporary XDG
state, with no UI or working session bus. It reads only the dedicated validation
credential, passing it over stdin. No credential enters arguments, environment,
repository files or public evidence.

The user delegated selection of an owned-library track and required preserving
it. The initial plan executed two runs, each with a 60-second watchdog. The
selected original item was 19,737 ms. No track, playlist, server queue, credential
or listening-history item was deleted; there was no scrobble or metadata-edit
request. Original audio was digitally rendered through null output. No audio
capture is published.

## 2026-10-26 result

PMS `1.43.5.11029-6bf8af3f0`, verified HTTPS, one owned library/item. Seven exact
provider POSTs returned 200 with no termination information:

- Run 1: playing 434 ms, paused 2,433 ms, playing 2,699 ms, stopped 5,195 ms.
- Run 2: playing 268 ms, periodic playing 10,254 ms, stopped 19,360 ms.

The second stop retained the last qualified nominal clock, rather than inventing
19,737 ms from duration or treating native EOF as heard completion. The runs took
13.60 and 23.60 seconds including reporting drain. The coordinator quit with
status 0 and temporary private state was removed.

The session read showed the matching track/client paused during run 1, and no
matching session after explicit stops. The early playing observation had zero
matching sessions: **a live Playing session observation was not obtained**.
Metadata fields `viewOffset`, `viewCount`, `lastViewedAt` remained absent, and the
exact item/library history query returned zero rows before and after. This is an
observed absence, not zero-valued metadata, proof that history should change, or
a completion threshold. Do not add scrobbling to manufacture an expected result.
History-to-device attribution remains unqualified.

[Redacted evidence](evidence/run.json) distinguishes requests, session state,
metadata fields and history. No title, artist, server/library name, address,
rating key, account/client/session ID or raw response is exported. See
[primary API observation research](../../docs/research/plex-live-observation-apis.md).

## Reproduction

Requires an explicitly scoped live test: timeline calls may change resume,
played-state or history. Do not blindly rerun; the initial two-run budget is
already consumed. Build this branch into its separate target:

```sh
CARGO_TARGET_DIR=/tmp/qoda-reporting-validation-target cargo build --locked -p qoda-backend
python3 prototypes/plex-reporting/probe.py
```

Keep this branch out of main. Follow-up cases need their own bounded plan and
record; remaining #16/#26 acceptance is not implied by seven successful POSTs.

## Handoff follow-up and diagnosis

The initial handoff probe observed a distinct native occurrence/render and a
Playing predecessor, but its successor session sample was empty. It stopped and
retained [the incomplete result](evidence/followup.json), without qualifying it.

The subsequent [bounded diagnostic](diagnostic-plan.md) used the same uniquely
reidentified intact item. [Dispatch timing](evidence/diagnostic.json) showed both
reports queued at 19,906 ms: old stop completed at 22,432 ms and successor playing
completed at 25,032 ms. Capability discovery alone took about 2.4–2.6 seconds per
call. At the later observation, the successor's matching Playing session existed.
No ownership loss was demonstrated; the earlier early sample is consistent with
this approximately five-second serialized discovery delay. The 35.67-second
diagnostic completed with clean exit and no metadata/history changes observed.

Temporary trace instrumentation is validation-only and contains state/position,
monotonic elapsed time and sanitized typed outcomes, never headers/URLs/IDs.
Caching validated source/provider capabilities is the next measured optimization;
it must not increase the three-second request deadline or weaken identity fences.
