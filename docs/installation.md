# User-scoped source installation

The installation contract for #33 uses a locked release build, versioned private
code, a stable launcher, desktop entry/icon and two on-demand user units. It adds
no login startup, socket activation, keybinding or Omarchy configuration. Native
dependencies remain distro-managed. Development validation targets isolated
prefixes and does not install this package on the active desktop.

Install and remove verify recorded file hashes, executable modes and the exact
current-release symlink. Modified, unexpected, symlinked or unowned artifacts
block the operation before changes. Reinstalling retains previous release
payloads. Switching releases or uninstalling requires both units to be stopped;
the installer never stops music. A failed filesystem transaction restores the
previous files. If rollback itself fails, it retains a marked staging directory
with the previous payload and copies of the integrations for manual recovery,
and prints that location. After an interrupted process, the next install/remove
reports the retained recovery directory; the launcher refuses activation until
it is reviewed. Quit remains available. Data, preferences, cache and keyring items are retained on removal.
There is no automatic schema rollback or data purge.

The launcher serializes user actions, starts the backend and waits for protocol
readiness before starting/showing the UI. Hide/status never start services. Quit
stops only these units; SIGINT gives the backend its existing final checkpoint
path. UI failure requires the next explicit launch to recreate it. Backend
failures have bounded service retries and restore paused. Upgrade activation,
real graphical logout and persistence-failure qualification remain explicit
release gates rather than assumptions from filesystem tests.

## Commands and prerequisites

Requirements match [development](development.md): Rust/Cargo 1.99.1 (release-build pin), Python 3.11+
(`tomllib`), current distro Quickshell/Qt Quick Controls, libmpv, ALSA, SQLite and
pkg-config. Saved sign-in additionally needs libsecret/Secret Service. The current
qualification target is Arch/Omarchy; these are not compatibility claims for
other distributions. The build uses `Cargo.lock`; `CARGO_TARGET_DIR` is respected.
`packaging/toolchain.txt` pins the compiler; preflight refuses a mismatch rather
than downloading tools. `release.json` records the compiler used. Build flags
remap home, source and target paths to neutral names in the binary. This does not establish byte-identical
builds across toolchain or distro versions.

```sh
python3 tools/install.py install
~/.local/bin/qoda show
~/.local/bin/qoda compact
~/.local/bin/qoda toggle
~/.local/bin/qoda hide
~/.local/bin/qoda status
~/.local/bin/qoda quit
python3 tools/install.py remove
```

`--prefix` defaults to `~/.local`; `--config-home` defaults to
`$XDG_CONFIG_HOME`/`~/.config`. Keep both choices for later upgrades/removal.
Installation runs `systemctl --user daemon-reload` but never starts/enables a
unit. The prefix's `bin` directory must be on PATH for terminal use. Desktop
launch uses its absolute path. Custom prefixes outside the desktop's XDG data
search path also need their `share` directory in `XDG_DATA_DIRS` for menu/icon
discovery; files are not copied to a second location automatically.

The installer stages `lib/qoda-player/releases/<version>-<content-hash>`, then
switches `current` and the recorded `bin`, `share` and user-unit artifacts. It
serializes with launcher actions. Retained releases are private installed code;
they are removed together on uninstall. The source checkout is no longer needed
to run the installed app. The optional bar widget remains separately installed; it can explicitly use the
[installed launcher](activation.md) with `--installed-prefix`.

The Wayland, runtime and session D-Bus environment must already be available to
the user service manager and match the invoking desktop. The launcher diagnoses
mismatches; it does not overwrite the manager's global environment. Units require
`graphical-session.target`, have no `[Install]` section, and stop with the
session. The backend uses `Restart=on-failure`, a two-second delay and a maximum
of three starts per minute. [Shutdown persistence failures](shutdown.md) have
distinct non-restarting exit statuses, exposed by Quit and Status. Intentional Quit stops units and cannot trigger
socket activation. The UI has no automatic restart. Commands are serialized by
an owner-only runtime lock. The existing backend lock adds independent duplicate
protection. An unmanaged development backend is refused rather than adopted.

## Qualification still required

Generated desktop entries and unit syntax are checked using the installed native
validators, including spaces, quotes, dollar signs, backticks and percent signs
in the prefix. A GIO launch regression also checks the actual desktop Exec path
and arguments, using a private test script and unavailable private D-Bus address. Isolated tests cover ownership, mode changes, future manifests,
upgrade/reinstall, retained releases, conflicts, injected write/reload failure,
stopped-only switching, exact-unit Quit, first-toggle behavior and peer/protocol
readiness. These do not prove an actual user manager's logout/crash behavior.

Before #33 closes, [#45](https://github.com/42lizard/qoda-player/issues/45) must qualify actual installed services in a disposable graphical
session: concurrent launch/Quit races, UI failure and explicit recreation,
backend crash retry/rate limiting, graphical logout, fenced MPRIS/installed-bar activation under the real manager, and final-checkpoint outcome propagation through the real user
manager. Process-level checkpoint/backup/cleanup exit reporting and launcher
classification are implemented and tested in [shutdown outcomes](shutdown.md). Existing schema incompatibility protection is backend-owned; this
installer does not offer code/database downgrade or automatic backup restoration.
Pause/checkpoint upgrades that retain a running session and explicit compatible
rollback remain [#46](https://github.com/42lizard/qoda-player/issues/46). Installation on the active desktop remains
opt-in after qualification.

`python3 tools/check-install.py` builds and exercises a real disposable package,
using a test manager adapter and a private empty backend. It verifies native
unit/desktop syntax and never reloads or starts units in the host user manager.
It requires `desktop-file-validate` and `systemd-analyze` in addition to the build
dependencies. No real account, credentials, library or audio is used.

The generated-unit test requires the full native Quickshell runtime because
`systemd-analyze verify` also checks executable availability. The Qt-only CI
image skips that native test; ownership, transactions, protocol and GIO fixtures
remain active. The full native installer check runs separately on Omarchy.
