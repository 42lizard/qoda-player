# Qoda in the Omarchy bar

The optional `qoda.player` plugin adds a music-note icon to the bar. Click it to drop down
the compact controls anchored to that icon. Click again, press Escape, or click
outside to dismiss. The icon stays available while Qoda is hidden or stopped.
Its playing indicator follows confirmed backend playback status.

Expand restores the full browser. Queue opens its queue, and Ctrl+K opens Search.
If the frontend is absent, an explicit open action starts the development launcher
and retries the requested surface for about a minute. Opening the popup never
launches the player or starts music. A newly started backend restores paused and
does not import credentials. Quit stops Qoda; the bar icon remains available.

The existing floating Compact view is retained. Both it and the bar popup use
`ui/CompactControls.qml`; transport, nominal identity, position and capabilities
come from the same Rust backend. The widget subscribes over the private local
socket, preserves decimal counter strings, and never becomes a playback owner.
It does not advertise a browser Raise capability or react to browser Raise tokens.
No MPRIS active-player selection can redirect its controls to another application.
Reconnect clears stale presentation; requests use the current backend instance,
account epoch and queue session/revision. Popup placement and outside dismissal
use Omarchy's own `PopupCard`, and colors follow the live shell palette.

## Install and remove

This targets the current Omarchy Quickshell plugin API (validated on Omarchy
4.0.6). It is a widget-only development integration, separate from release
packaging (#33); the source checkout and normal development dependencies remain
required for opening the library.

From the checkout, build once and explicitly enable the widget:

```sh
cargo build --locked -p qoda-backend
python3 tools/bar-widget.py install --enable
```

It installs under `$XDG_CONFIG_HOME/omarchy/plugins/qoda.player` (default
`~/.config`). It uses the Omarchy CLI to add the bar entry on the right, before
Audio when that entry is present in the current user layout, and configure this
checkout as the player path. Other bar entries/settings are preserved. No root,
autostart/service installation, keyring access or shell restart is required.
The shell loads the new plugin through its normal plugin/config refresh.

The installer prints a bar-config backup under `omarchy/qoda-widget-backups/`.
Updating an owned payload also keeps the previous plugin there, outside plugin
discovery. Content-addressed QML bundles give changed widgets and their shared
controls fresh component URLs, avoiding stale icons after a plugin rescan.
Recorded hashes prevent replacement/removal of user edits, unexpected
files, symlinks, foreign installations or unknown installation records.

```sh
python3 tools/bar-widget.py remove
```

Removing an activated installation backs up the current bar config and uses
`omarchy plugin disable qoda.player` before deleting owned files. The disable
operation removes that bar entry; other settings and Qoda playback are retained.
Without `--enable`, installation only copies files. `--config-home <directory>`
supports isolated install/update/remove checks; it cannot activate another
profile against the running desktop.

## Checks

`./tools/check.sh` includes pure QML checks for shared compact controls, stale
frames, large counters, instance replacement, disconnected commands and confirmed
playing state, plus Python ownership/update/removal tests. Rust playback logic is
unchanged by this integration.

`python3 tools/check-widget.py --output /tmp/qoda-widget-check` loads the actual
plugin, installed Omarchy `PluginBarApi`, `WidgetButton` and `PopupCard` against a
fictional socket and browser. It checks icon-triggered open/dismiss, anchoring,
lossless command fences, reconnection, non-raising subscriptions and restoring a
hidden browser without a playback request. It changes no live bar configuration,
uses no Plex/credentials/audio, and stops its owned processes. Native floating
compact/full lifecycle and private MPRIS checks remain in `tools/check-surfaces.py`.
