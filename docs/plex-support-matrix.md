# First-release Plex support and evidence

2026-10-26. This completes the owned-path qualification record for #16 under the
[accepted support boundary](first-release-scope-proposal.md). It is a measured
compatibility matrix, not certification of every Plex server, account or format.
Continuous converted playback #41 remains required software work before release.

The historical and current controlled baseline is one owned PMS
**1.43.5.11029-6bf8af3f0**, verified HTTPS and five discovered music libraries,
using an explicitly approved browser PIN that returns an opaque credential.
“Legacy PIN” names this authentication mechanism; it does not mean an old PMS.
Private original credentials, server/account/track identifiers, endpoints, raw
responses and media are absent from public evidence. Validation instrumentation
is retained on separate branches and must not be merged into the player.

| Capability | Qualified observation | Boundary |
| --- | --- | --- |
| Browser authorization | Historical strong-PIN approval in Brave, opaque credential/keyring round trip, owned PMS access; a fresh approval also reached a distinct same-account credential. | The fresh harness stopped on its own missing device-token-field assumption before revocation. Its nonpass and later scoped fixture cleanup remain explicit. No automatic opaque-token expiry/renewal promise. |
| Credential substrate | Existing dedicated credential was read without replacement; a private real GNOME Secret Service passes synthetic store/read, lock rejection, restart/unlock persistence, replacement and scoped deletion; absent/invalid private buses refuse. | No desktop collection lock, PAM or user-prompt acceptance test. Original keyring item is intact; actual credential round trip and synthetic failure mechanics are distinct evidence. |
| Session-only/local sign out | Native credential-stdin connection succeeds with desktop keyring unavailable. Disconnect clears visible library/catalog, queue/archive, metadata rows and private JPEG, revokes its local artwork grant, and completes cleanup. New process on the same private profile without a credential stays disconnected, empty and paused. | Backend observation, separate from browser/QML/PAM acceptance. Save/Forget was not requested against the host keyring. General preferences remain. |
| Selected-device revocation | Separate fresh SDK short-PIN control: PIN/client/new-device binding, distinct opaque token and same account; DELETE 200 and exact device disappearance; child cloud 401, original cloud/PMS 200. | Owned PMS still accepted that child token (200) after 30.8 s. Device removal is not immediate PMS revocation. Control server version was not rerecorded; keep this result separate from versioned media cases. No account-wide logout/migration or expiry measurement. All new qualification devices are removed. |
| Catalog/search | Historical owned discovery, bounded catalog/search/JPEG reads; current native browse and 100-row album page with 100 lazy local artwork grants. | A sampled/bounded baseline, not a complete scan of every item, codec or page combination. Cache/provider/identity behavior also has deterministic fixtures. |
| Artwork cache | Current owned JPEG fetch, private disk expiry plus validation-only memory eviction, stale disk bytes served, background provider refresh, URL rotation/old URL 404 and fresh disk mtime. Disconnect removes JPEG and revokes the refreshed grant. | Remote artwork was not edited and bytes happened to match; no changed-remote-pixel/push-invalidation claim. Successful account connection deliberately purges artwork caches. Original failed reconnect-based test setups are retained. |
| Plain playlists | Disposable owned create/rename/append/reorder/remove/delete and identity checks, with fixture cleanup; smart fixture write refused. | Tested server collapsed saved duplicate tracks, including import from a duplicate-preserving server queue. Qoda preserves local queue repeats and reports an unconfirmed saved duplicate without retrying (#47). Servers returning distinct entries remain supported by typed occurrence edits. |
| Original media/Range | Historical original FLAC preparation and authenticated initial/nonzero Range reads; current native run refreshes the pinned owned source through actual provider requests, with four proxy-body opens and three nonzero Range opens. | Sampled original FLAC. No universal codec/transcode or physical network-outage claim. Unknown Plex gain fields are not used as ReplayGain. |
| Current-stream recovery | One controlled client-side body interruption after 551 ms confirmed nominal presentation; one recovery attempt retains source/reference, occurrence/session/visit and version; fresh render confirms 951 ms; explicit Pause retains the same pass. Run stops after 3.97 s, null AO, exit 0. | Intentional transport-wrapper fault with real Plex original media, not a real network outage. No physical output. Older fresh-Range-only observations are not relabeled recovery. |
| Timeline reporting | Separate null-output current playing/paused/resumed/stopped and duplicate-occurrence handoff cases, with bounded transport/cache timing. | Preparation is excluded. No scrobble, completion threshold, provider-history import or universal administrative/session-termination guarantee. No offset/count/last-viewed change observed in the recovery case; absent history effects are not promised. |

## Retained evidence

- [Historical live baseline, including failed/limited cases](https://github.com/42lizard/qoda-player/tree/9ef8a40/prototypes/plex-live).
- [Nominal reporting runs and timing follow-ups](https://github.com/42lizard/qoda-player/tree/e3af727/prototypes/plex-reporting).
- [Current isolated lifecycle/native/cache cases and limits](https://github.com/42lizard/qoda-player/blob/3e52fc5/prototypes/owned-qualification/observations.md), with [the executed recovery/cache plan](https://github.com/42lizard/qoda-player/blob/3e52fc5/prototypes/owned-qualification/stream-recovery-plan.md).

All current helpers stop and private XDG data, logs and cached JPEGs are removed.
The selected original 19737 ms track remains present. No music deletion, playlist,
metadata/artwork edit or scrobble was sent in these current cases; production
nominal timeline reporting was active during the bounded recovery run. The earlier
disposable-playlist tests performed their specifically named playlist mutations.
The older source-play-queue cleanup uncertainty remains recorded: imported test
playlists were removed, but source queue deletion was not established. Production
uses the local queue and does not require that import experiment or a claim that
its temporary source queue was purged.

## Unclaimed compatibility

Shared/restricted/other-account permissions and loss, JWT PMS, other server
versions, arbitrary codec/layout/source changes, remote artwork editing/push
invalidation, universal token expiry/revocation propagation and forced server
administrative termination remain unqualified. So do desktop PAM/physical-input,
multiple-display, permanent installed-manager/logout and final DAC/ALSA guarantees
identified by the accepted support boundary. These are not passing tests or
canceled software defects. Existing deterministic software gates and #41 still
apply; this matrix does not declare the whole release complete.
