# Isolated lifecycle qualification — incomplete

Throwaway acceptance artifact for [Validate user-scoped lifecycle and packaging](https://github.com/42lizard/qoda-player/issues/19). Derived from Rust integration prototype `9fd6df5`. This is not a production installer or application and must not be merged.

The latest guarded run completed **37 scoped checks** before a harness error resetting an unloaded backend unit. Acceptance remains incomplete. See `evidence/results.json` for the exact completed prefix and error; initial failures and interrupted runs are retained.

## Portal incident

**Read [portal-incident.md](evidence/portal-incident.md) before running this study.** The first private buses loaded standard activation directories and started additional portal processes. Five Hyprland portal instances crashed during shutdown. The document portal also exited at first startup and was restored. This was a harness isolation defect with a user-visible side effect.

The corrected bus explicitly has no activation directories. `portal_guard.py` actively tests that portal/document/keyring activation requests are denied and records unchanged real host portal processes. The lifecycle harness runs that check before starting Qt and checks host services again after cleanup. The guarded run produced no additional coredumps and left the host portals unchanged. This prevents the study trigger; it does not fix or fully diagnose the portal's underlying teardown defect.

## Run

From the repository root:

```sh
python3 prototypes/lifecycle/portal_guard.py
python3 prototypes/lifecycle/study.py
```

Requires the running user's systemd manager and graphical session, Python, Cargo/Rust, Quickshell, libmpv, libsqlite3, D-Bus tools and `desktop-file-validate`. Builds with Cargo.lock. The default build target is ignored `.scratch/lifecycle-target`; `QODA_BUILD_TARGET` may select an existing Cargo target directory. Do not execute concurrently with another lifecycle study.

Each run uses a private temporary root, prefix, data/cache/state/config directories, Unix socket, private bus and synthetic silent WAV with `ao=null`. It installs uniquely named **temporary runtime** user units in `$XDG_RUNTIME_DIR/systemd/user`, reloads the manager and stops/removes those exact units afterwards. The real graphical target is never stopped; a disposable target tests stop propagation. No root operations, package installation, real provider credentials, real audio, desktop bindings or persistent user-unit configuration are involved. The private root and synthetic local logs/database remain for diagnosis; the public summaries redact its path and the home/runtime paths.

## What was observed

The real user service manager exercised serialized/coalesced concurrent launches, a backend runtime flock, private socket permissions, readiness, protocol mismatch, explicit graphical environment delivery, hide/compact/toggle behavior, UI SIGKILL without backend interruption, backend SIGKILL recovery with a SQLite queue/position checkpoint restored paused, actual private-bus MPRIS identity/media routing, Rust-owned null-libmpv volume and stale-generation guards, explicit persistence errors and bounded Quit, no automatic resurrection, Stop and clear, staged update and compatible rollback, MPRIS Raise/Quit outside the backend service cgroup, optional hidden/paused startup and disposable graphical lifetime shutdown. A newer schema was refused without a restart loop.

A real SQLite writer lock produced the checkpoint failure. The volume fixture only uses null audio: it establishes Rust guard/event ownership and does not qualify a fidelity audio device or ReplayGain samples. The source releases use the same test binary with distinct release identities and QML versions; this proves the switching mechanism, not migration compatibility between independently developed versions.

The installed stock Quickshell accepts `//@ pragma AppId qoda-player`: both its public appId and the compositor client class were observed as `qoda-player`. `ShellId` is a separate stable study identity. No custom fork, native plugin, title/class-based activation or desktop rule is used. Desktop file/action syntax and a matching SVG icon were generated/validated inside the isolated prefix; actual desktop launcher/icon presentation remains unverified.

A CLI pitfall was reproduced: `quickshell ipc ... call lifecycle ... show` is parsed as the sibling `show` introspection subcommand unless arguments are protected. Earlier successful exit codes therefore did not imply activation. The controller now uses `call -- ...` and checks a structured UI acknowledgement. The original startup syntax error and activation timeout remain in evidence.

## Outstanding qualification

The task stays open. Remaining work includes:

- Complete the final run after making the harness's `reset-failed` step handle already-unloaded units explicitly; then observe restart limiting, explicit recovery and Quit/startup races end to end.
- Independent locked-build reproducibility, dependency failure/preflight behavior, relocated installation and ownership-conflict cases.
- Actual locked Secret Service/session-only flows; the private bus establishes only missing-service behavior and never accesses the user's keyring.
- Deliberate UI/backend version mismatch, UI reload, readiness/checkpoint timeout cases and provider-work cancellation under shutdown.
- Stronger update failure reporting, incompatible-schema rollback refusal plus backup recovery and measured data-loss accounting. The prototype has no production migration or installer implementation.
- Manifest-owned uninstall/conflict preservation, default data retention, explicit fake-credential/backup purge and remaining-dependency/data reporting. The study's own cleanup is not qualification of a production uninstaller.
- Actual desktop entry/icon launching and media-player selection alongside other players; real session-bus portal feature compatibility. The no-activation private bus cannot establish portal compatibility.
- Production Rust integration of the direct-hardware guard profile and its regression measurements; this study uses null audio only.

Physical keyboard, pointer focus acceptance and additional displays remain with the separate native-input task.
