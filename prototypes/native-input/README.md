# Native physical input acceptance study

Throwaway validation artifact for [Verify physical keyboard and multi-output acceptance](https://github.com/42lizard/qoda-player/issues/18), derived from `prototype/native-windows` at `826cd7e`. No Plex, audio, persisted session, service installation or desktop configuration changes.

Run `python3 prototypes/native-input/run.py` in a graphical session. The native **Qoda · physical input checklist** window opens. Click **Start / Prepare**, perform each instruction in the player, then select the observation that matches what happened. The helper remains available while the player hides. Closing the helper or choosing Quit stops this study.

Use only the fictional search fixture **cedar river**. Raw search text and arbitrary key text are never exported. Captured keys are limited to Tab, Shift+Tab, Enter, Space, Escape and the Ctrl+K shortcut; sources are this artifact's control names. Pointer observations contain no coordinates. No screenshots or unrelated application titles are collected.

## Evidence and interpretation

- `evidence/observations.json` is a live sanitized state snapshot, updated approximately twice per second. It includes known native input callbacks, helper activity, focus, geometry, form, fake playback, close count, search-fixture equality and navigation state.
- `evidence/human-reviews.json` records explicit on-screen reviews with the state at confirmation. These are human reports, not automatically checked assertions.
- `evidence/baseline.json` is the initial pre-interaction state. Zero input events and zero reviews mean acceptance is pending.
- The IPC surface offers read-only snapshots/reviews and Quit. It cannot advance the checklist, inject keys, change focus, invoke close, dismiss popups or award a pass.
- A native Qt callback establishes delivery to this app. It cannot independently authenticate a physical device or distinguish external input injection. Human performance of the requested action supplies that context. No injection is used by this study.
- Popup Escape may be consumed by Qt's popup handling before the observers run. Absence of a recorded key is not a pass or failure; review actual popup/form behavior. The checklist deliberately combines observations with native state, rather than claiming complete automated event coverage.
- The study fixes the prior native prototype's Escape typo (`escape` called the JavaScript global rather than `handleEscape`). Bare Escape/Space ignore auto-repeat. Observed buttons explicitly adapt Return/Enter to their clicked signal; this is an adapter feasibility check, not a claim about unmodified Qt buttons. Space continues through normal control handling; the bare surface uses the player handler.
- Start seeds Artists, selected item 26 and scroll position as a synthetic restoration fixture. It does not count as performed navigation. Compare the before/after snapshot for restoration.
- Wide review temporarily uses fullscreen. Its in-player **Finish wide review** button exits fullscreen and returns to the helper. This explicit review transition is not evidence for compact Escape focus return.

## Environment and limits

Observed on 2026-10-25: one connected `eDP-1`, 2400×1600 physical pixels, logical 1200×800 at scale 2. No second output is available. Native computer automation is unavailable. Existing compositor Close window is Super+W; this study installs no bindings.

Candidate acceptance scope is this single built-in display at 200%. It remains **unverified until actual interaction is recorded**. Other outputs, other scales, fractional scaling, cross-output placement/focus and hotplug are explicitly unverified. A helper window is the known prior application for the compact focus-return check; arbitrary third-party applications are not qualified by it.

Startup loads successfully. The existing host-portal application-ID warning is still present; product identity/portal integration belongs to the lifecycle decision. No physical acceptance results have been inferred from successful startup or from previously successful IPC checks.

This branch is not production code and is not intended to be merged. The decision ticket stays open until observations are assessed; clicking through a checklist alone does not close it.
