# Cross-library track search

The **Search** navigation entry searches tracks across the connected provider's
music libraries. Albums, Artists, Tracks and Playlists keep their existing
per-library scope. Search never changes playback by itself. Each result shows
its library and server, and identical provider track keys in different sources
remain separate. **Play now**, **Next** and **+ Queue** use the current result's
source-scoped identity and backend metadata.

Queries are limited to 512 characters and start after the normal 300 ms editing
delay or Enter. An empty query clears the view without requesting every catalog.
At most 32 distinct libraries are searched, four concurrently, with a six-second
per-library deadline and a 15-second total deadline. Each library contributes at
most its first 100 matches, and the displayed aggregate is limited to 500. Source
and aggregate limits are explicit; refine the query to narrow the scope. This is
track search, not an assertion of exhaustive albums/artists/playlists search or
whole-library pagination.

Results become usable as individual libraries respond. Source status shows
pending, refreshing, ready, cached, failed or timeout, including library/server
labels, bounded sanitized failure reasons, stale provenance and additional-result
indicators. An unavailable library cannot hide successful results from another.
Cached/stale/incomplete matches remain inspectable; actions wait for fresh,
source-validated data. Permission/identity denial removes the source's rows.
The existing private metadata cache supplies and stores per-library pages within
its independent budget; search creates no separate durable catalog.

New queries cancel old work. Account changes, disconnect/sign-out and metadata
cache clearing invalidate the search. Replies require the active account and
search generation. Actions require the current account, search revision and
queue session/revision, then resolve only displayed actionable result identities.
Client titles or old/hidden matches cannot select or queue media. Playback still
requires the normal pinned-version preparation and explicit play intent.

The library rail scrolls when settings or a small window leave too little height,
keeping Search and Account reachable. Result models remain stable across status-
only updates, preserving selection/scroll state. Narrow result rows retain source
labels and keyboard-accessible Play/Next/Queue controls.

Automated checks use fake providers and virtual time for partial results,
identity/permission fences, concurrency, deadlines, cancellation and limits;
QML tests exercise keyboard actions and retained models. Production offscreen
fixtures inspect wide/small search with duplicate keys and a failed source.
Native empty/private-session checks cover offline refusal and surface retention.
No Plex account access, real music or physical output is used for these checks.
