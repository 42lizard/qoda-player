# Owned Plex qualification follow-up, 2026-10-26

These are measured, bounded cases on a validation branch, not a release pass.
No music deletion/edit, playlist mutation, timeline or scrobble was sent here.
Credentials, private endpoints and device/account identifiers are absent from the
retained artifacts. The existing dedicated legacy keyring item was read only.

| Case | Observation | Limit |
| --- | --- | --- |
| Fresh strong browser PIN | Human approval in Brave returned a distinct token for the original account; an exact new device candidate was found. | Harness incorrectly required a token field omitted by devices.xml and stopped before removal. `fresh-auth-evidence.json` is deliberately a nonpass; its cleanup-identity flag identifies the device candidate, not a verified token association. No browser token revocation result. |
| Fresh SDK short-PIN control | Link 204; matching creation/poll PIN and client; distinct opaque token/same account; exact new non-server device; child/parent PMS initially 200. | A separate control, not the browser handoff. Server package/version was not rerecorded for this control; keep the historical versioned baseline distinct. |
| Targeted short-client removal | DELETE 200; exact entry absent; child cloud 401; parent cloud/PMS 200. | Child PMS still 200 after 30.8 seconds. Removal is not immediate PMS authorization invalidation. No account-wide logout, migration or expiry claim. |
| First browser fixture cleanup | One same-product fresh client in first-run creation window, distinct from the original device, uniquely reidentified after short-client removal. DELETE 200, absent, original device/cloud still present/200; remaining qualification devices 0. | Cleanup only: its lost in-memory child credential was not reused to test PMS revocation. |
| Private Secret Service | Isolated real daemon: store/read, lock and IsLocked rejection, restart/unlock and persistent restoration, replace/read, scoped clear/absence. No-service and invalid bus refuse. | Synthetic marker only. Private bus/PID/filesystem namespace and XDG paths; no desktop daemon/PAM/prompt claim. Original keyring is untouched. Older failed unlock result remains historical, not rewritten. |

The original browser-PIN/library/catalog/artwork/plain-playlist/Range baseline
and limitations remain on [`validation/plex-live`](https://github.com/42lizard/qoda-player/tree/9ef8a40/prototypes/plex-live).
It did not establish saved duplicate support (the tested server collapsed it) or
temporary source-queue cleanup. Production #47 now explains an unchanged duplicate
append without retrying or clearing the uncertainty fence. Existing queue repeats
remain distinct. Gain uses verified embedded tags, with unknown Plex units excluded.
The [reporting branch](https://github.com/42lizard/qoda-player/tree/e3af727/prototypes/plex-reporting)
separately records null-output nominal pause/resume/stop/duplicate-handoff results.

Still pending here: controlled current-stream interruption/recovery on an actual
owned source; application-level session-only/disconnect cleanup and cache checks;
actual artwork-change invalidation or an explicit limit on that unmeasured claim;
consolidation into a supported/untested matrix. Fresh Range requests do not prove
interruption recovery. Shared/restricted/other-account loss, JWT PMS and universal
server compatibility remain excluded by the accepted support boundary.

## Owned native/cache follow-up

`stream-recovery-evidence.json` completes the named bounded plan against PMS
1.43.5.11029-6bf8af3f0 over verified HTTPS. The first two retained nonpasses
(`stream-recovery-preflight-nonpass.json`, `artwork-reconnect-nonpass.json`) stopped
before any playback: the setup assumed reconnect retained artwork, but successful
account connection deliberately purges it. Album browsing returned 100 rows and
100 local grants. No source behavior or cache policy was changed to make those
attempts pass.

The final case expires one newly created private JPEG file and uses a
validation-only memory eviction within the same account/session. Production
cache/refresh code serves the stale disk bytes, fetches a valid owned provider
JPEG, rotates the local URL, refuses the old URL (404), and writes fresh disk
mtime. Bytes happened to match. Remote artwork was not edited; changed remote
pixels/push invalidation remain unmeasured and unclaimed.

The sole playback run uses the same uniquely reidentified intact 19737 ms item.
After a confirmed 551 ms nominal clock, one validation-only current proxy body
returns Network while preserving all original metadata/headers/bytes until then.
The actual recovery coordinator performs one refresh, retains source/reference,
occurrence, session, visit and pinned version, and opens a new render generation
with confirmed 951 ms position. Four proxy-body opens, including three nonzero
Range opens, were observed. This measures controlled client transport recovery
with real Plex original-media requests, not a physical network outage. Explicit
Pause retains occurrence/visit at approximately 992 ms.

Disconnect then clears visible library/catalog, queue/archive and metadata rows,
revokes the artwork URL (404), removes its private JPEG and finishes cleanup.
The run ends after 3.97 s with native exit 0 and no speaker AO. A fresh process on
the same private XDG profile receives no credential: it remains disconnected,
empty and paused, then exits 0. Desktop keyring remains unavailable to both
processes; no Save/Forget operation is requested. Afterward the original track is
present and its observed offset/count/last-viewed fields are unchanged. No music,
playlist, metadata, artwork edit or scrobble is sent. Production nominal timeline
reports were active, so this is no guarantee of absent history effects.

Both owned native processes stop and their private state/logs/JPEG are removed.
The validation branch's forced-null player, body wrapper, observation fields and
memory-eviction command are instrumentation only and must not be merged. Targeted
warnings-denied Clippy and Python syntax checks pass; the live case is separate
from automated, synthetic production checks.
