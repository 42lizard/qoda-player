# Plex capabilities and integration constraints

Evidence reviewed: 2026-10-25. Resolves [Establish Plex capabilities and integration constraints](https://github.com/42lizard/qoda-player/issues/2).

This is public-documentation research, not a live-server compatibility test. No credentials, account data, private server, or media were accessed. Recommendations below are planning inferences, not implementation decisions.

## Conclusion

The first-release scope has enough documented integration surface to proceed to domain and architecture decisions. Treat authentication, playback preparation, playlist entry identity, and optional loudness data as explicit responsibilities. Do not promise that every accessible library or playlist is writable, that a raw stream URL is permanently usable, or that Plex analysis guarantees a complete ReplayGain pair.

## Current server contract

The [official PMS reference](https://developer.plex.tv/pms/) documents version negotiation through `X-Plex-Pms-Api-Version`; omission selects legacy `0.0`. The downloaded reference identified itself as `1.2.3`.

Its recommended authentication uses PIN/browser authorization, device keys, seven-day JWTs, and nonce-based renewal. Requests retain `X-Plex-Token`.

| Capability | Documented surface |
| --- | --- |
| Libraries | `GET /library/sections/all` |
| Browsing | `/library/sections/{sectionId}/all`; artist → album → track hierarchy |
| Details | `GET /library/metadata/{ids}` |
| Search | `GET /hubs/search`; section context, typed hubs, per-hub limit |
| Artwork | Returned artwork keys; metadata artwork GET |
| Playlists | `/playlists`, playlist detail/items CRUD |
| Reorder | `PUT /playlists/{playlistId}/items/{playlistItemId}/move` |
| Original media | Returned part key under `/library/parts/...` |
| Playback state | `POST /:/timeline` |

Playlist entries have separate `playlistItemID` identities. Smart playlists differ from plain lists; providers can advertise read-only playlists; inaccessible playlists may return 404.

Media streaming should establish a playback decision: implicit decisions may disappear. Timeline reports accompany state changes and periodic playback updates. Track `index` and `parentIndex` are documented. The public schema does not establish `albumGain`/`albumPeak` availability. These facts come from the same [PMS contract](https://developer.plex.tv/pms/).

## Browser sign-in and credential lifetime

Plex's [first-party native-app authentication guidance](https://forums.plex.tv/t/authenticating-with-plex/609370) describes a stable client identifier, creating a time-limited PIN, opening Plex's browser authorization page, and polling until authorization succeeds or expires. The application does not need to collect the user's Plex password. That article dates to 2020 and describes the older token flow; use the current contract above when choosing new-app credential handling.

**Planning inference:** the architecture decision must account for browser launch, asynchronous polling/cancellation, a device key, renewal, expired/revoked authorization, and a stable application identity. The credential-storage decision should cover both the private key and returned tokens. Before implementation is accepted, verify these flows against the supported PMS versions and an authorized test account; public documentation alone does not establish compatibility with the user's installation.

## Discovery and access

Plex [documents account resource discovery](https://support.plex.tv/articles/206721658-using-plex-tv-resources-information-to-troubleshoot-app-connections/) through `plex.tv/api/resources`, returning associated servers and connection information. This is discovery evidence, not proof that every advertised address is reachable.

[Managing Library Access](https://support.plex.tv/articles/201105738-creating-and-managing-server-shares/) confirms per-server and per-library grants, invitation acceptance, editing/removing access, and remote-access prerequisites. [Access restrictions](https://support.plex.tv/articles/204232573-restricting-the-shares/) can further filter visible content. [Single Item Access](https://support.plex.tv/articles/shared-media/) also includes albums, tracks, artists and playlists, and restrictions still apply to their contents.

**Planning inference:** keep account, server, library and item identity distinct. Scope cached identifiers to their source; do not deduplicate solely by title. Discover usable endpoints and fetch authorized content rather than presenting a cached catalog as proof of current access. Decide whether individually shared items are included in this first release or merely handled gracefully when encountered. A shared-library read capability must not imply library administration or playlist ownership.

## Original media and versions

[Direct Play and Direct Stream](https://support.plex.tv/articles/200250387-streaming-media-direct-play-and-direct-stream/) distinguishes sending an unchanged compatible file from repackaging or transcoding. It also describes multiple media versions and client selection. [Plex's format compatibility guidance](https://support.plex.tv/articles/201373793-is-plex-media-server-on-a-nas-right-for-me/) ties direct playback to media details, client capabilities, network conditions and playback settings.

**Planning inference:** keep a catalog track separate from its playable media selection. Resolve playback immediately before use; preserve the selected version, returned part key, authorization and preparation requirements. Fidelity acceptance must verify the delivered media and decoder/output path, rather than relying on an “Original” label. Retry and resume behavior should distinguish a connection failure, inaccessible item, expired authorization and unsupported format.

## Loudness, metadata and ordering

The [music feature overview](https://support.plex.tv/articles/205744257-getting-started-with-plex-music/) establishes multidisc support and loudness leveling, but does not define a third-party ReplayGain field contract. [Library analysis settings](https://support.plex.tv/articles/200289526-library/) allow loudness analysis to be disabled or scheduled. [Sonic-analysis documentation](https://support.plex.tv/articles/sonic-analysis-music/) says analysis results are kept in server data rather than modifying music files. [Embedded-metadata guidance](https://support.plex.tv/articles/200381093-identifying-music-media-using-embedded-metadata/) explains that catalog values can depend on library preferences and tagging quality.

**Planning inference:** distinguish embedded ReplayGain, server-derived loudness data and missing analysis. Do not silently interpret a time-varying loudness envelope as album ReplayGain. Keep gain provenance and units explicit; choose precedence and clipping policy in the audio decision. Verify actual music responses for gain/peak fields, disc ordering, compilation credits and multiple versions before fixing their mapping. Preserve source order where known and define behavior for missing disc/track numbers rather than guessing silently.

## Evidence gaps and validation gate

These are requirements for a later authorized feasibility check, not reasons to collect credentials during this research:

- Supported PMS versions: version header behavior, music pagination/filter/sort fields and exact metadata responses.
- New JWT sign-in, refresh, revocation and clock/expiry failures against owner and shared-server accounts.
- Reachability changes across local/remote connections; original stream preparation, HTTP range seeking, reconnect and authorization renewal.
- Playlist create/rename/delete/add/remove/reorder, duplicate tracks, permission loss, smart playlists and concurrent edits. Test duplicate entries specifically to catch accidental use of track identity for entry mutation.
- Track/album gain and peak availability, provenance and units with analyzed/unanalyzed and tagged/untagged files. Verify that optional fields really remain optional.
- Playback reporting effects on Plex history and resume state, with pause, stop, completion, skip and gapless handoff.

## Follow-on decisions

1. In the provider-model ticket, choose identity, playlist-entry representation, plain/smart/read-only capabilities, and the boundary between catalog tracks and prepared media.
2. In authentication/cache policy, choose the supported server-version floor, credential storage/renewal, cache invalidation and behavior after lost access.
3. In queue/session behavior, choose whether/how local playback reports to Plex without making Plex's remote play queue the application's session owner.
4. In the fidelity ticket, choose gain provenance/precedence and what happens when original playback or loudness data is unavailable.
5. If documentation is insufficient for any chosen contract, add a narrowly scoped, authorized live-server feasibility task or prototype before marking that design implementation-ready.
