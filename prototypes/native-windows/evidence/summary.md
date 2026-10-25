# Native surface observations

Native Quickshell windows through application IPC and read-only Hyprland diagnostics; no physical key or mouse injection.

| Check | Outcome |
| --- | --- |
| normal tiled workspace | Observed |
| full native map and 200-percent scale | Observed |
| full compositor focus | Observed |
| narrow queue is not persistent | Observed |
| scroll position established | Observed |
| programmatic focus-chain operation | Observed |
| wide persistent queue | Observed |
| queue drawer opened | Observed |
| queue dismiss keeps full form | Observed |
| full dialog opened | Observed |
| dialog dismiss keeps full form | Observed |
| full Escape handler leaves browser open | Observed |
| hidden maps neither form | Observed |
| fictional root ticker survives hiding | Observed |
| restores full and navigation | Observed |
| native close maps to hide | Observed |
| restore after actual close | Observed |
| compact passive no keyboard request | Observed |
| passive panel preserves previous toplevel | Observed |
| panel reserves no desktop space | Observed |
| compact is native overlay surface | Observed |
| compact selects invoking monitor | Observed |
| compact settles to on-demand focus | Observed |
| compact native active focus | Observed |
| compact dialog opened | Observed |
| first dismiss leaves compact | Observed |
| second dismiss hides and releases keyboard | Observed |
| compositor restores previous toplevel | Observed |
| toggle restores compact | Observed |
| rapid toggle leaves no keyboard grab | Observed |
| all visibility transitions retain selection | Observed |
| explicit quit ends prototype instance | Observed |

## Unverified

- Physical Tab/Shift+Tab/Enter/Space/Ctrl+K/Escape dispatch and actual pointer interaction: no UI automation surface is available.
- Multiple physical outputs, hotplug and scale changes: only one monitor at scale 2 is connected.
- Real playback/backend survival, large catalog performance and production activation/shortcut binding are separate work.
