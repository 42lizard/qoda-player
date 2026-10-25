# Rust / Quickshell integration observations

Verdict: the separate Rust backend and stock Quickshell/QML route is feasible at this bounded fixture size without a custom native Quickshell plugin. **31/31 observations passed in the final run.** This is a feasibility result, not a production implementation or broad performance guarantee.

| Measure | Final observed | Prototype target |
| --- | ---: | ---: |
| Local backend command acknowledgement | 0.190 ms | 250 ms |
| Five warm catalog pages | 21, 35, 30, 74, 49 ms | 250 ms each |
| QML search-burst maximum heartbeat interval | 38 ms | 50 ms |
| Combined sampled backend/UI RSS | 338.1 MiB | 512 MiB |

Fixture: 100,000 fictional tracks, 2,000 artist labels, 10 tracks per album, 100 displayed rows and a 200-row hard page cap. The GUI does not hold the entire catalog. RSS sums shared mappings twice; it is sampled rather than a continuous peak. A heartbeat is a main-loop timer observation, not a frame or physical-input measurement.

The first completed run observed a 52 ms heartbeat and missed the 50 ms target. That failure is retained in `initial-run.json`. Model insertion was changed from 100 separate JavaScript append calls to a batch append. A subsequent run observed 40 ms; the final run including explicit teardown observed 38 ms. Short before/after samples do not isolate causality or establish tail-latency guarantees.

## What was observed

- Owner-only socket/runtime paths; incompatible-version rejection; fragmented and coalesced messages; snapshot plus racing updates without a revision gap.
- Pending/acknowledged UI commands; lost acknowledgement displayed as uncertain; snapshot reconciliation with no automatic replay; bounded same-instance operation-token duplicate prevention; stale-revision and old-instance rejection.
- Bounded catalog pages, superseded scan cancellation and UI request-ID rejection; forced revision gaps trigger reconnect/snapshot.
- Rust mock-authenticated artwork download, local PNG decoding in QML, changed local URLs for replacement, and no provider credential/URL in presentation state.
- Real Rust/libmpv initialization, event copying, play/pause/position updates, hidden UI progress, independent UI restart, a new paused backend epoch after crash, and successful owner-thread destruction/process exit.
- Backend-owned prototype MPRIS identity/properties and Play/Pause routed through the same command authority. Actual status is derived from mpv events.

## Boundaries and decisions

Keep the selected architecture. Use request IDs separately from logical edit IDs; scope edit IDs to a unique UI instance and backend epoch. On an uncertain outcome, refresh before offering a deliberate retry; the toy 256-operation cache is not durable exactly-once delivery. Subscribe/snapshot under one serialization boundary, and resynchronize on a missed revision. Keep catalog work off the session/UI threads and page the QML model; batch model insertion at this scale. Keep libmpv handles and borrowed event storage on their owner thread and send only copied events across it.

Production refinements required by this evidence: bound ingress and in-flight provider work; make slow-subscriber disconnect/resynchronization explicit; never imply safe replay after operation-result eviction or backend restart; keep wire revisions exactly representable by the QML client (use an explicit bounded-counter/epoch or string representation rather than copying the toy QML int indefinitely). This artifact does not qualify those limits under hostile/flood or long-duration load.

No durable queue was implemented: the backend crash fixture resets its in-memory queue and starts paused. Production queue/checkpoint restoration remains required by the existing contract. No real Plex, TLS/authentication matrix, keyring, service/installer lifecycle, full MPRIS compliance, physical keyboard, hardware output, gapless or DAC-fidelity claim follows. The existing stock-host portal registration warning persists and belongs to lifecycle/identity qualification; it is retained verbatim with runtime paths redacted.

## Captured checks

- PASS: private runtime and socket permissions
- PASS: incompatible protocol rejected
- PASS: fragmented and coalesced NDJSON frames
- PASS: atomic snapshot subscription bootstrap
- PASS: snapshot and racing updates have no revision gap
- PASS: pending command then acknowledged authoritative update
- PASS: QML lost acknowledgement becomes uncertain without replay
- PASS: same-instance operation token prevents duplicate
- PASS: concurrent stale revision rejected
- PASS: lost reply resynchronized without replay
- PASS: bounded catalog page from 100000 rows
- PASS: oversized page request capped at 200 rows
- PASS: cancelled old search cannot overwrite new results
- PASS: superseded search work notices cancellation
- PASS: backend authenticated artwork fetch and QML decode
- PASS: versioned local artwork URL invalidates image cache
- PASS: presentation state contains no provider credentials
- PASS: QML hidden while Rust libmpv keeps progressing
- PASS: mpv pause events reach QML and position stops
- PASS: MPRIS commands share backend authority and actual status
- PASS: disconnected UI resnapshots missed queue edit
- PASS: missed revision forces disconnect and fresh snapshot
- PASS: UI process restart preserves backend and audio
- PASS: backend restart changes epoch and never autoplays
- PASS: old-epoch mutation rejected after restart
- PASS: local command budget 250ms
- PASS: catalog page budget 250ms
- PASS: UI heartbeat interval budget 50ms
- PASS: combined sampled RSS budget 512MiB
- PASS: graceful libmpv owner-thread teardown exits cleanly
- PASS: no QML runtime error
