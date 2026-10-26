# Omarchy palette integration

Qoda follows the active Omarchy palette across the full window, compact player,
settings, menus, browsers, transport and queue. Layout A's hierarchy and spacing
remain the same. Fonts continue to inherit Qt's desktop defaults where the UI
has no explicit size/weight.

The UI reads `~/.local/state/omarchy/current/theme/colors.toml`, with the older
`$XDG_CONFIG_HOME/omarchy/current/theme/colors.toml` path as a fallback (default
`~/.config`). It asks the installed `omarchy theme color --file … --all` resolver
for canonical colors, including legacy palette aliases. Background and foreground
must be valid RGB hex colors; accent falls back to blue, then foreground.
Surfaces, borders, muted text and selection tints derive from those colors. Accent
control text chooses black or white for contrast. Per-role control bindings keep
Qt buttons, inputs, sliders and popup controls synchronized with live changes.

File notifications plus a 2.5-second refresh handle in-place changes, atomic
replacement and files created after startup. The resolver runs only when file
contents/path change, with a two-second termination deadline and a subsequent
forced-stop deadline. Results for an outdated file snapshot are discarded. A
partial/unreadable palette or failed/unavailable resolver retains the last valid
palette; startup without a valid Omarchy palette uses the approved charcoal/green
fallback. No theme hooks or desktop configuration changes are installed.

Palette updates do not recreate the shell or backend, issue playback/queue
commands, or clear browser/selection state. Theme colors are presentation only.

`./tools/check.sh` includes dark/light live control binding, contrast, malformed
update and fallback QML tests. With Quickshell and Omarchy installed,
`python3 tools/check-layout.py --theme-check --output /tmp/qoda-theme-check`
uses private synthetic palette files, the actual color resolver and production
full-window UI. It checks legacy aliases, atomic directory-symlink replacement,
light/dark controls, partial-file retention, queue/browser stability and renders
both palettes without changing the desktop theme. Native compact/full lifecycle
checks remain in `tools/check-surfaces.py`.
