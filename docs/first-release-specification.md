# First-release specification and acceptance map

The existing decisions in [#1](https://github.com/42lizard/qoda-player/issues/1)
remain the product inputs. This document consolidates them for #12. Implementation
continues under #21; a specification or passing CI does not ship or qualify the
player. The [accepted support boundary](first-release-scope-proposal.md) narrows external
claims while keeping the independent software backlog and owned-server checks.

## Product and module boundaries

Qoda is a standalone Omarchy music player, initially backed by Plex. Rust owns the
independent backend; Quickshell/QML owns browse/search, queue/playlist controls and
full/compact/hidden presentation. Additional providers must fit the provider
boundary, not add upstream credentials or media API ownership to QML. libmpv owns
decoding/output; the backend owns output policy, source grants and lifecycle.

The settled domain/authority boundaries are:

| Boundary | Contract |
| --- | --- |
| Media reference | Provider/source-scoped identity; identical track keys in different libraries/accounts remain distinct. URLs and credentials are not identity. |
| Listening session | Independent local queue snapshot, stable occurrences and distinguishable visits. Plex playlist edits do not retroactively edit it. |
| Backend owner | One coordinator instance owns queue mutations, render generations, nominal timing, provider work and persistence. |
| Commands | Local peer identity, instance, account and queue/page/render fences protect the applicable operation. Large counters cross QML as decimal strings. Stale or partial bulk operations are rejected, not partly applied. |
| Preparation | Pinned media/version preparation, prefetch and speculative successors confer no presentation or listening credit. |
| Presentation | Qualified nominal clocks identify current presentation; decoder selection alone does not. Missing/stale clocks remain explicit. Physical presentation is a separate measurement. |
| UI/Desktop | UI requests and MPRIS are clients/projections, never competing playback/queue owners. Widgets do not claim browser Raise ownership. |
| Asynchronous work | Replies need their live account/source/session/revision/generation/desktop owner as applicable; cancellation and late replies cannot restore earlier authority. |

The authority and timing inputs are [#7](https://github.com/42lizard/qoda-player/issues/7),
[#10](https://github.com/42lizard/qoda-player/issues/10) and
[#20](https://github.com/42lizard/qoda-player/issues/20), with the independent
backend and fidelity decisions in [ADRs](adr/0001-independent-rust-backend.md).

## Required user flows

1. Start or restore the browser without starting music. Connect Plex via the
   accepted browser authorization path; choose a source/library explicitly.
   Saved sign-in is opt-in through Secret Service, with visible locked/unavailable
   handling and explicit disconnect/cleanup. Authentication material stays private.
2. Browse artists/albums/tracks, search within and across source-scoped libraries,
   and load bounded covers/pages on demand. Offline startup can display cached
   discovery/catalog read-only; cached metadata never authorizes an upstream edit
   or media request. Partial and unavailable sources remain distinguishable.
3. Play an explicit selection, album, playlist snapshot or selected track batch.
   Selection alone does not play. Play next uses FIFO ordering; Add appends;
   independent duplicates, stable shuffle, repeat, Previous/Next and unavailable
   entries retain their accepted identities and fences.
4. Browse and manage supported plain Plex playlists by entry identity. Smart or
   denied operations are read-only. Reordering/removal must not mistake duplicate
   occurrences or treat an uncertain API response as confirmed success.
5. Use approved layout A: left library navigation, center browser, right queue and
   bottom transport. Narrow layouts use a queue drawer without replacing browser
   state. Full/compact/hidden forms retain navigation; window close means Hide,
   while Quit is explicit. Settings/focus/drag interactions keep their declared
   keyboard and revision behavior.
6. Recover a current stream within the pinned visit/version and bounded retry
   policy, preserving position and pause intent. A failed or unqualified seek must
   not silently restart audio from zero. New process restoration stays paused.
7. Switch output explicitly, paused. Desktop/PipeWire is default; native direct
   ALSA and optional HQ conversion retain their distinct policies and labels.
   ReplayGain defaults to Auto, uses verified provenance, and never double-applies
   native gain. Unsupported/unreadable metadata uses an explicit unity fallback.
8. Hide/reopen and use desktop controls independently of backend lifetime. The
   installed launcher serializes activation and fences delayed UI work by backend
   instance/deadline. Quit stops resources and reports checkpoint/backup/cleanup
   failure; it does not become permission to restart or play.

Detailed implemented flows and limits remain in [development](development.md),
[search](search.md), [library interactions](library-interactions.md),
[reordering](reordering.md), [offline](offline.md), [settings](settings.md),
[theme](theme.md), [activation](activation.md) and [shutdown](shutdown.md).

## Audio and persistence contracts

[ADR 0002](adr/0002-fidelity-and-gain-contract.md) separates strict native output,
ReplayGain and opt-in conversion. Native output must negotiate/admit the intended
format before submission; failure does not silently choose a different fidelity
profile. HQ conversion explicitly declares resampling, precision reduction/dither
and optional layout conversion. It is not labeled sample-perfect. The gain and
continuity evidence is bounded by the exact capture point, fixture and profile.

Queue/session SQLite checkpoints and consistent backups use private user-scoped
storage and serialized writes. Existing unsupported/newer/corrupt state and
unowned artifacts are preserved. UI exposes save failure and recovery actions;
sign-out/account cleanup includes the applicable private caches/backups and
credentials. Restoring a backup preserves the original and starts paused. Cache
budgets/eviction do not become a general permission to delete foreign files.

Installation owns code/integrations separately from preferences, state, cache and
keyring entries. Source builds are locked and versioned, installation/removal are
conflict-aware, login autostart is absent by default, and uninstall retains user
data. Current upgrades require stopped processes. In-use checkpointed switching
and schema-safe code/backup rollback remain #46; installed logout/crash behavior
remains unqualified, outside the experimental installed-service support claim
recorded when resolving #45.

## Acceptance evidence map

| Area | Implementation/evidence | Remaining gate |
| --- | --- | --- |
| Browse, queue, playlist and UI contracts | Production Rust/QML plus deterministic provider/IPC/layout fixtures; scoped live owned-server baseline. | Complete owned-server cases #16. Physical input/multi-output claims from #18 are excluded by the accepted support revision. |
| Source/auth/cache/privacy | Private credentials, account-scoped caches, offline read-only startup and cleanup/migration fixtures. | Live keyring/permission/revocation matrix #16; excluded server/account paths need an explicit support boundary. |
| ReplayGain/native gapless | Native FLAC extraction and 36 named player-side WAV/FLAC gain/transition cases. | Other embedded containers #30. Final named-device PCM/profile/continuity claims from #27 remain unverified and excluded. |
| Optional converted output | SoXR VHQ, explicit plans and precision/layout policies. | Continuous converted DSP state across boundaries #41; do not call reset/transient behavior transparent. |
| Recovery | Bounded pinned-version/visit recovery with synthetic native position checks. | Owned-server interruption/permission behavior #16. |
| Plex listening/reporting | [Typed provider transport and bounded nominal observer](plex-reporting.md), with fake-provider/HTTP tests and scoped live pause/resume/stop/duplicate-handoff qualification; preparation is excluded. | Broader owned-server cases #16. No played-state/history synchronization claim. |
| History | Bounded Previous path is implemented and separate from a complete archive. | Complete nominal history, retention/export/migration/privacy contract and storage #32. |
| Lifecycle/install | Ownership/rollback fixtures, installed launcher stubs, native process shutdown, private-bus activation, native widget and UI fixtures; software increment #45 complete. | In-use updates/rollback #46. Actual installed manager/logout/crash behavior remains unqualified and excluded from supported claims. |
| Physical/hardware claims | Explicitly separated from digital/sample and IPC-driven evidence. | #18/#27, outside the accepted narrower first-release claim. |

Evidence tiers are distinct: implementation, deterministic synthetic fixtures,
controlled live observations, and unverified/excluded external conditions. Exact
commits and CI/measurement records live in the linked issues and topic documents.

## Staged handoff and resolution

The [implementation queue](implementation-roadmap.md) maps #21 and #23–#30 to their
focused follow-ups. #28 groups offline startup (implemented #31), history #32 and
packaging #33. Remaining software work includes #30/#32/#41/#46; the support
proposal does not cancel it. #45's remaining real-manager qualification is separate
from its implemented shutdown and activation changes.

Resolve #12/#1 only after this consolidation is reviewed against the explicitly accepted support boundary. Resolve the
production umbrella #21 only when its still-required implementation and accepted
acceptance criteria are satisfied. Never convert absent resources, a prototype,
accepted defaults or passing tests into a measurement claim.
