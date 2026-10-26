# Confirmed Plex playback reporting

Implementation for #26. **Live reporting is disabled pending #16.** Neither an
environment variable nor a UI request enables it. Existing player behavior does
not send timeline requests or scrobble calls. The backend snapshot exposes
`reporting.state = disabled_pending_live_qualification` independently of audio,
queue, provider sign-in and general error state.

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
after a successful native reset; its reporting drain is bounded to three seconds.
There is no percent-listened rule, completion inference, scrobble or history import.

## Plex adapter

The provider operation accepts typed backend data, with no UI-controlled URL or
credentials. It uses the track's source authorization and discovers the library
provider's timeline and metadata feature keys through `/media/providers`. Missing,
ambiguous, cross-origin, query-bearing and malformed feature paths fail closed.
It uses the [documented POST contract](https://developer.plex.tv/pms/#tag/Timeline/operation/timelinePostSlash),
source headers and a private session header; no fabricated Plex play-queue ID.
Discovery and result bodies are capped at 64 KiB. Errors expose only sanitized
status. Explicit server termination suspends further reporting; no server text is
published and local audio is not changed. Its final live product policy still
requires qualification before enabling the gate.

Both the public Plex operation and the coordinator are gated off. Fake-provider
tests enable only an internal test constructor; fake HTTP tests call the private
transport against loopback fixtures. Those tests prove Qoda attribution,
cancellation, bounds and request shape, not server history effects or live PMS
compatibility. See [the controlled live plan](live-plex-reporting-plan.md) before
changing either gate.
