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
