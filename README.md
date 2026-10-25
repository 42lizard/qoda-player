# Qoda Player

A standalone music player in development for Omarchy, built with Quickshell/QML and mpv/libmpv. Plex is the first music provider; additional providers may follow.

## Status

The first working development slice has a Rust backend, Plex library browsing/search, a standalone Quickshell/QML window, embedded libmpv playback controls, and a backend-owned queue with a persistent side panel. Live read-only checks reached five music libraries and verified catalog display, search, hide/show and UI restart. Native playback checks use synthetic audio with null output; real-music playback and the full release remain under development in [issue 21](https://github.com/42lizard/qoda-player/issues/21).

Run `./tools/dev.py` to build and open it. Use **Save sign-in** after connecting, then **Restore saved sign-in** on later launches. On the original validation machine, `./tools/dev.py --use-validation-login` can still explicitly reuse the validation sign-in. Nothing autoplays. See [development instructions](docs/development.md) for dependencies, sign-in, controls, process ownership and current limitations.

The queue supports Add, FIFO Play next, independent duplicates, reorder/remove, Previous/Next, stable shuffle, Repeat Off/One/All, Replay, Clear upcoming and Stop & clear. Playback uses Desktop/PipeWire with ReplayGain off. SQLite checkpoints preserve the queue, selected occurrence, prepared version IDs and local position across backend restarts; restoration stays paused. The UI exposes save failures and Retry save, with daily consistent backups and explicit recovery. Plex playlists can be browsed, created, renamed, deleted and edited by entry ID; playlist playback takes an independent local queue snapshot. Smart playlists are read-only. The Albums tab provides paged title search, disc-ordered track details, authenticated cover artwork and independent album queue copies. The Artists tab supports paged name search and artist-scoped albums; album details link back to their artist. Visible album cards load cover thumbnails through revocable local URLs with a bounded memory cache. MPRIS exposes backend-owned desktop transport, seek, shuffle and repeat controls while the UI is hidden, with metadata gated by nominal playback evidence. Gapless chaining now stages one upcoming track in libmpv, with a session-only On/Off control. Compatible synthetic WAV and FLAC pairs produced exact concatenated decoded PCM; real Plex transitions and audio-device continuity still need qualification. A compact overlay shares backend transport and nominal playback state; Expand returns to the retained browser, and Hide/Show restores the last form. Persistent metadata/artwork caching and fidelity output remain outstanding. Run `./tools/check.sh` for formatting, Clippy and workspace tests.

## Agreed first-release scope

- Full library browser, search, compact now-playing panel, playback, playlist editing, and queue management.
- One Plex account, with selectable accessible servers and music libraries, including shared libraries where permissions allow.
- Plex playlists with changes saved back to Plex.
- A player-owned, editable queue restored across restarts with track and position, without automatic playback.
- Server-connected playback, with local metadata and artwork caching; offline music downloads come later.
- Desktop audio coexistence and a dedicated fidelity mode targeting unchanged decoded samples except for optional ReplayGain. Gapless playback and the precise output guarantees require investigation.
- Playback continues while the interface is hidden. Keyboard navigation, desktop media controls, and an Omarchy launch/toggle shortcut are included.
- A capability-aware music-provider boundary, checked against a hypothetical second provider; only Plex is implemented initially.

The player will use a modular music-provider layer for authentication, catalog metadata, and API access. Local storage is a separate responsibility. See [GLOSSARY.md](GLOSSARY.md) for the agreed language.

The selected architecture is an [independent Rust/Cargo backend with embedded libmpv](docs/adr/0001-independent-rust-backend.md), connected to the Quickshell/QML UI through a versioned local socket interface. The backend owns the session and desktop media controls, allowing playback to continue while the UI reloads or restarts.

## Planning workflow

The canonical map is [Find the way to the first Plex release specification](https://github.com/42lizard/qoda-player/issues/1). It carries the `wayfinder:map` label. Its child decision tickets use `wayfinder:research`, `wayfinder:grilling`, `wayfinder:prototype`, or `wayfinder:task`; native sub-issues and blocking relationships define the frontier.

Research notes live on separate `research/*` branches linked from their tickets. These branches are planning evidence, are not merged, and have no pull requests.

The selected interface direction is the album browser with a persistent queue, documented in [Choose library browsing and player interactions](https://github.com/42lizard/qoda-player/issues/6). Its throwaway comparison prototype stays on `prototype/library-interactions`; it is not production application code.

Playback actions, shuffle/repeat, restoration, recovery, and transition edits are specified in [Define listening-session and queue behavior](https://github.com/42lizard/qoda-player/issues/7). The UI prototype's illustrative queue behavior is superseded by that contract.

Credential handling, SQLite persistence, cache budgets, checkpoint/retry defaults, and recovery are specified in [Define authentication, cache and durable-state policies](https://github.com/42lizard/qoda-player/issues/10). Live Plex compatibility validation remains a separate prerequisite for the final specification.

[Live Plex observations](https://github.com/42lizard/qoda-player/issues/16) establish an owned-server baseline on PMS 1.43.5: explicit legacy browser authorization, catalog/search/artwork access, disposable-playlist CRUD, and original FLAC preparation/Range reads. The tested JWT path fails PMS authentication; shared permissions, reporting, recovery and the remaining acceptance cases are still unqualified, so this gate remains open.

[Player-side audio measurements](https://github.com/42lizard/qoda-player/issues/13) record decoded-sample and transition evidence from a synthetic libmpv harness. Hardware/DAC fidelity and real-device gapless continuity remain unverified.

The [fidelity and ReplayGain contract](docs/adr/0002-fidelity-and-gain-contract.md) selects desktop PipeWire playback by default, explicit direct ALSA fidelity output, and Auto ReplayGain with a high-precision per-file gain path. Native output and gain transitions require qualification before they receive a verified label.

[Native ALSA handoff observations](https://github.com/42lizard/qoda-player/issues/17) qualify a prototype profile at 48 kHz/S16/stereo on the selected built-in speaker PCM, with exact supported sample/gain comparisons and explicit unsupported-format guards. Downstream hardware fidelity and production Rust integration remain unqualified. [Playback occurrence timing and buffered edits](https://github.com/42lizard/qoda-player/issues/20) need a separate contract because decoder events can lead presentation.

[Playback timing and buffered-edit behavior](https://github.com/42lizard/qoda-player/issues/20) separates engine selection from nominal current presentation. Confirmed clocks and optional qualified predecessor spans drive metadata/history/reporting; missing clocks remain explicit. Buffered edits wait for a reset/rebuild boundary, with uncertainty retained where downstream retraction is unqualified. The throwaway model and synthetic traces do not establish a universal physical clock or live Plex reporting.

[Native Quickshell surface observations](https://github.com/42lizard/qoda-player/issues/14) establish tiled/full and compact-panel behavior through application IPC on the measured single-monitor setup. Physical keyboard dispatch, multi-output behavior, and scale changes require separate acceptance checks.

The [desktop lifecycle and distribution contract](https://github.com/42lizard/qoda-player/issues/11) selects on-demand user services, targeted launch/toggle commands, explicit Quit, paused crash recovery, and versioned source installs under a user prefix. The isolated qualification below records the tested mechanisms; no production package or desktop integration has been installed.

[Isolated lifecycle qualification](https://github.com/42lizard/qoda-player/issues/19) records 76 scoped checks and identical clean locked builds for service recovery, paused synthetic persistence, rootless release switching, rollback and manifest ownership. Stock Quickshell supports the product AppId; its portal registration warning remains, while the measured native-window flows produced no further portal crashes. The artifact uses null audio and private credential fixtures; production assembly, real providers, physical input and device integration keep their separate acceptance boundaries.

[Rust–Quickshell integration observations](https://github.com/42lizard/qoda-player/issues/15) demonstrate local IPC, bounded catalog models, mock-authenticated artwork, libmpv events and minimal backend MPRIS controls without a custom native Quickshell plugin. The throwaway artifact uses fictional data and null audio; durable restoration, live Plex, desktop lifecycle and hardware output still require their own acceptance gates.

## License

MIT. See [LICENSE](LICENSE).
