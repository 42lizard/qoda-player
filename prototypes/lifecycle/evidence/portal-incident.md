# Portal side effect during lifecycle qualification

Observed 2026-10-25, Europe/Vienna. This was a test-harness isolation defect, not a Qoda playback result.

Five extra `/usr/lib/xdg-desktop-portal-hyprland` processes crashed with SIGSEGV at approximately 15:53:33, 15:54:16, 15:55:07, 15:55:57 and 15:56:42 CET (PIDs 862449, 863410, 866503, 868131, 869434). The initial study used `dbus-daemon --session`, which loads standard activation service directories. Quickshell's toolkit attempted to access portals on that private bus. The study's bus shutdown was followed by the extra processes crashing.

Core evidence from PID 869434 establishes that its `DBUS_SESSION_BUS_ADDRESS` and `DBUS_STARTER_ADDRESS` point to the study's private socket, with starter bus type `session`. The stack contains `wl_proxy_marshal_array_flags`, `wl_proxy_marshal_flags`, unsymbolized portal frames and libc `exit`. This supports a Wayland teardown failure in the extra portal process. It does not identify the exact invalid object or prove the underlying portal source defect. Debug symbols were unavailable in that bounded inspection. The extracted core copies were deleted and are not published.

The original main desktop portal and Hyprland portal services remained active with their original PIDs and zero restarts. The original document portal exited with status 21 at 15:53:24, during the first study. Its connection to the activation side effect is inferred from the timing; the exact internal failure is not established. It was restored with `systemctl --user start xdg-document-portal.service` and confirmed active/successful. No desktop configuration was changed. No claim is made about absence of disruption to in-flight portal operations.

The lifecycle study was interrupted and its temporary units/files were removed. The initial registration warnings were incorrectly treated as an identity warning without checking contemporaneous coredumps; the crashes were established after the human called them out.

## Harness correction

The private bus now uses a minimal explicit configuration with EXTERNAL authentication and no service directories or activation includes. `portal_guard.py` verifies `ListActivatableNames` contains only the bus itself, and actual `StartServiceByName` attempts for the desktop portal, Hyprland portal, document portal and Secret Service fail with ServiceUnknown. This check is mandatory before starting the UI. The study also compares the real host portal service states/PIDs before setup and after cleanup.

`python3 prototypes/lifecycle/portal_guard.py` passed: all four activations denied; the four host portal services remained active with unchanged PIDs. This prevents this harness from starting portal processes; it does not fix the portal's underlying exit-time crash. The old configuration was not deliberately rerun after diagnosis because it had already caused repeated user-visible faults and document-portal disruption.

Actual portal features on the real session bus are still unqualified. The no-activation bus deliberately cannot establish real portal feature compatibility, and successful `AppId`/compositor observations do not establish that compatibility.
