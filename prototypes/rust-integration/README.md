# Rust / Quickshell integration feasibility prototype

**Throwaway planning evidence. Not the player, installer, or a production protocol implementation.**

Question: can a separate Rust backend, embedded libmpv and stock Quickshell provide authoritative session updates, bounded catalog pages, local artwork and backend-owned MPRIS without a custom native Quickshell plugin?

This question requires real Rust/QML/FFI/DBus boundaries. A standalone HTML simulation would not answer it, so this study deliberately uses a native client and a repeatable experiment runner instead of the generic logic-demo HTML format. No production module is created or copied onto main.

From the worktree root:

```sh
python3 prototypes/rust-integration/run.py
```

This builds the locked release binary, creates a private scratch directory and synthetic silent WAV, starts a loopback mock artwork provider, backend and native window, and waits for Ctrl+C. All sound goes to mpv's **null** output. The controls operate real application commands. No Plex account, credentials, system services, desktop settings, hardware audio devices, or persistent application data are used.

```sh
python3 prototypes/rust-integration/run.py --check
```

Runs bounded observation scenarios and rewrites `evidence/results.json`, the redacted startup log, and a standalone HTML/Markdown report. These are prototype experiments, not a production regression suite. Cargo's dependencies are downloaded on the first build; subsequent builds use Cargo.lock. Requirements: Rust/Cargo, Python 3, distro libmpv development/linker support, Quickshell with Qt Quick Controls Basic, a graphical Wayland session and its session DBus. The runner terminates only the processes it created. Scratch fixtures/logs remain in the printed `/tmp/qoda-rust-prototype-*` directory for inspection and may be deleted afterwards.

## Recommended fixture and budgets

Selected under the human's standing acceptance of recommended planning defaults:

- 100,000 fictional tracks, 2,000 artist labels, 10 tracks per album; 100 rows per displayed page, hard response cap 200.
- Local backend command acknowledgement and a warm local catalog search/page: 250 ms each. The forced 300 ms supersession scenario is excluded from normal latency targets.
- QML 16 ms heartbeat: maximum observed interval 50 ms during the sampled search burst, after startup. This measures main-loop responsiveness, not actual frame presentation or input-to-photon latency.
- Combined sampled backend/UI RSS: 512 MiB. Sum includes shared pages twice; samples are not continuous peaks or proof against leaks.
- These are this prototype's test targets, not production SLA claims. Results and failures must be reported as observed.

## What crosses the boundaries

- NDJSON protocol version 1; owner-only runtime directory/socket and same-UID peer check; 1 MiB inbound frame cap. Fragment/coalesce frames explicitly in the runner.
- Backend instance/epoch and increasing revision. Snapshot enqueue and subscription registration occur within one serialized actor turn. The QML client detects missed revisions and reconnects for a new snapshot; it never manufactures authoritative queue state.
- Pending actions are visible until acknowledgement. Request ID matches the reply; a separate operation ID identifies a logical edit. UI-instance nonce prevents collisions after UI restart. Operation-token results are cached for **at most 256 operations, one backend lifetime**. This is not durable exactly-once delivery: there is no blind automatic edit replay, and expired/unknown outcomes require reconciliation. The toy cache clears on reaching the bound; production must specify expiry/unknown-result handling before offering safe retry.
- Optional expected revision detects concurrent stale edits; an explicitly supplied old backend instance is rejected before applying an edit.
- Catalog scans run off the session actor. Per-connection generation cancels superseded scans at 256-row boundaries. The UI independently ignores replies whose request ID is no longer current. Results replace a bounded ListModel; they do not populate a 100,000-object QML model.
- Artwork is fetched by Rust using a fictional Authorization header against a fixed loopback-only provider, written to private local versioned PNG paths, and loaded by QML. Provider URLs/tokens never appear in the response. Versioned filenames demonstrate replacement without stale QML image caching; production cache invalidation/auth/TLS policies remain separate.
- One thread owns libmpv initialization, commands, borrowed event data, termination and destruction. Only copied Rust values leave it; no callbacks or FFI pointers cross threads. A small channel maps application play/pause intent into mpv commands. Actual mpv property events update QML state/position and the MPRIS mirror.
- Minimal MPRIS root and player interfaces use the deliberately distinct `org.mpris.MediaPlayer2.qoda_integration_prototype` name. Play/Pause/PlayPause route through the same actor; actual PlaybackStatus is signaled by mpv events. This is a feasibility subset, not a complete MPRIS implementation or the production desktop identity.

## Explicit limits

No real provider, TLS/network failure matrix, keyring, SQLite/checkpoint restoration, production persistence, install/service lifecycle, physical keyboard acceptance, gapless transition, real-time hardware output or DAC fidelity is established here. An abrupt backend restart deliberately resets the in-memory fixture queue and starts paused; the agreed production durable queue remains required.

The actor/control channels are intentionally simple and not a production ingress/backpressure design. Subscriber output is bounded; overflow disconnects the writer so a real client must resynchronize. Search and artwork fixture code is intentionally narrow. Production must bound/cancel in-flight provider work and avoid reusing the toy operation-result eviction policy as a reliability guarantee.

The native window's layout is a diagnostic surface. The accepted product layout and session behavior remain the decisions on the GitHub map. Automated scenarios use the application's own targeted IPC, not synthetic physical keyboard/mouse claims.

## Primary references

- [Quickshell Socket](https://quickshell.org/docs/v0.3.0/types/Quickshell.Io/Socket/) and [SplitParser](https://quickshell.org/docs/v0.3.0/types/Quickshell.Io/SplitParser/): delimited local socket messages; writes explicitly flush.
- [zbus service examples](https://docs.rs/zbus/5.19.0/zbus/): Rust service/interface ownership. Exact resolved dependency versions are in Cargo.lock; this is not a library-version support promise.
- [mpv C client header](https://github.com/mpv-player/mpv/blob/v0.41.0/include/mpv/client.h): event-pointer lifetime, property formats, client initialization and destruction. The installed header is the ABI reference used for the small FFI surface.
- [MPRIS Player interface](https://specifications.freedesktop.org/mpris/latest/Player_Interface.html): command/property meanings and change signals.
