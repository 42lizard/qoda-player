# Installed UI activation

The installed backend unit explicitly supplies `--ui-launcher <prefix>/bin/qoda`.
The backend advertises MPRIS CanRaise when a browser is attached or this configured
launcher is available. Development backends keep the existing attached-browser
behavior unless this argument is deliberately supplied. No launcher is discovered
from PATH or an upstream response.

Without an attached browser, MPRIS Raise starts a bounded helper request carrying
`show --expected-instance <backend-instance> --deadline <monotonic-ms>`. It never
translates into Play. The coordinator keeps handling playback/queue requests while
activation is pending, coalesces at most eight waiting desktop callers, and checks
the current desktop owner/deadline before acknowledging completion. Quit and lost
desktop ownership cancel the owned helper. Failed launch state is exposed in
`ui.launch_error`; commands never echo the configured path or upstream data.

A fenced installed-launcher request cannot start a backend: it requires an active
unit and the matching instance. Checks repeat before UI startup and IPC. Expired
requests stop before starting services. The helper deadline bounds activation;
the MPRIS Raise response allows twelve seconds, while ordinary media commands keep
their three-second bound.

Before starting an inactive UI unit, the launcher atomically writes a private
`$XDG_RUNTIME_DIR/qoda/ui-owner.json` containing format, release, backend instance
and expiration. It rejects modified/unsupported, symlinked, linked or non-private
records. The UI unit runs the installed `launch.py run-ui` wrapper. That wrapper
validates this handoff before creating Quickshell and monitors the same backend
instance afterward. Backend loss/replacement stops only its owned UI process;
it never starts or stops the backend. UI crashes remain stopped until the next
explicit invocation. Hide keeps the UI process alive. The unit requires the
backend without activating it, and participates in its explicit stop/restart and
graphical-session lifetime.

Fatal launcher configuration, unsafe/unavailable runtime paths, duplicate backend
ownership and engine initialization failures use exit 78. Missing executables at
the unit entry point use 127. Generated units prevent automatic retries of these
statuses and the orderly shutdown outcomes 74/75/76. Recovery remains an explicit
launch after repair. Backend crashes still use the bounded on-failure policy and
restore paused; actual user-manager crash/logout qualification is outside the accepted
experimental first-release service claim, as recorded in #45 and the
[support boundary](first-release-scope-proposal.md).

## Bar integration

The separate widget installer accepts an explicit installed prefix:

```sh
python3 tools/bar-widget.py install --enable --installed-prefix ~/.local
```

This opt-in backs up the bar configuration, sets `launcherPath` to the installed
`bin/qoda`, and clears the development path. Installed mode maps Open/Search/Queue
to `qoda show/search/queue`; failure remains visible in the popup and never falls
back to the checkout or starts a development runner. Popup open/close and transport
subscriptions remain independent of application activation. To return explicitly
to development mode, enable the widget without `--installed-prefix`, which clears
`launcherPath` and sets the checkout path. This implementation does not change the
already installed widget or desktop configuration during development.

## Checks

The private-bus `tools/check-activation.py` exercises a real backend's absent-UI
MPRIS Raise against a fictional launcher, verifies instance/deadline arguments,
error propagation, a responsive queue while launch is pending, and cancellation
on Quit. It creates a private D-Bus configuration without service activation.
`CARGO_TARGET_DIR` is supported; build the backend first.

Rust fixtures cover invalid configuration, coalescing, expired callers and helper
cancellation. Python fixtures cover fenced/expired activation, private owner-record
preservation, UI-wrapper generation loss and expired handoffs. The native widget
fixture checks installed show/search/queue and failure without development fallback.
No check accesses Plex, credentials, media or host service configuration. These
checks do not replace actual installed-unit/logout or physical-input acceptance.
