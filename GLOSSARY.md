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
The player's ordered listening session, including the current track and upcoming tracks. It is independent of saved playlists.
_Avoid_: Playlist

**Desktop playback**:
Listening that can coexist with audio from other applications.

**Fidelity playback**:
Listening intended to preserve decoded audio samples except for optional ReplayGain. The conditions under which this is guaranteed remain to be established.
