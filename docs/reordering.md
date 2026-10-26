# Queue and playlist reordering

Drag the dedicated handle to move an ordinary upcoming queue occurrence or an
editable plain-playlist item. The line above/below a target shows its insertion
position; **Move to end** is available during a drag. The existing up/down buttons
remain keyboard accessible. Queue occurrences and playlist items have independent
identities, so existing duplicate occurrences remain separate. Plex controls which
tracks it saves: the measured owned server collapsed duplicate additions, including
imports from a duplicate-preserving server queue. Qoda does not assume every Plex
version behaves the same. When an append of an existing track returns the same
ordered playlist entries, Qoda explains that another copy was not confirmed and
keeps the explicit-refresh fence. It sends no automatic retry. Use the local queue
for repeated tracks; a confirmed distinct playlist entry remains supported.

The retained owned-server observations are on the
[`validation/plex-live` branch](https://github.com/42lizard/qoda-player/tree/9ef8a40/prototypes/plex-live/evidence)
(`playlists.json` and `duplicate-queue.json`). The latter does not establish cleanup
of the temporary source queue; it does establish removal of the imported test
playlist. These limits are separate from the deterministic occurrence-edit tests.

Selected/traversed/pinned queue entries cannot be dragged or used as ordinary
anchors. Smart/read-only playlists, a busy playlist operation and an uncertain
write disable editing. An uncertain write still requires explicit refresh and
inspection. Playlist moves save to Plex through the existing provider operation;
queue moves affect the local listening session.

A gesture captures backend instance, account, resource/session, revision and
item identities. Changes to those identities, eligibility, visibility or viewport
size cancel it. A per-gesture token prevents late release/destruction callbacks
from adopting or cancelling a newer gesture for the same item. The backend
continues to validate the captured resource revision and metadata; the UI never
optimistically changes membership/order. Searching, dragging, hovering and edge
scrolling do not start playback.

Only the handle grabs pointer movement, preserving ordinary list scrolling.
Edge scrolling runs at bounded steps while the pointer remains near the viewport;
the current source delegate is retained beyond the ordinary cache window. The
ghost stays in window/overlay coordinates. Escape cancels a drag; an idle Escape
can then dismiss the queue drawer while the full player remains visible.

Drop handlers validate and store a proposed destination only. The source sends
one typed backend command after Qt's drop delivery returns, avoiding drag-state
changes inside drag events. See [Qt contracts and source-backed limits](research/qml-drag.md).
The internal drag has no native file/URL payload and cannot move an item into a
different controller/resource.

Validation uses Qt 6 offscreen/software pointer and keyboard events for accepted
moves, Escape, stale revisions, modal overlays and stationary-pointer scrolling
across multiple viewports. Production fixtures validate exact queue/playlist
request fields, unchanged displayed order until backend updates, duplicate item
identity and disabled smart/read-only/busy/uncertain edits. Wide/narrow captures
inspect the handles and responsive playlist controls. Native empty/private-session
surface lifecycle remains separate from physical input acceptance (#18). No real
Plex writes, music, physical output or desktop configuration changes are needed.
