# Qoda Player

Language for the music player and its music sources.

## Language

**Music provider**:
A source of music and its catalog, accessed through a configured connection. Accounts and servers are provider-dependent concepts; Plex is the first music provider.
_Avoid_: Storage backend

**Local storage**:
Information the player keeps on the user's device, such as cached metadata, settings, and application state.
_Avoid_: Music provider

**Playlist**:
A named, saved, ordered collection of playlist entries, owned by its music provider in a provider-defined context that need not be a single library. In the first release, playlists belong to Plex and edits are saved back to Plex.

**Playlist entry**:
One occurrence of a track in a playlist, with its own identity and position. The same track may occur more than once, and each occurrence can be moved or removed independently.

**Music library**:
A provider-defined collection used as a browsing context. Its contents retain their source identity when included in searches across libraries or servers.

**Track**:
A catalog item representing a song or other audio work in a particular source. Copies in different libraries or servers remain separate tracks, even when their titles and artists match.

**Album**:
A provider-defined grouping of tracks with its own identity, credits, and supplied ordering. An album's credits can differ from those of its tracks.

**Media version**:
One available media representation of a track. A track's catalog identity is separate from the selected representation and temporary playback access information.

**Prepared playback**:
The temporary access information needed to play a selected media version. It is separate from the enduring identity of the track and version.

**Artist credit**:
A provider-supplied attribution to one or more artists. Track credits and album credits are distinct, and missing credits remain unknown.

**Capability**:
An action currently supported for a source or item, such as browsing, playback, or a particular playlist edit. A cached capability does not guarantee that permission still exists when the action is attempted.

**Search result**:
A match within a selected search scope that retains the matched item's source and identity. Relevance belongs to that source; combined results can be grouped by source or sorted by an explicit common field.

**Partial results**:
Results for which some selected sources could not be searched or loaded successfully. They are distinct from both an unfinished page sequence and a complete successful result.

**Availability**:
The current known accessibility of a source or item, including unreachable, sign-in required, access denied, missing, or unavailable for an unknown reason. Losing access does not erase the item's identity.

**Uncertain edit outcome**:
A playlist edit whose success is unknown, such as after a response timeout. The playlist must be refreshed before offering a retry that could duplicate an already applied change.

**Artwork reference**:
A provider-supplied reference to optional artwork. It is distinct from a locally cached image and any temporary access URL.

**Gain information**:
Optional information about an audio level adjustment, with its provenance and units retained when known. Missing gain is unknown, not a verified zero adjustment.

**Queue**:
The player's ordered current and upcoming listening entries. It is independent of saved playlists; playing an album or playlist copies its order into the listening session rather than following later edits to the saved collection.
_Avoid_: Playlist

**Queue entry**:
One occurrence of a track in the listening queue, with its own identity and any selected media version. Repeated occurrences of the same track remain distinct.

**Current occurrence**:
The queue occurrence identified as nominally presenting. Preparing or decoding an occurrence does not establish it as current. If presentation cannot be identified, that uncertainty is explicit and the last confirmed occurrence remains recovery information.

**Playback visit**:
One presentation pass through a queue occurrence. Repeat or explicit replay starts another visit; seeking or recovering the same pass does not. Visit identity is separate from queue membership and the Previous navigation cursor.

**Nominal presentation**:
An estimate of which content has reached the output's reported playback position. It does not establish physical sound or listener audibility.

**Presentation uncertainty**:
An inability to identify presenting content or position from the available output evidence. It is distinct from playback intent and from a failure to load content.

**Listening session**:
The player's current collection of listening entries, their order, playback position, history, and shuffle/repeat state. Play now starts a new session; Stop and clear discards it.

**Listening history**:
The played entries of the current listening session, retained separately from the current and upcoming queue. It supports inspection and Previous navigation, and resets when a new session starts or the session is discarded.

**Listening archive**:
Retained nominal playback visits across listening sessions, independent of the current queue and Previous navigation. Observation positions do not establish audibility, completion or time spent listening.

**Play now**:
An explicit action that replaces the active listening queue with the chosen content and starts playback immediately.

**Play next**:
An explicit action that schedules content immediately after the current track. Repeated additions retain their request order and each selection's internal order.

**Add to queue**:
An explicit action that appends content to the end of the listening queue without deduplicating repeated tracks.

**Clear upcoming**:
An action that removes upcoming entries from the listening session while preserving the current entry and its playing or paused state.

**Stop and clear**:
An action that stops playback and discards the active listening session, including its queue and Previous history.

**Hide**:
An action that removes the interface from view while playback and the listening session continue. Closing the player window hides it.

**Quit**:
An action that stops playback and exits the player while retaining the listening session for a later paused restoration.

**Shuffle**:
A stable rearrangement of upcoming entries that leaves the current entry and listening history unchanged. Explicit Play next priority, appended content, and deliberate queue edits remain respected.

**Repeat mode**:
The session's automatic completion behavior: Off ends the queue, One repeats the current entry, and All cycles entries still belonging to the session. Explicit Next bypasses Repeat One, and removed entries do not return through repeat.

**Playback intent**:
Whether the user currently wants playback to proceed or remain paused/stopped. Buffering and recovery do not override a later pause, stop, or session replacement.

**Cached catalog**:
Locally retained metadata from previously accessed provider content. It may be stale or incomplete and does not imply that music is available for offline playback.

**Sign out**:
An explicit account action that stops affected playback and removes that account's credentials, cached catalog/artwork, saved listening session and retained listening archive. General application preferences remain.

**Confirmed access loss**:
Reliable provider evidence that previously accessible content is no longer permitted. A temporary network failure or expired sign-in alone is not confirmed access loss.

**Session-only sign-in**:
Account access whose authentication secrets are retained only for the running application session. It requires sign-in again after the backend exits.

**Desktop playback**:
Listening that can coexist with audio from other applications and supports ordinary application volume control. Output conversion may be part of the desktop audio path.

**Fidelity playback**:
Direct output that prefers decoded PCM sample values unchanged except for selected ReplayGain processing. Strict Fidelity refuses unsupported conversion; optional high-quality fallback permits explicitly reported rate or precision conversion without calling the result sample-perfect.

**High-quality fallback**:
An opt-in Fidelity policy that prefers an exact supported format, then permits high-quality conversion to a supported output format when necessary. It is separate from the choice to assume conventional stereo when a driver cannot report channel positions.

**ReplayGain mode**:
The selected loudness adjustment policy: Off, Track, Album, or Auto. Auto preserves album-level differences for an unshuffled album session and otherwise selects track-level adjustment, with missing-data fallbacks made explicit.

**Output verification profile**:
Evidence for a particular output path, format, package set, and processing configuration at a named measurement boundary. It does not imply verification of unmeasured downstream hardware or other configurations.
