# Listening archive

Open **Menu → Listening archive** or **Settings → History**. The archive shows
nominally observed playback visits across sessions, newest observation first.
Previous remains a separate bounded navigation path. Track copies retain source
identity; repeated occurrences and repeated visits are separate entries. Queue
removal, Clear upcoming, Stop and clear, or starting another session do not erase
the archive. Disconnect/sign-out clears account-bound archive data.

The [contract](listening-archive-contract.md) was recorded before adding storage.
Visits start only with a fresh positive confirmed nominal playing clock. A
prepared/restored/paused selection, speculative successor, failed stream or stale
clock cannot start a row. Seeks, current-pass restarts, recovery, output changes
and paused backend restoration keep the visit ID; replay, repeat and navigation
to another pass create another. The checkpoint stores that identity, independently
of mutable queue order and the Previous cursor.

Rows contain source-scoped track, session, occurrence and visit identities,
bounded title/artist/album snapshots, first/last observation timestamps and
last/max qualified positions. Position can move backward after a seek. Maximum
position is not listening duration or completion. Continuing observations save
at most every five seconds; a short final interval or crash can leave the most
recent position unrecorded. No hardware presentation or provider play count is
inferred. Known write/drop failures remain visible for the run and cause final
archive flush to fail; clearing the archive resets that warning. A new process
cannot prove that previous runs recorded every observation.

Default retention is 90 days and 10000 visits maximum. Choose 7, 30, 90 or 365
days; Apply trims older last observations transactionally. Off requires confirmation
and clears retained visits. Clear also requires confirmation. The hard visit cap
always applies and the SQLite file is limited to 128 MiB. The UI pages 20 visits.
Recording is bound to a verified account; browsing retained rows is available
while offline. A different verified account first clears the previous account's
rows. Failed account cleanup hides old rows and offers Retry account cleanup;
recording remains disabled until a successful connection binding.

**Export JSON** writes a consistent format-1 snapshot to a new private file under
`$XDG_STATE_HOME/qoda/archive/exports/` (default `~/.local/state/qoda/archive/exports/`).
The selectable path appears when the background worker completes. The export
includes retention, field meaning and known incompleteness for the exporting run.
Exports are user-owned copies: Clear and sign-out retain them, as stated in the
UI. Remove those copies separately when no longer wanted. No music is removed
or modified by archive controls.

The private archive uses `qoda/archive/visits.sqlite3`, an independent application
ID/schema 1, a single owner lock and serialized bounded worker commands. Files
are mode 0600, directories 0700; unsafe links/permissions, replaced identities,
foreign/newer/corrupt databases and orphan recovery files block recording and
are preserved. No credentials, upstream URLs, raw provider responses or audio
payloads enter the archive. An interrupted transaction retains prior committed
rows. There is no archive migration from queue/Previous snapshots: those cannot
establish historical observation evidence.

Session checkpoint schema/payload 5 adds the durable current visit ID. Opening
versions 1–4 creates a consistent private pre-migration backup, then commits the
rewrite transactionally. Older checkpoints get a new identity without an archive
entry. Later schema-5 restoration retains that ID and stays paused. Restore
accepts supported backups 1–5; unsupported/newer/corrupt data remains preserved.
Any future archive migration must first take a consistent private backup; schema
1 is the initial archive format and has no legacy archive rewrite.

Deterministic tests cover no preparation/stale-clock credit, duplicate visits,
seek/recovery/restore identity, account and clear ordering, retention/cap, private
exports, file replacement/orphans, unknown/corrupt/newer storage, transaction
failure and final-flush incompleteness. QML tests cover account-fenced controls,
clear confirmation, retention drafts and disabled/cleanup states. Fictional
full-window renders check the settings sheet at narrow widths; they do not
qualify physical input or multi-display behavior.
