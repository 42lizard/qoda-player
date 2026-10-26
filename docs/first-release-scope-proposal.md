# First-release support boundary

**Accepted support revision, 2026-10-26.** The user selected a narrower first-release
scope, reviewed the concrete boundary, clarified that browser-PIN sign-in does not
mean an old Plex server, and agreed to proceed. This revision does not mark missing
tests passed or remove software bugs from the backlog. The remaining in-scope work
and evidence requirements below still apply.

## Supported path

| Area | First-release claim | Explicitly outside this claim |
| --- | --- | --- |
| Desktop | The measured Arch/Omarchy laptop stack, one `eDP-1` display at scale 1.6; full/compact/hidden state, QML controls and application IPC have automated/native evidence. | Certification of physical keyboard/pointer dispatch and focus return; multiple outputs, hotplug and other scale/device combinations. |
| Plex | One owned PMS over verified HTTPS using the tested browser-PIN sign-in path. The recorded baseline is PMS 1.43.5.11029-6bf8af3f0 with five discovered music libraries. | Shared/restricted/read-only account matrices, permission-loss tests on another account/server, JWT PMS access, and universal server-version compatibility. |
| Audio default | Desktop/PipeWire playback with explicit ReplayGain policy. Digital sample count/order/gain checks are qualified only at the named player PCM-file boundary and tested formats/rates. | A system-wide bit-perfect claim for PipeWire, final device PCM qualification, real-time device continuity, or DAC/analog fidelity. |
| Direct output | Keep direct ALSA strict and opt-in HQ conversion available as experimental profiles with configured/observed parameters and their documented guards. | Calling any unmeasured device/profile verified, or inferring physical fidelity from PCM-file tests. |
| Packaging/lifecycle | Owned source install/remove, checkpointed release switching and conservative compatible code rollback, plus the implemented on-demand launcher/units, have deterministic and native-process checks. | Stable installed-service/logout/crash qualification until it is observed in a disposable graphical session. Keep this integration development/experimental in the release boundary. |

Current host observations: Omarchy 4.0.6, Quickshell 0.3.2 (Omarchy distribution),
Qt 6.12.0 and libmpv client API 2.5.0. The sole active output is `eDP-1` at 1.6.
ALSA exposes built-in speaker/headset and HDMI PCMs; no external DAC or connected
second display is established. Historical observations are identified separately
from these current host observations. No account access or playback was performed
in preparing this proposal.

## Work that remains required

This is an acceptance/support-matrix reduction, **not a feature cancellation**.
The independent implementation backlog stays required: confirmed Plex reporting
#26, wider embedded gain metadata #30, complete nominal history/export #32,
continuous converted DSP across track boundaries #41, and safe checkpointed
updates/schema-aware rollback #46. Existing code must still satisfy its contracts;
the proposal does not turn defects or failed tests into exclusions.

The owned-server path still needs its remaining controlled live checks under #16.
Playlist occurrence identity/mutation results, credential lifecycle, recovery and
reporting may only receive a live-qualified label after being observed. Synthetic
fixtures remain evidence for implementation, not a replacement for those results.
No real music starts automatically; any live scenario requiring playback must use
an explicitly agreed test session. Credentials stay in the existing private
backend/keyring boundary.

## Gate disposition

- #18: resolve the first-release gate by deliberately limiting the supported claim
  to the automated/native single-output evidence. Record physical/multi-output
  cases as excluded and future qualification, rather than recording a pass.
- #27: retain the player-side digital qualification and experimental direct-output
  status. Remove final device/continuity measurement as a first-release blocker;
  record those measurements as future qualification, with no analog claim.
- #16: narrow the required server/account matrix to the owned browser-PIN sign-in path.
  Finish its remaining in-scope owned-server cases; do not declare all Plex
  versions, authentication paths or permissions supported.
- #45/#33: retain installed services as experimental pending actual qualification;
  complete remaining implementation and ownership requirements. Do not claim the
  service-manager/logout tests ran merely because generated units parse.
- #12/#1: consolidate the accepted specification against this explicit boundary,
  preserving the distinction between implementation, automated fixtures, historical
  live observations and excluded/unverified external conditions.
- #21/#28: remain open until their still-required implementation children are done.

Gate closures must link this explicit revision and retain their unmeasured cases
as exclusions, not successful tests. The production release is not complete while
its remaining software and owned-server acceptance work is open.
