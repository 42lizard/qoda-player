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
coordinator dispatches remaining actions, prepares selected media, publishes the
resulting state and requests durable checkpoints. `Desktop::complete` acknowledges
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
