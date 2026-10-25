# Qoda Player

Language for the music player and its music sources.

## Language

**Music provider**:
A source of music and its catalog, accessed through an account or connection. Plex is the first music provider.
_Avoid_: Storage backend

**Local storage**:
Information the player keeps on the user's device, such as cached metadata, settings, and application state.
_Avoid_: Music provider

**Playlist**:
A named, saved, ordered collection of tracks. In the first release, playlists belong to Plex and edits are saved back to Plex.

**Queue**:
The player's ordered listening session, including the current track and upcoming tracks. It is independent of saved playlists.
_Avoid_: Playlist

**Desktop playback**:
Listening that can coexist with audio from other applications.

**Fidelity playback**:
Listening intended to preserve decoded audio samples except for optional ReplayGain. The conditions under which this is guaranteed remain to be established.
