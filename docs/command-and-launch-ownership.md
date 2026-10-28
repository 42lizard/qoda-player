# Listening-session commands and installed launch ownership

## Listening-session commands

`session_commands::Playback` owns command admission: validate the request, save
changed output/gain preferences, cancel recovery, classify speculative cancellation,
complete any native reset, then validate again before admitting a mutation. Both
local socket requests and translated desktop media commands cross `admit`.

`Context` borrows the current account, catalog and settings. It does not retain
provider access or create another listening session. A single effect classification
records whether a command may affect buffered output or the current occurrence.
Invalid instance/session/revision, catalog, navigation and edit arguments are
rejected before cancellation. Queue operations are validated on a bounded copy
through Queue's existing operations, rather than through a second membership model.

The coordinator reconciles observed natural handoffs before admitting commands.
During cancellation a native barrier can reveal a later current occurrence.
Admission records that nominal evidence, revokes old grants and advances the output
epoch before returning. A socket request retains its original revision and must be
reviewed if the queue changed. A desktop command is checked for owner, deadline and
reply liveness before cancellation and again before mutation, and translated against
the new nominal view. A request to pause cannot become a request to resume because
a reset changed playback intent.

The command module resolves each queue edit into one operation, applies that same
operation to the bounded trial and then to the live Queue, and returns preparation,
automatic-next, checkpoint and seek effects. It also applies output changes,
including preparation cancellation, clock retention, epoch/load invalidation,
grant revocation and explicit paused intent. The final desktop liveness check has
no intervening await before Queue mutation. Queue remains the authority for
membership, current occurrence, visits and navigation. Player, gapless, recovery
and proxy retain their existing native and provider implementation. The
coordinator dispatches remaining actions, delegates selected media preparation,
publishes the resulting state and requests durable checkpoints. `Desktop::complete` acknowledges
the resulting error or success after that processing; Quit acknowledges its
shutdown snapshot.

These ownership changes preserve [nominal presentation](adr/0003-nominal-playback-presentation.md)
and [bounded continuous conversion](adr/0004-continuous-converted-input.md).
Admission is not listening evidence. Digital/null tests do not qualify physical output.

Decoder failure classification belongs to the continuous input stream. The polling
coordinator and multiplex worker report failures through one generation-aware
operation. It classifies and retires an uncommitted future under one state lock;
duplicate or late reports for retired input are ignored. Current or committed
input failures still cancel the stream explicitly.

## Selected prepared playback

Selected prepared playback owns ordinary preparation tasks, result identity,
automatic-progression policy, pinned recovery integration and completion through
version verification, stream grant and native load. Command admission still owns
validation and buffered-edit reset. Queue remains authoritative for membership,
nominal presentation and visits; speculative successor preparation remains with
gapless. Ordinary preparation starts a fresh recovery budget, while recovery of
the same visit retains its bounded budget and pinned media version.

The coordinator supplies the current account, Queue, output epoch and playback
intent when crossing this interface. Completion must still own that selection
and epoch before granting access or loading output; a consumed result cannot be
applied twice. Pausing ordinary preparation permits a valid paused load. Cancelling
recovery, replacing the selection or changing account/output invalidates old work.
Tests use the existing synthetic provider adapter and native null output through
the same initiation and completion interface as the coordinator. Full cancellation
owns both selected work and recovery; callers do not assemble two cancellation
steps. Stream interruption alone retains the same-visit recovery budget.

## Displayed track catalog

The displayed track catalog owns its typed page, request lifetime, catalog revision
and cache provenance. Browse start, accepted refresh and invalidation are the only
ways to change that state. Socket serialization is output from that ownership,
not the source from which playback commands reconstruct metadata. Single-track
selection retains the existing album-detail fallback; bulk selection additionally
requires account/revision fences and preserves displayed order atomically.
Existing live/cached access and metadata-cache policy remain responsible for
source permission and refresh behavior. Album, artist, search and playlist
workflows retain their own ownership.

## Source and pacman launch

`tools/launch.py` owns launch definitions, quoting, process-manager identity,
ownership validation, activation fencing and diagnostic journal names. `Launch` is
configured once for the resolved release and either the source or pacman installation.
`for_release` creates a configured launcher for a release switch; callers do not
change module globals. `integration_files` renders each installation's definitions
directly, with no rewriting of rendered commands.

`tools/install.py` retains source installation inventory, staging, transactional
recovery and rollback. `packaging/arch/stage.py` stages package-owned files using
the shared definitions. `packaging/arch/entry.py` selects the pacman configuration,
checks ownership and performs its existing data-compatibility check before starting
the backend. Pacman retains package transactions and its stopped-process guard.
Neither path starts playback during installation or enables automatic startup.

The two installations retain distinct process-manager names and share user data as
before. Their compatibility refusal, paused restart, ownership checks and instance /
deadline fences remain required. This preserves [ADR-0001](adr/0001-independent-rust-backend.md).

## QML backend connection

