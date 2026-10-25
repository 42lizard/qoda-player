# Isolated lifecycle qualification

Throwaway acceptance artifact for [Validate user-scoped lifecycle and packaging](https://github.com/42lizard/qoda-player/issues/19), derived from Rust integration prototype `9fd6df5`. This is not a production installer or player and must not be merged.

Final result: **76 scoped checks passed** (43 lifecycle, 33 extended), two clean locked builds produced identical hashes, and the real-session window smoke passed with no new portal coredumps. The retained registration warning is described below. The consolidated evidence is `evidence/qualification.json`. Earlier failures and interrupted runs remain in the evidence directory; a passed prefix is not a completed run.

## Reproduce

```sh
python3 prototypes/lifecycle/qualify.py
```

Requires the running user's systemd manager and graphical session, Python with Gio/GLib bindings, Cargo/Rust, Quickshell, libmpv/libsqlite3, D-Bus tools, GIO and `desktop-file-validate`. The command performs two clean offline locked builds in the same absolute build root, compares binary hashes, then runs the core, extended and real-session window smoke scopes. Offline builds require the locked dependencies to be cached. This is a same-host/toolchain/build-path reproducibility observation, not a cross-platform or arbitrary-build-path guarantee. Exact distro versions are in `evidence/environment.txt`.

Build output is under ignored `.scratch/lifecycle-clean-build`. Each test scope creates a private temporary prefix, data/cache/state/config directories, Unix socket, bus and synthetic silent WAV with `ao=null`. Uniquely named temporary runtime user units are installed in `$XDG_RUNTIME_DIR/systemd/user`, then stopped and removed. The real graphical-session target is never stopped; a disposable target tests systemd lifetime propagation. No root operations, package installation, real provider credentials, real audio, active bindings or persistent desktop configuration changes are involved.

The private run directory and synthetic local logs/database remain for diagnosis. Published summaries redact its path and home/runtime paths. The test processes, bus and runtime units are cleaned up before exit. Do not run multiple lifecycle studies concurrently. `study.py --case startup-race` provides a focused check; `extended.py` exercises packaging/failure boundaries against an existing build selected with `QODA_BUILD_TARGET`.

## Portal incident and correction

Read [portal-incident.md](evidence/portal-incident.md). The first private buses loaded standard activation directories, which started extra portal processes. Five extra Hyprland portal processes crashed during shutdown. The document portal also exited at first startup and was restored. This was a harness isolation defect with user-visible side effects.

The corrected private bus has no activation directories. Mandatory preflight actually attempts portal/document/keyring activation and verifies ServiceUnknown before starting Qt. Host portal states/PIDs are compared before and after each scope. This prevents the study trigger; it does not fix the portal's underlying teardown defect.

A separate bounded smoke check runs a fictional native window on the **existing real session bus**, requiring all host portals to be active beforehand. It records app ID, actual mapping, hide/show, clean exit, unchanged host portal PIDs and new Hyprland portal coredumps. The application-ID registration warning persists, but the measured native-window flows succeed without portal crashes. No file chooser, screencast, document export or other portal feature is qualified. Browser authorization and its fallback remain with the live Plex gate; successful window mapping must not be treated as proof of OpenURI support.

## Evidence scopes

| Area | Executed boundary | Limit |
|---|---|---|
| Build | Two clean Cargo.lock/offline builds, same build path/toolchain, binary comparison | Other stacks/build paths unqualified |
| Launch | Tool preflight, eight concurrent requests, runtime flock, private socket, readiness/protocol/version errors, graphical environment | One user/session; no simultaneous seats |
| Visibility | Show/hide/compact/toggle, missing-UI hide, UI crash/reload, reconnect to live authority | Physical keys/focus and monitor matrix belong to native-input task |
| Recovery | Real systemd failure restart, three starts per minute, explicit retry, paused SQLite queue/position restoration | Synthetic minimal session, not the production session schema |
| Quit | Current-process readiness record, startup cancellation, activation races, rejection of new Rust commands, owner-thread join, persistence failure/timeout | Null libmpv; no native-device waveform qualification |
| Session lifetime | Optional hidden/paused start, same-session suppression after Quit, new disposable-session start, target stop propagation | No logout of the human's active session |
| Desktop | Product AppId in public Qt state and actual compositor class, desktop-file launch, icon lookup | No permanent menu/binding installation |
| Media | Backend MPRIS identity and authority, stock Quickshell client with two independent private players, selected routing, Raise/Quit, Rust unity-volume guard | No commands sent to unrelated real players or global media keys |
| Credentials | Private missing/locked collection, scoped item lookup/delete, incomplete locked purge, explicit fixture unlock/retry, unrelated-item retention | Fake metadata only; no real secrets, vendor prompts or host keyring changes |
| Update | Staging while playing, pause/checkpoint/stop, atomic release pointer, matched UI/backend, real bad-UI activation failure, explicit recovery | Test versions share one Rust binary; no production migration implementation |
| Rollback | Compatible code rollback, newer-schema refusal with unchanged pointer/data, explicit SQLite backup recovery with discarded-occurrence report | Synthetic schema transition and backup; no real account data |
| Removal | Manifest files/symlink, runtime/config integrations, modified/unowned/escaped paths, preexisting conflicts, default data retention, explicit item/data/backup purge and report | Only disposable roots and private fixture items |

## Findings that affect implementation

- Use stock Quickshell's `//@ pragma AppId qoda-player`; retain a separate stable ShellId. Both public appId and compositor class were observed as `qoda-player`. No custom fork/plugin or title/class-based activation is needed.
- Protect positional IPC arguments with `quickshell ipc ... call -- ...`. An unprotected argument `show` selected the sibling introspection command and returned success without activating the UI. The controller now requires a structured acknowledgement.
- An explicitly stopped unit may already be unloaded when `reset-failed` runs. Ignore only that specific error after confirming an owned unit file and inactive state; preserve other failures.
- A current-process readiness record precedes mutable actor command acceptance. Shutdown distinguishes cancelled startup from failed persistence; stale sockets and old readiness records do not establish readiness.
- Orderly Quit asks the Rust actor to checkpoint/shut down and joins the libmpv owner before stopping services. New mutable commands are rejected while Quit is pending. Failure paths stop the service and expose the persistence error rather than claiming a save.
- Hide does not create an absent UI. Quit suppresses optional login startup for the current graphical lifetime, while explicit manual launch remains possible.
- Stock host identity, portal registration, MPRIS identity, desktop filename and shell identity are separate observations. The persistent portal warning is not evidence of a crash, nor proof that portal features work.

The prototype is deliberately not a full Session/provider/storage implementation. It omits the production schema, provider work, authentication, cache policies and complete MPRIS compliance; its revision-based occurrence allocator is not qualified for new-entry uniqueness across restored sessions. Production uses the already-decided occurrence/session model and decimal-string protocol counters. Requalify the assembled production player rather than transplanting this fixture as its session implementation.

Required real-provider credential/prompt/sign-out checks remain with live Plex validation. Physical input and additional outputs remain with native-input acceptance. Device/sample/gain claims remain with the audio profile and require fresh production integration measurements. These boundaries are explicit and must not be converted into production pass claims.

The private Secret Service fixture follows the [item deletion](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Item.html) and [locking/lookup](https://specifications.freedesktop.org/secret-service/latest-single/) API boundaries; it does not emulate secret transfer or real user prompts.
