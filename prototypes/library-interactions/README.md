# Library interactions — throwaway prototype

Question: which information hierarchy best supports browsing music while keeping playback and queue access clear?

This is a fictional, in-memory HTML stand-in for a future Quickshell/QML interface. It is not the production stack, does not play audio, and has no Plex access or persistent storage. Queue operations illustrate action placement; their policy remains undecided in the separate listening-session ticket.

Run from this branch's repository root:

```sh
python3 prototypes/library-interactions/serve.py
```

Open <http://localhost:8765/?variant=A>. Designs share the same route and data:

- `?variant=A`: album browser, navigation sidebar, persistent queue, bottom transport.
- `?variant=B`: artist/album/track columns, queue dialog, bottom transport.
- `?variant=C`: listening-first cover and transport, secondary collection pane and queue dialog.

Use the floating bottom switcher or left/right arrows outside form fields. The URL retains the variant; all music/player state resets on reload. The prototype is kept only on `prototype/library-interactions`, with no PR or merge to main. The variant switcher belongs only to this standalone study.

## Try

1. Browse an album; follow its artist; play the album and inspect the queue.
2. Search `Soft geometry`, switch to all libraries, and inspect the two distinct copies and the offline-source notice.
3. Open a track's actions and add it to Night bus. Open that playlist, move/remove one of its duplicate occurrences, rename it, or create/delete a disposable playlist.
4. Open Slow mornings and inspect read-only editing actions.
5. Switch Ready / Loading / Empty library / One server offline / Access removed / Playlist edit timed out in the study header.
6. Switch full → compact → hidden and reopen via the simulated shortcut. The playing state remains independent of visibility.
7. Navigate with Tab/Shift+Tab and Enter; use Ctrl+K for search, Space for play/pause outside controls, Escape to dismiss a dialog then collapse/hide the player.

8. Select tracks with their checkboxes or Ctrl/Shift clicks; apply bulk Play, Play next, Add to queue, or Add to playlist. Queue and editable playlist selections can be removed together. Duplicate playlist occurrences remain independently selectable.
9. Drag editable playlist entries or queue entries to reorder, or use their move buttons with the keyboard. Shrink the browser below 1,050 CSS pixels: A hides the right queue pane and keeps its Queue button/dialog.
10. Hide from full or compact, then simulate the toggle: it restores the last visible form. Escape closes dialogs and hides compact; it leaves full mode open. The player menu offers explicit Quit, simulated without closing the review page.

The expandable Prototype state below the application shows full fixture state, including independent playlist-entry identities and queue order. It is study instrumentation, not proposed application UI.

## Accepted direction

The human chose A and confirmed the interaction contract in [Choose library browsing and player interactions](https://github.com/42lizard/qoda-player/issues/6). B and C remain only as comparison evidence. A now reflects the selected responsive queue, selection/bulk actions, drag and keyboard ordering, and full/compact/hidden transitions.

The proposed native surface contract is a normal tileable full window plus a compact overlay on the invoking monitor, with no reserved desktop area and keyboard focus only when used. The HTML compact panel suggests placement inside its review surface; it does not establish actual native placement or focus behavior.

## Limits

No OS window/layer-shell behavior, global shortcut, real keyboard focus restoration across native windows, hardware audio, accessibility audit, large-library benchmark, network request, or backend persistence is implemented. The selected design and prototype coverage are distinct from native validation. JavaScript syntax and local HTTP delivery were checked; no automated browser surface was available for agent click-through or visual QA.

Decision ticket: [Choose library browsing and player interactions](https://github.com/42lizard/qoda-player/issues/6).
