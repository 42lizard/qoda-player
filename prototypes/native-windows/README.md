# Native windows and focus — throwaway Quickshell study

Question: can stock Quickshell implement the selected normal tiled browser plus compact layer panel while preserving navigation and releasing keyboard ownership correctly?

This is a fictional UI and a root-owned timer, not a music player or backend. There is no Plex access, audio, persistence, desktop shortcut installation, or permanent desktop configuration change. Keep it on `prototype/native-windows`, with no PR/merge.

## Run

From this branch's checkout:

```sh
python3 prototypes/native-windows/run.py
```

One command starts Quickshell with the Basic controls style for that process and opens the full study window. Hide keeps it running; use its Quit button or Ctrl+C in the launching terminal to stop the prototype. Root state starts afresh after a process exit.

To run the application-IPC observations and exit the test instance:

```sh
python3 prototypes/native-windows/run.py --check
```

Close an already running instance of this prototype before starting another run. The launcher and IPC calls select only this config path; they do not operate the Omarchy shell instance. Transient logs go into ignored `.scratch/native-prototype.log`.

## Evidence and measured scope

- [Summary](evidence/summary.md).
- [Full app state and filtered compositor observations](evidence/results.json).
- [Recorded startup warning](evidence/startup-warnings.txt).

On the recorded Quickshell 0.3.2 development build, Qt 6.12 and Hyprland 0.56.2, one eDP output at scale 2 was available. All 32 scoped checks were observed through the prototype's **application IPC**, Qt window/focus properties and read-only Hyprland diagnostics. No physical key or mouse input was injected, and physical keyboard counters remain zero. The checks therefore do not prove actual Tab, shortcut, Escape or pointer dispatch.

The full surface is a `FloatingWindow`, observed as a non-floating native tiled client on the active normal workspace. At about 581 logical pixels it hides the persistent queue and provides a drawer. Client maximize requests did not widen it, consistent with the installed Omarchy default `suppress_event = "maximize"`. A temporary fullscreen request from this prototype provided a 1,200-logical-pixel width to check the persistent queue, then returned to normal tiling. This is measurement instrumentation, not a recommendation to make the production browser fullscreen or override compositor policy.

The compact `PanelWindow` was observed as a 420×240-logical-pixel overlay on the invoking output, with unchanged compositor reserved areas. A passive show uses keyboard mode None and leaves the previous toplevel focused. An intentional keyboard-oriented show briefly primes Exclusive focus, settles to OnDemand after 90 ms, and focuses a native control. Hiding immediately returns the keyboard mode to None; rapid hide/reopen cycles left no mapped panel or keyboard ownership. The compositor returned focus to the prior toplevel in the recorded sequence.

Dialog/drawer dismissal precedence and full-form Escape behavior were exercised by calling the **same handler through IPC**, not by claiming a real keypress. Native Qt window close generated the close-as-hide event, preserving the fake playback flag, page, query, selection and scroll position. Explicit Quit removed the prototype instance and surfaces. The fake timer advancing while hidden establishes root lifetime only; real backend/audio survival is a different integration test.

Qt emitted a host-portal registration warning (connection already associated with an application ID); no QML runtime error occurred and the recorded surfaces/focus checks completed. The observed native class is `org.quickshell`. Product identity, portal integration and production activation still need the desktop lifecycle/distribution decision; do not assume a product-specific app ID from a unique title.

## Hands-on acceptance still needed

These are instructions for actual interaction, not completed results:

1. Tab/Shift+Tab through controls and use Enter. Ctrl+K should focus search; typing spaces in search must not toggle playback. Use **Key test** to focus the bare surface, then Space should toggle the fake playback flag.
2. Open Test dialog and press Escape. The dialog closes first, leaving its full/compact form visible. Escape without a popup leaves the full form open; in compact it hides the panel and returns focus.
3. Expand/Compact/Hide, close the full window with the compositor, and restore through the config's toggle command. Page/query/selection/scroll should remain. Confirm actual clicks outside the compact panel can reach other applications and do not leave a grab.
4. On multiple outputs, invoke compact from each output and check it appears on the invoking monitor without moving while interacting elsewhere. Test output removal and reconnection.
5. Check supported scale changes and narrow/wide sizes for clipping, readable text, pointer hit targets and preserved navigation. Current live evidence covers only one 200% output.

The study exposes `probe` IPC methods for state inspection and these transitions. For example:

```sh
quickshell ipc -p prototypes/native-windows/shell.qml call probe snapshot
quickshell ipc -p prototypes/native-windows/shell.qml call probe toggle
```

Primary interfaces: [Quickshell windows](https://quickshell.org/docs/v0.3.0/types/Quickshell/QsWindow/), [layer keyboard modes](https://quickshell.org/docs/v0.3.0/types/Quickshell.Wayland/WlrKeyboardFocus/), and [Qt window activation](https://doc.qt.io/qt-6/qwindow.html#requestActivate). The installed read-only Omarchy `Ui/KeyboardPanel.qml` supplied local precedent for brief Exclusive priming followed by OnDemand; the prototype implements its own minimal surface without importing or copying that widget.

Decision context: [Validate native player windows and keyboard focus](https://github.com/42lizard/qoda-player/issues/14). The selected UI and architecture contracts remain the canonical product requirements.