`BackendConnection.qml` owns the Unix socket, hello, reconnect, complete-line
handling and command availability for both the player shell and installed widget.
`BackendState.qml` owns snapshot validation, lossless decimal counters, ordering
within one backend instance and fresh command envelopes. Invalid complete frames
close the connection; commands require a validated snapshot from the current
connection. The complete-line size check does not bound unfinished SplitParser
buffering. A replacement backend may restart its revision at zero.

The shell retains its last display while disconnected, but its instance/readiness
is reset and commands are disabled. The widget clears its snapshot. The shell's
adapter advertises its role and owns stable visual models, Raise suppression on
first snapshot/reconnect, and acknowledged Quit. The widget remains non-raising
and restricts operations to its transport controls and Quit. The installed widget
payload includes the common modules and changes its fingerprint with them.

`tools/check-backend-connection.py` exercises the production shell connection and
widget adapter with private Unix peers, fragmented lines, invalid frames, large
counters, replacement and reconnect. Existing layout, widget, Raise and Quit tests
retain their respective presentation coverage.

## Account sign-in lifetime

`account::Controller` owns account and keyring generations, the active sign-in
worker, session credentials, live provider authority, cached discovery and typed
keyring action/saved-state knowledge. Its command and result interface executes
account transitions against the existing preparation, catalog, output, reporting,
cache and archive owners. Main dispatches actions/results and publishes their
projections; it does not sequence account cleanup. The external sign-in adapter is
Plex in production and a pending adapter in tests; identity paths are explicit so
tests never read or overwrite the user's source identities.

Reconnect invalidates old work and output, retains read-only same-account discovery
and returns paused. Disconnect/Forget clears account data and saved listening
state; Forget additionally queues keyring deletion after any pending operation.
Account generations fence late authentication/connection results. Independent
keyring generations preserve serialized Save/Forget semantics: a cancelled restore
may update saved-state knowledge but may not reconnect. Dropping the owner aborts
its current sign-in worker.

The current connection failure policy still groups `AccessDenied` and
`SignInRequired` for discovery/archive cleanup. The account tests characterize that
existing behavior; it must not be interpreted as proof that expired sign-in means
confirmed access loss. This refactor preserves the policy rather than expanding
its meaning. Null-output transition tests and the existing disposable keyring
helper cover stale restore/connection results, a superseding browser sign-in,
Save→Forget, paused reconnect, partial discovery and failed discovery.

## Cache maintenance

`maintenance::Controller` owns user-requested artwork/metadata Clear and budget
operations. It validates input, admits one operation per target, invalidates
browse/grant authority before spawning work, retains typed operation/account
identity, accepts completion once, and drains the work before store shutdown.
The existing cache modules retain storage, cleanup epochs, budgets and security.
Account sign-out retains its separate destructive account transition.

Metadata maintenance temporarily owns the foreground status. Intervening activity
relinquishes that ownership and restores the prior status before the new activity
publishes its own result. Completion can update that status only while its typed
operation still owns it; displayed sentences never determine operation kind.
Application-wide cache failures remain diagnostic after an account replacement,
while account-bound artwork republication requires the current account generation.

Production-interface tests use actual disposable stores, hold completed replies
across browsing/account changes, verify admission/retry and clear/disable behavior,
and drain both targets before shutting down their stores. Native offline checks
retain the real backend's queue/discovery/restart coverage.

## Nominal observation

`observation::Controller` owns native sampling, clock freshness, handoff
reconciliation and ordered fan-out to reporting, archive and desktop/UI presentation.
A sample pairs native status, stream failure and capture time. Presentation and
command qualification sample fresh evidence instead of combining a new status
with freshness flags retained from a prior timer tick. Freshness lives with nominal
observation; the desktop module owns its projection and control protocol.

The first-positive-clock ordering is preserved: reporting observes prior
confirmation, then Queue confirmation is updated, then archive and desktop/UI use
that confirmation. Sink-specific rules remain explicit: reporting requires duration
and can retain a paused report; archive excludes paused observations; desktop/UI
can display a confirmed pause. Output epoch, account and confirmation still have
different meanings. Account replacement cannot reinterpret the same old output
epoch as evidence for the new account; it needs a new output epoch and clock.
This prevents late old-account evidence gaining new reporting/archive authority.

Queue remains authoritative for occurrences and visits, gapless for successor
bindings, and command admission/preparation for their reset and invalidation work.
Their native reset path uses the observation module's clock-recording operation.
Output-change barriers capture and qualify fresh status/failure evidence before
confirmation: retaining position does not let a stalled clock establish presentation.
Synthetic tests exercise the production fan-out with real reporting and archive
adapters. Native null-output tests enter both timer and pre-command paths and
verify one promotion, old-grant revocation and no replacement load. These preserve
[ADR-0003](adr/0003-nominal-playback-presentation.md) and
[ADR-0004](adr/0004-continuous-converted-input.md); no physical output claim follows.

## Complete transport commands

