# Checkpointed release switching and recovery

Initial installation remains `python3 tools/install.py install`. It does not start
a player. The default `install` and `remove` refuse running units. To explicitly
upgrade an existing owned installation while it runs:

```sh
python3 tools/install.py update
```

Keep any `--prefix` and `--config-home` options used for installation. The build,
source compatibility evidence, copied release tree and ownership inventory are
prepared before acquiring the activation lock. Building does not pause playback.
Once activation begins, other launcher operations serialize or report that they
must retry. A launcher loaded from a superseded release refuses stale activation.

The updater checks loaded unit ownership and refuses drop-in overrides. It also
requires the manager and caller to use the same effective XDG configuration,
state, cache and data profile, so preflight checks the profile the backend will
actually open. `--config-home` locates the installed unit files; application
preferences follow the matching XDG profile environment. It stops
the owned UI, requests Pause on one pinned backend instance, and waits for native
pause/idle and a new successfully saved checkpoint serial. It then stops the
backend, verifies final shutdown/checkpoint outcome, and requires the socket gone.
A failed checkpoint stops the switch before any code selection. It does not
silently resume playback or rewrite the manager's desktop environment.

While both units are stopped, read-only format/health preflight checks the known
session, archive, metadata cache, settings, identities and retained session backup
files. Unsupported/newer, foreign, damaged, oversized or unsafe files and unresolved
SQLite recovery journals cause refusal and preservation. Preflight does not
initialize databases, migrate, change permissions or restore a backup. A stopped
`install` also performs this check before selecting code.

Linux `renameat2(RENAME_EXCHANGE)` atomically selects the complete prepared code
tree; there is no missing-current interval between two renames. Unsupported
exchange fails rather than falling back to a non-atomic update. Unit files are
reloaded and only components that were previously running restart. Backend
readiness must show a saved, paused session; the updater never issues Play or
Resume. A previously active UI is recreated in its full form. No previously
stopped backend is started and no autostart is enabled.

## Explicit code rollback

Retained release IDs are directory names under `<prefix>/lib/qoda-player/releases`.
Use an exact ID:

```sh
python3 tools/install.py rollback --release 0.1.0-<retained-content-hash>
```

Rollback requires recorded identical storage-reader/model/dependency evidence,
including the orchestration containing identity persistence, and compatible
on-disk format checks. `release.json` records relative source hashes and supported
format versions. The build refuses storage-source drift during compilation.
Identical version numbers alone are insufficient. This deliberately allows a
narrow compatible rollback, such as a UI-only change, and refuses many otherwise
plausible backend downgrades. Older releases with no evidence are refused.
Malformed domain invariants are still checked by the backend's existing
fail-closed typed readers; file-format preflight is not a universal semantic
validator or a promise that every preference will work in an older build.

Rollback never rewrites a database to an older format. If readers/models differ,
newer data and retained releases remain available. The command requests the same
pause/checkpoint/stop discipline and restarts only previously running components,
paused, after successful selection.

## Failed activation and interrupted operations

Filesystem/write/reload failure restores previous verified files. If new code
has not run, the previously running old release can restart paused. If new code
ran, automatic old-code restart additionally requires identical-reader evidence
and current data preflight. Otherwise previous code is restored but launch stays
blocked by a recovery marker; the failed candidate and newer data are retained.
The error gives the recovery directory. No guessed downgrade or silent backup
restoration takes place. Failure to stop newly activated units also retains code
and recovery files without replacing their running payload.

A process interruption can retain `.qoda-stage-*/recovery-needed` under the prefix
`lib` directory. Launch refuses that incomplete transaction; Quit remains
available. After stopping the owned player, explicitly recover verified previous
code with the exact reported directory:

```sh
python3 tools/install.py recover --recovery-dir /absolute/prefix/lib/.qoda-stage-<id>
```

Recovery verifies the recorded previous inventory and integration copies, accepts
only unchanged owned old/new external files, requires identical storage evidence
and data preflight, and leaves all services stopped. It handles interruption
immediately after atomic exchange, including before the old tree is renamed to
its backup location. The displaced candidate is retained for review with a
`recovery-complete` marker. It does not restore session/archive state or discard
newer data. Modified/missing/unqualified recovery material or an interrupted
recovery itself can still require manual review; all evidence is preserved and
no force/erase option is provided.

Session backup restoration is a separate explicit backend operation, never an
update/rollback fallback. It replaces the selected session checkpoint with an
older snapshot: queue edits, position and visit state after that snapshot can be
lost, while the independent archive is not rolled back. The original database is
preserved by the existing recovery mechanism and newer schemas are refused.
Review [session recovery and shutdown](shutdown.md) before invoking it.

Automatic session-backup retention also validates recovery copies before pruning;
unknown/newer/corrupt backups remain intact and produce a backup warning.

## Evidence boundary

Disposable-prefix fixtures cover staged builds, pause/checkpoint/stop ordering,
unit ownership/override refusal, compatible and refused rollback, newer data,
write/reload/activation failure, post-exchange interruption, verified code recovery,
and concurrent old launch/Quit behavior. A production libmpv null-output fixture
plays an original synthetic tone and proves the installed Pause handshake waits
for native pause and a new committed serial while retaining the same visit.
`tools/check-install.py` builds a real private package, starts only owned empty
native backend processes through a test manager adapter, switches release paths,
rolls back compatible code, verifies fresh paused instances and checks native
unit/desktop syntax. No host services, credentials, Plex or speakers are used.
These checks do not qualify actual user-manager/logout/crash behavior, physical
output or power-loss durability; installed service integration stays experimental.
