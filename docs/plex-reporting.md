# Confirmed Plex playback reporting

Implemented for #26. Timeline/session reporting is enabled after controlled
owned-server qualification under #16. It starts only from confirmed nominal
playback; preparing a track never counts as playback. It does not implement
scrobbling, played-state synchronization or history import.

## Ownership and dispatch

The coordinator captures an observation only for the selected occurrence whose
render generation has a confirmed, fresh nominal clock, a ready native entry and
no stream/native failure. Duration must be known and positive. Restoration,
selection, metadata access, preparation, prefetch, native EOF and unknown clocks
cannot establish a reporting owner. Native pause alone cannot establish one.

An owner retains the account epoch, queue session, occurrence, render generation,
source-scoped track and a private reporting-session UUID. Integer positions are
rounded from finite positive seconds and clamped to duration. Qualified playing
observations update its last-known position; confirmed native pause and stop use
that retained position. A successor's first positive clock produces a predecessor
stop (`continuing=1`) and its own playing report at its own position. Repeated
occurrences and replacement renders receive separate reporting identities.

Transitions are immediate; continuing updates have a ten-second interval. One
worker serializes requests with a three-second overall deadline. At most eight
pending transitions are held; adjacent updates for the same owner/state coalesce
to the latest position. Overflow suspends reporting and cancels its work, without
changing local playback. Failed requests are not retried or replayed after restart.

An account/provider change aborts the old worker and forgets its owner and pending
reports. A render boundary cancels stale position work before the next positive
clock. A later predecessor terminal report retains the predecessor's identity.
Cancellation cannot retract a request a server already received, and can lose a
best-effort terminal update. Sign-out sends nothing using revoked credentials.

An explicit Stop request waits for matching native idle acknowledgement, excluding
natural EOF and native errors. Quit can send a final retained-position stop only
following a successful native reset; its reporting drain is bounded to three
seconds. There is no percent-listened rule or completion inference.

## Plex adapter and status

The provider operation accepts typed backend data, with no UI-controlled URL or
credentials. It uses the track's source authorization and discovers the library
provider's timeline and metadata feature keys through `/media/providers`. Missing,
ambiguous, cross-origin, query-bearing and malformed feature paths fail closed.
It uses the [documented POST contract](https://developer.plex.tv/pms/#tag/Timeline/operation/timelinePostSlash),
source headers and a private session header; no fabricated Plex play-queue ID.
Discovery and result bodies are capped at 64 KiB. The currently supported rating
keys are numeric; the broader published opaque-key space is not qualified here.

Validated capabilities are cached per source inside the account-bound provider
for five minutes. Use does not slide freshness; a new provider/account starts
empty. HTTP 404/410 invalidates the route without retrying that failed POST.
Metadata paths remain immutable in the cache and are cloned for individual keys.
A cache miss plus POST still shares the three-second worker deadline.

`reporting.state` exposes disconnected, ready, accepted, unavailable, server
termination or suspended status separately from audio/queue errors. Status changes
publish even when the audio clock is unchanged. Explicit server termination
suspends further reporting; server text is never published and local audio is not
changed. Fake HTTP/provider cases cover this behavior; the live fixture did not
force administrative termination.

## Measured live scope

[Validation record](https://github.com/42lizard/qoda-player/tree/e3af727/prototypes/plex-reporting):
PMS `1.43.5.11029-6bf8af3f0`, verified HTTPS, browser-PIN credential, one intact
19,737 ms owned-library item. The API-version header was omitted; successful
requests do not prove negotiation of every current API-version semantic.
Private XDG state and libmpv `ao=null` kept the desktop/player state isolated.
No speaker PCM, metadata edit, deletion or scrobble was requested.

The initial two runs established accepted playing/paused/resumed/explicit-stop
POSTs, bounded position updates and matching paused-client state. Follow-up
staging and handoff used two distinct local occurrences of the same item. An
early successor-session sample was absent and is retained as incomplete evidence.
The diagnostic traced correct dispatch order and approximately five seconds of
combined repeated capability discovery. It did not demonstrate lost ownership.

The final cached run observed both predecessor and successor Playing sessions,
then absence after Stop. First discovery/POST took 2,537 ms. Cache-hit predecessor
stop and successor start each completed in 7 ms; the successor began at its own
488 ms instead of inheriting the predecessor's 19,352 ms terminal position. The
run completed in 28.46 seconds with clean exit and private-state cleanup.

Metadata `viewOffset`, `viewCount`, `lastViewedAt` remained absent and the filtered
item/library history stayed empty in these observations. Absence is distinct from
zero. These results qualify the tested timeline/session interaction, not history
creation, completion thresholds, universal short-track behavior, cross-server or
shared-account compatibility, real interruption/revocation or physical output.
Other owned-server live gates remain in #16. See the [historical live plan and
execution pointers](live-plex-reporting-plan.md) for scope and retained evidence.