Listening-session command admission also completes Pause, Resume and Stop. Pause
retains a matching native position and requests a forced checkpoint. Stop cancels
preparation, resets output, revokes the stream and retains the selected occurrence
at the beginning; Stop and clear remains the distinct queue-discarding operation.
Resume retains pending preparation, unpauses only matching live output, or asks
for preparation of the retained/next selection. The coordinator consumes reporting
and preparation/checkpoint effects rather than finishing transport policy.

Production and native null-output tests cross the same command interface. The
prepared-playback fixture no longer recreates forwarded Pause. Its late completion
checks exercise Pause/Resume/Pause while preparation is pending; transport tests
also cover retained visits, Stop followed by immediate Resume, disconnected/empty
Resume and retry of an unavailable selection. Existing desktop deadline and
buffered-reset tests remain in place.

## Durable listening-session checkpoints

`storage::Session` owns queue-stamp deduplication, purge generations, accepted
completion serials, explicit Retry save and the storage projection. Sign out tells
this owner to retain a purge obligation after replacing the queue; account code
no longer manages storage generations or cleanup presentation. The serialized
storage worker retains bounded requests, coalescing, SQLite transactions, security
checks and backup policy. Final shutdown uses the same owner's flush, with the
existing checkpoint/backup exit classification.

A failed submission still counts as attempted: unchanged queue state does not
silently retry, and Retry save forces another attempt. Old completions cannot mark
a newer request or Sign out cleanup saved. Cleanup remains pending until the
corresponding save commits; a committed save with a backup warning stays saved.
Tests use temporary SQLite for save/reopen/purge and a controlled bounded storage
transport for delayed completion and Busy/retry ordering, through the production
checkpoint interface.

## Buffered-output reset characterization

The three buffered-output reset triggers have distinct outcomes. This table records
the existing policy before concentrating their shared implementation.

| Trigger | Successful captured snapshot | Failed native barrier | Caller outcome |
| --- | --- | --- | --- |
| Admitted command cancellation | Reconcile any bound handoff, retain matching native clock evidence, invalidate the successor, revoke grants, advance output epoch, enter review. | Invalidate successor, revoke grants, advance epoch, invalidate native load ownership, send Pause, enter review. | Pause intent; reject on reset/handoff error; otherwise revalidate the original socket revision or the desktop owner/deadline before mutation. |
| Speculative preparation completion failure | Same captured-snapshot reset. | Invalidate successor, revoke grants, advance epoch, enter review. | Pause intent; retain the original preparation/staging error even if snapshot reconciliation also fails. |
| Unbound/failed observed handoff | Same captured-snapshot reset, only while the current stream grant has not failed. | Invalidate successor, revoke grants, advance epoch, enter review. | Pause intent; retain the observed-handoff error and checkpoint the reset. Failed current grants remain on the stream-interruption/recovery path. |

A successful snapshot retains position only for matching output ownership and
confirms only native positive-clock evidence according to the existing
`observation::record_clock` rules, with current proxy failure included. It is not
an ordinary fresh polling sample, an archive observation, or a provider report.
The observation owner continues to qualify ordinary presentation and output-change
barriers. This ownership change does not relax those rules.

An exhausted epoch during failed-barrier cleanup propagates as a fatal runtime
error. An epoch or handoff error while consuming a successful snapshot is a reset
error: commands reject it; automatic triggers preserve their original error.
This distinction is retained rather than silently normalized during the refactor.

`buffered_reset::Runtime` now owns the shared reset protocol. Command admission
asks it to cancel speculative input and classifies the resulting reset outcome;
preparation completion crosses `prepared`; observed handoff failure uses
`after_failure`. The coordinator projects completion results without assembling
native cleanup. Observation no longer imports the command module for resets.
Trigger-specific native load invalidation and error handling remain explicit
inside this owner, while command revalidation and failed-stream recovery decisions
stay with their existing owners.

Native null-output regressions cover preparation staging failure, stale completion,
unavailable-successor fallback, paused snapshot position retention, unbound or
queue-mismatched observed handoffs, successful and failed barriers, and failed
current grants staying on the interruption path. Existing command tests retain
untouched versus committed input, handoff reconciliation, late preparation and
desktop expiry coverage through production command admission.

## Prepared-load admission and retained gain (issues #65, #66)

The native playback owner admits a prepared load after direct loading, silent
inspection and resume verification. Its explicit-load generation covers staged
successors until the next explicit invalidation. Stale work is stopped before
publishing readiness or a source position; pending inspection/resume cannot
unpause. Playback intent alone never admits an obsolete load.

Prepared playback retains original gain together with account, listening session,
queue occurrence and pinned media version. Seek and converted network recovery
keep that record only for the same identity; cancellation discards it. A converted
preparation has no original gain of its own. Completion resolves it from the
retained record or refuses playback explicitly. Unknown embedded metadata remains
an explicitly described unity outcome from original preparation, never evidence
that server conversion preserved tags. Gain calculation and retry budgets retain
their existing owners.

The private native `InspectNext` test command routes a load through the real
inspection implementation on null output and holds completion after native setup.
It controls scheduling only; production has neither the command nor the gate.
