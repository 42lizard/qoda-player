# Read-only observations for the live reporting test

Researched 2026-10-26 for #16/#26. Public documentation only; no credentials,
user-server requests or playback. Official PMS OpenAPI **1.2.3**, canonical JSON
SHA-256 `7e51d7d0cfc424e790eff9a5b9062791fca8dd3b1093b774432a6c6f2c9ec0c0`.

| Distinct observation | Request and fields |
| --- | --- |
| Active session | `GET /status/sessions` (admin; unfiltered): match `MediaContainer.Metadata[].ratingKey` and `Player.machineIdentifier`; inspect `Player.state`, `viewOffset`, `Session.id`. [API](https://developer.plex.tv/pms/#tag/Status/operation/statusGetSlash). |
| Resume/count | `GET /library/metadata/{ratingKey}` → `MediaContainer.Metadata[].{viewOffset,viewCount,lastViewedAt,duration}`. Offset/duration: milliseconds; lastViewedAt: epoch seconds. [API](https://developer.plex.tv/pms/#tag/Content/operation/libraryMetadataGetSlash). |
| History | `GET /status/sessions/history/all?metadataItemID=<key>&librarySectionID=<section>&sort=viewedAt:desc`; optional `accountID`. Paginate: `X-Plex-Container-Start`/`X-Plex-Container-Size`. Inspect `Metadata[].{historyKey,ratingKey,accountID,viewedAt,deviceID}`, container `offset`/`totalSize`. Admins see all users; others their own. [API](https://developer.plex.tv/pms/#tag/Status/operation/statusGetHistoryAll). |

`POST /:/timeline`: `X-Plex-Token` authenticates; client-identifier header required,
session-identifier optional. `key`, `ratingKey`, `state`, `time`, `duration`
declarations lack required markers. HTTP 200 can contain
`MediaContainer.terminationCode` (integer) and `terminationText` (string); no code
enum is specified. [Timeline API](https://developer.plex.tv/pms/#tag/Timeline/operation/timelinePostSlash).

## Interpretation limits for the harness

Treat these as separate measurements. A successful POST does not establish a
session, offset persistence, consumption increment or history creation. Preserve
missing values distinctly from zero; permission failure is an unavailable
observation, not an empty result. Bound requests/pages and label truncation.
Keep unrelated sessions private and discard them without logging.

Do not assume server `Session.id` equals the supplied client session UUID, or
numeric history `deviceID` equals the client identifier. Verify returned item and
account attribution. Concurrent playback of the same item can confound causality;
retain before/after deltas and timing without asserting undocumented thresholds.
Avoid metadata refresh/check/augmentation options; `skipRefresh=1` is documented
for skipping synchronous local-agent/analysis refresh.

The metadata schema calls `ratingKey` opaque: numeric-only transport validation is
a narrower compatibility policy, not a universal Plex identity guarantee. This
run should record whether its selected fixture meets that policy. No history
deletion, scrobble, metadata editing or administrative session termination is
needed for the initial observations. Explicit termination behavior beyond its
published response shape remains unqualified until deliberately exercised.

## Device/account attribution

PMS 1.2.3 documents neither `GET /devices`, `GET /accounts`, nor a history/device-client
join. [`/security/resources`](https://developer.plex.tv/pms/#tag/General/operation/securityGetResources)
`Device.clientIdentifier` describes source connections. Session `Player.userID` /
history `accountID` lack specified equivalence. This absence claim concerns the
reviewed specification, not every server implementation.

An empirically available `/devices` response could support a **version-scoped
observed** mapping only after verifying unique `Device.id == history.deviceID`
and exact `Device.clientIdentifier == our X-Plex-Client-Identifier` on the same
PMS. Do not assume `machineIdentifier` is an interchangeable response field
unless actually observed. A friendly device name or timestamp alone is
insufficient. Likewise verify the history account against the privately known
playback account; do not infer identity from display names. Without these joins,
record an item/account history delta with client attribution unverified. Do not
present an undocumented response shape as the current official API contract.

## Music completion: what the primary evidence establishes

The current timeline parameter list declares neither `identifier` nor `type`.
Separate `PUT /:/scrobble` requires `identifier` and either `key` or `uri`; it
marks played state without creating view history.
[Scrobble API](https://developer.plex.tv/pms/#tag/Timeline/operation/putScrobble).
The published contract therefore gives no basis for adding undocumented music
parameters or an automatic scrobble merely to make a test's history nonempty.

Pinned first-party Plex for Kodi `97191f2` handles audio playing, pause, resume,
stop and end through `updateNowPlaying`. That calls the server timeline builder,
which supplies neither `identifier` nor `type`. The reviewed audio playback path
contains no separate automatic completion-scrobble call. Manual Mark Watched
methods for video/photo are different operations. This is evidence about that
client revision, not proof about all Plex clients or present server internals.
[Audio handler](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/player.py#L598-L630),
[timeline builder](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/_included_packages/plexnet/nowplayingmanager.py#L175-L196),
[manual video action](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/_included_packages/plexnet/video.py#L95-L103).

Historical first-party explanations agree with keeping those operations distinct.
Plex employee/co-founder Elan described Plexamp's offline listens through timeline
and the scrobble endpoint as marking watched/unwatched (2022). He later cited an
offline-specific server condition, not a general music-completion threshold.
Those old implementation details do not justify fabricating offline events,
durations, completion positions or count overrides for this live test.
[Staff explanation](https://forums.plex.tv/t/where-can-logs-containing-scrobble-information-be-found/802070/9),
[offline-specific clarification](https://forums.plex.tv/t/where-can-logs-containing-scrobble-information-be-found/802070/12).

For resume semantics, Elan distinguished Plexamp's retained queue/current position
from returning much later to an individual track with the library's **Store Track
Progress** setting (2021). In 2022 he said enabled progress should persist on
pause or stop and mentioned a Plexamp bug fix. This makes library mode a relevant
unmeasured condition; it does not establish the tested library's setting or
guarantee serialization of an offset for every music track.
[Queue versus track resume](https://forums.plex.tv/t/plexamp-bug-played-tracks-does-not-reset-playback-to-the-beginning/746657/4),
[pause/stop clarification](https://forums.plex.tv/t/plexamp-doesnt-continue-playing-audiobooks-from-where-they-left-off/813276/4).

The official support page labels its 90% played threshold as **video**. A 2021
staff reply mentions short tracks and Sweet Fades as an edge case; it is not a
current minimum-seconds or universal music threshold contract. No such threshold
was established by this research.
[Video setting](https://support.plex.tv/articles/200289526-library/),
[historical short-track discussion](https://forums.plex.tv/t/a-few-bugs-in-plexamp-that-i-ve-noticed/740159/8).

The coordinator reports seven HTTP-200 timeline submissions across two authorized
runs of one 19.737-second track, a matching paused session followed by its absence
after stop, missing metadata offset/count/timestamp fields before and afterward,
and zero entries in the filtered history observations. These establish the scoped
session interaction and **no observed serialized metadata/history change** in
those reads. They do not prove a missing required parameter, a requirement for
scrobbling, a numerical completion threshold, or universal lack of history
support. Do not silently replace absent fields with zero or widen #26's scope.
