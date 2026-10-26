# Current-stream recovery

An observed failure of the current media grant pauses output, revokes current and
prepared-successor access, stops the native entry, and retains the occurrence,
pinned provider version and last qualified nominal position. A failed prepared
successor cannot trigger current-stream recovery. A failed current grant cannot
be discarded by a speculative successor handoff. Authentication, permissions,
missing/unsupported media, invalid replies and output/ALSA errors do not receive
automatic network retries.

A network interruption permits at most two fresh preparations for this selected
visit, with 500 ms/1500 ms backoff, six seconds per attempt and a 15-second outer
deadline. The budget survives a successful refresh followed by another failure;
it is not an endless retry loop. Explicit playback/queue actions and fresh normal queue visits reset the budget; a recovery reload does not.
Preparation verifies the returned source-scoped track and exact pinned version
before source-header/gain reads or a local playable grant. A changed version
pauses and marks that occurrence without substituting it or selecting another.

Replies require the active account, queue session/revision, occurrence and native
generation. Valid transport/queue edits, output/gain changes, pause, stop,
sign-out and explicit selection cancel pending recovery. Late replies cannot
cancel newer recovery work or resume a cancelled selection. During recovery the
native player is paused/stopped. Successful refresh reapplies the retained
nominal position before restoring the prior play intent. Nonzero starts remain paused until the loaded native time position matches the requested start (within 50 ms before / 250 ms after); verification times out after five seconds. A source that ignores seeking stays paused with an error instead of restarting from zero. A previously paused
player stays paused. This rebuffering can introduce a gap and may repeat buffered
samples near the nominal position; it is not sample-accurate hardware recovery.

Recovery never advances a visit. Queue history/recorded presentation stays with
the existing occurrence. After exhaustion, the UI offers identity-fenced **Retry
stream** and **Skip**. Retry is an explicit normal preparation of the retained
pinned version; Skip is an explicit Next and preserves paused intent. The budget
and recovery task are transient; restart remains paused and requires explicit
sign-in/Resume. No listening reports follow from recovery/preparation events.

Tests use deterministic fake providers with virtual time, typed proxy body/open
failures, version refusal before byte reads, budget/deadline checks, stale account/
session/revision/generation refusal, cancellation and a synthetic WAV loaded into
libmpv's null output at the retained position. No Plex account, real music or
physical output is used for automated recovery checks.
