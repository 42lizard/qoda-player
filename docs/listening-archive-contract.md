# Nominal listening archive contract

Recorded before adding archive storage for #32. The archive retains observed
playback visits independently of editable queue membership and the bounded
Previous navigation path. It records nominal player evidence, not audibility,
completion, wall-clock listening time, a Plex play count, or provider history.

A visit has its own identity within a listening session and queue occurrence.
The same source-scoped track can therefore appear in several occurrences and
several visits. Explicit replay/repeat and navigation to another presentation
pass create a new visit. Seeking, restarting the current pass, stream recovery,
output reconfiguration and paused restart keep the existing visit. Preparation,
prefetch, stale/error clocks and unconfirmed successors cannot create or advance
an archive entry. The first fresh positive nominal playing clock creates it.

Each retained visit has immutable session/occurrence/visit/track identities,
a bounded display snapshot, first/last observation timestamps and last/max
qualified nominal positions. A seek can lower last position; max position is
not played duration. Backward wall-clock adjustments never lower persisted
observation timestamps. Position does not extrapolate across pauses, stale
samples, recovery or downtime. No end-of-file or 100 percent completion is
invented from duration. A crash can leave the last committed observation older
than the final actual position; the UI exposes write failure rather than
silently claiming completeness.

Default retention is 90 days with a hard 10000-visit cap; the user can choose
7/30/90/365 days or disable recording, which explicitly clears retained archive
rows. Retention runs transactionally with writes and at startup; oldest entries
are removed first. Retention changes and Clear are explicit archive controls.
Exports use a versioned JSON document containing a consistent bounded snapshot,
including policy and field meanings, written atomically to a new private file.
Export never modifies the archive or music. Exports are user-owned copies and
are outside subsequent archive clearing/sign-out cleanup; the UI says so.

Archive storage and any recovery copies are private, separate from the session
checkpoint and credentials. Binding a different verified account or disconnecting
clears the former account's archive before accepting later observations. Clearing
resets the observation baseline so late queued work cannot recreate old entries.
No credentials, authenticated URLs or provider responses are archived. General
retention preference survives sign-out.

The archive has an explicit application/schema identity. Empty storage is
initialized transactionally. Queue/navigation checkpoints are never imported as
historical listening evidence. Future supported migrations must first create a
consistent private backup and commit the rewrite transactionally. Corrupt,
unknown, newer or unsafe files and backups are preserved with recording blocked;
no automatic delete/recreate or guessed migration is permitted. File creation,
interrupted writes, export replacement and cleanup must retain this boundary.

Qualification requires deterministic tests for duplicate/repeated identities,
seek/recovery continuity, stale/late clocks, no preparation credit, retention,
export, account/clear fences, private files, unknown/newer/corrupt data and
transaction failure. Synthetic native observations remain distinct from physical
input, device presentation and real-server history effects.
