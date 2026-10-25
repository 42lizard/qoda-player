# Qoda Player

A planned standalone music player for Omarchy, built with Quickshell/QML and mpv/libmpv. Plex is the first music provider; additional providers may follow.

## Status

Planning. No playable application exists yet. The current effort will produce an implementation-ready specification through a Wayfinder decision map on this repository's GitHub issue tracker.

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

## Planning workflow

The canonical map is [Find the way to the first Plex release specification](https://github.com/42lizard/qoda-player/issues/1). It carries the `wayfinder:map` label. Its child decision tickets use `wayfinder:research`, `wayfinder:grilling`, `wayfinder:prototype`, or `wayfinder:task`; native sub-issues and blocking relationships define the frontier.

Research notes live on separate `research/*` branches linked from their tickets. These branches are planning evidence, are not merged, and have no pull requests.

The selected interface direction is the album browser with a persistent queue, documented in [Choose library browsing and player interactions](https://github.com/42lizard/qoda-player/issues/6). Its throwaway comparison prototype stays on `prototype/library-interactions`; it is not production application code.

Playback actions, shuffle/repeat, restoration, recovery, and transition edits are specified in [Define listening-session and queue behavior](https://github.com/42lizard/qoda-player/issues/7). The UI prototype's illustrative queue behavior is superseded by that contract.

## License

MIT. See [LICENSE](LICENSE).
