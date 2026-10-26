# Plex playback reporting contract

Researched 2026-10-26 for #26. The original recommendation was to keep live
reporting disabled pending #16; subsequent [scoped live qualification and current
implementation](../plex-reporting.md) supersede that implementation gate. This
note used public first-party documentation and source only; no user server,
credentials, playback, history mutation or API request was exercised.

## Current published contract

Read 2026-10-26: official PMS OpenAPI **1.2.3**. Sorted, compact UTF-8 JSON SHA-256:
`7e51d7d0cfc424e790eff9a5b9062791fca8dd3b1093b774432a6c6f2c9ec0c0`.
Documentation is mutable. [Official PMS API](https://developer.plex.tv/pms/).

- Use **POST `/:/timeline`**. Query fields include `key` (details path),
  `ratingKey`, `state`, `time`/`duration` (integer milliseconds), and
  `playQueueItemID` for server play queues. States are `playing`, `paused`,
  `stopped`, `buffering`. `continuing=1` describes another item following a stop.
- Report transitions and approximately every **10 seconds** on LAN/WAN
  (20 seconds cellular). Authentication uses `X-Plex-Token`.
  `X-Plex-Client-Identifier` is required; `X-Plex-Session-Identifier` optionally
  distinguishes playback sessions. Query fields themselves lack `required`
  markers in this schema.
- Documented responses are **200/400**. Normal JSON contains an empty
  `MediaContainer`; 200 can also carry `terminationCode`/`terminationText`.
- Discover timeline capability/path from `/media/providers`; the documented
  default path is not universal. Omitted `X-Plex-Pms-Api-Version` means legacy
  version 0.0.
- **PUT `/:/scrobble`** separately marks played state; it does not create view
  history. It takes provider `identifier` plus `key` (rating key) or `uri`.
  [Timeline endpoint](https://developer.plex.tv/pms/#tag/Timeline/operation/timelinePostSlash),
  [scrobble endpoint](https://developer.plex.tv/pms/#tag/Timeline/operation/putScrobble),
  [API paths/versioning](https://developer.plex.tv/pms/#section/API-Info).

## Pinned first-party client evidence

Plex for Kodi's `NowPlayingManager` sends to the item's own server, suppresses
unchanged item/state updates until its timer expires, includes metadata key and
rating key, and adds `playQueueItemID` only when a server queue exists. Its source
caps reported position at duration because its authors observed HTTP 400 above
duration. This is client behavior, not an additional current PMS specification.
Its timer is 15 seconds; current PMS documentation above takes precedence.
[NowPlayingManager at 97191f2, lines 135–196](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/_included_packages/plexnet/nowplayingmanager.py#L135-L196).

That historic client uses a request with no explicit method, whose HTTP helper
falls back to GET without a body. Qoda should follow the current documented POST,
not reproduce that old default. Its request wrapper obtains the item's server
URL/token and adds Plex headers internally.
[Request wrapper](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/_included_packages/plexnet/plexrequest.py#L10-L19),
[HTTP method dispatch](https://github.com/plexinc/plex-for-kodi/blob/97191f248a20223bb93296789c010ff2204a9c76/lib/_included_packages/plexnet/http.py#L83-L105).

The archived Plex Media Player remote-control API also describes POST timeline
notifications **to subscribed controllers**. That is a different receiver and
XML contract; its server-address/token fields are not a template for reporting
to PMS. [First-party remote-control documentation](https://github.com/plexinc/plex-media-player/wiki/Remote-control-API).

## Qoda implementation recommendations

These are design recommendations, not additional Plex guarantees:

- Put a typed reporting operation on the provider boundary. QML supplies no
  endpoint, credential, header, fabricated server queue ID, or remote session ID.
  Reuse the selected source's validated server origin and scoped authorization;
  never reroute a report through whichever library is currently visible.
- Maintain a confirmed reporting owner distinct from browser selection and
  decoder/prefetch selection: account epoch, source-scoped track, occurrence,
  nominal playback generation, local listening session and last qualified
  position. Start reporting only after the current owner has a fresh positive
  nominal clock. Metadata reads and prepared successors produce no report.
- For pause/stop, retain that owner's last qualified position. At a handoff,
  keep the predecessor's terminal report attributed to the predecessor; begin a
  successor only after its own confirmation. Never borrow the old end position
  for a new rating key. Do not equate native EOF with heard completion.
- Give each backend playback session a private reporting identifier, separate
  from auth identity. Omit `playQueueItemID`: Qoda owns an independent local queue.
  Omit sync/offline flags and unmeasured buffering/bandwidth metrics. Round/clamp
  finite nominal seconds to nonnegative milliseconds within a known positive
  duration. Defer duration-unknown reports until their contract is qualified.
- Use one serialized worker, bounded latest-update coalescing and a short request
  deadline (for example three seconds). Keep terminal transitions bounded too;
  no unlimited historical outbox or automatic replay after restart. Fence work
  before dispatch and after completion; account revocation cancels pending work.
  Cancellation cannot retract a request already received by a server.
- Reporting failure updates a separate sanitized status and never blocks the
  audio actor or changes the local queue. Parse a bounded response; distinguish
  explicit server termination information from a generic transport failure.
  The desired local treatment of termination needs qualification before enabling.
- No scrobble call, percent-listened rule, completion counter or history import
  belongs in this increment. No GPL client implementation code needs to be copied.

### Proposed live qualification checklist

Before enabling live reporting, record the tested PMS/API version and capability
path, current source/account/session attribution, accepted POST/headers/response,
playing → paused → playing → stopped, bounded updates, direct-play gapless and
cross-source handoffs, duplicate queue occurrences, seeks, recovery, unknown/very
short clocks, failures and sign-out cancellation. Inspect resulting session,
metadata offset, play count and history as distinct effects. Any history-mutating
live scenario needs a deliberately chosen fixture and recorded before/after
state; this research performed none. Deterministic fake-provider/HTTP tests can
qualify Qoda's state machine and transport now, while live dispatch stays disabled.

## Unresolved live evidence for #16

Not established by reviewed documentation: music completion thresholds,
play-count/history effects, retry idempotency, overlapping-request ordering,
session expiry, or older/shared-server requirements. These remain live
qualification items under #16.
