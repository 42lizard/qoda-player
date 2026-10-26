# Library selection and queue refinements

This completes the remaining scoped interaction work in #29 after the search
(#34), drag ordering (#35) and settings/focus (#36) increments. It preserves the
approved A hierarchy: navigation rail, album browser, persistent wide queue or
narrow drawer, and bottom transport. It is not a physical-input acceptance claim;
that remains #18.

The library picker shows each library with its server in a wider popup. Matching
library/server names get a shortest distinguishing ID prefix; selection always
uses the complete source ID. The narrow rail prioritizes that suffix when needed,
with full source context in the tooltip/accessibility name. Missing selections
are not silently replaced; an empty source list disables selection. A changed
library model closes stale choices while retaining selection by ID across reorder.

Mouse and keyboard choices work; Escape dismisses choices without hiding the
player, and Ctrl+K closes the popup and opens/focuses cross-library Search. A
source choice clears per-library queries and the temporary playlist seed, then
requests only the currently visible browser. It issues no playback or queue
mutation and leaves cross-library Search independent of the saved browse source.

Queue rows derive their height from content instead of reserving the old 108 px
minimum. The measured standard-font fixture uses 94 px healthy rows and smaller
inter-row spacing, while failed rows and larger fonts can grow. Artist/album
context, the Selected label/accent, Play/Retry, move buttons, Remove and the drag
handle remain available. Backend occurrence/item identities, revisions and
read-only/uncertain playlist restrictions are unchanged.

Validation uses Qt pointer/keyboard tests, production wide/narrow/short offscreen
fixtures with duplicate names, and isolated native full/compact lifecycle checks.
The fictional backend records exactly one source-scoped album-list request on
source change and no implicit playback or queue command. Capture completion is
acknowledged only after PNG encoding finishes, preventing a final capture from
being truncated during fixture cleanup. No real Plex or music is used.
