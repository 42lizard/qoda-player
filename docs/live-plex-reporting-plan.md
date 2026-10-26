# Controlled live reporting qualification plan

Prepared for #16/#26; **not executed**. The production reporting gates stay off.
This is a proposal for a deliberately selected disposable Plex fixture, not
authorization to play arbitrary library music or mutate listening history.

## Required test subject

The operator must designate an owned library and a disposable audio track in it,
ideally a short synthetic lossless fixture. Record the source/track privately;
public results use synthetic labels. The existing browser sign-in/keyring flow
supplies credentials locally; no token is requested in chat or put in argv/logs.
Shared/restricted libraries remain outside this support revision.

The timeline operations may change playback sessions, resume offset, play count
and listening history. No automatic restoration of history/play counts is
promised: undocumented compensating mutations would obscure the measurement.
No scrobble or metadata-edit call is part of this plan. If the test subject must
be added to a server, that is a separate user action or explicitly scoped task.

## Bounded first run

1. Confirm designated source/account and PMS/API version over verified HTTPS.
   Discover the matching library provider's timeline/metadata paths. Retain only
   redacted structural evidence, never headers, tokens or private server URLs.
2. Record the designated item's session presence, offset, play count and history
   separately before playback. Read-only observations alone are not pass results.
3. Use an isolated validation harness with private temporary state and the chosen
   fixture only. Render digitally without opening the laptop speakers or changing
   desktop audio configuration. It must use actual qualified nominal observations;
   fabricated timestamps or preparation cannot earn reporting credit.
4. Exercise one bounded playing → paused → playing → explicitly stopped run,
   then one complete fixture run if authorized. Record accepted POST/header shape,
   timestamps, bounded cadence and sanitized response/termination shape. Limit to
   two runs of the designated track, each at most 60 seconds. Stop on denied access,
   unexpected capability, explicit termination, or a cleanup/identity uncertainty.
5. Observe session presence, offset, play count and history separately afterward.
   Distinguish observed effects from undocumented thresholds; a 200 response alone
   proves neither history mutation nor correct attribution.
6. Stop owned helpers, delete private capture/state and confirm no validation
   backend/UI remains. Retain redacted findings on the validation branch, with no
   credentials or private capture committed. Do not flip production gates merely
   because the first POST works.

## Subsequent qualification

After the first run, review actual effects and termination semantics. Additional
explicitly designated fixtures may be needed for duplicate occurrences,
cross-source/gapless handoff, seeking, recovery, sign-out cancellation, very short
and unknown clocks, and failure cases. Fake tests already cover Qoda's mechanisms;
they do not substitute for these live contracts. Any broader test plan must name
its fixture/mutation scope before execution. The remaining unrelated owned-server
acceptance items in #16 stay open.
