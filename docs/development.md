# Running the first application slice

This is development software, tracked in [issue 21](https://github.com/42lizard/qoda-player/issues/21). It contains a real Rust process, Plex provider, embedded libmpv owner and standalone Quickshell window. It does not install a service or modify Omarchy configuration.

## Requirements

Rust/Cargo 1.99 or newer, libmpv development/runtime libraries (tested with mpv 0.41), Quickshell (tested with 0.3.2), Qt Quick Controls, a running PipeWire desktop session, and Brave for the sign-in button. TLS certificate verification stays enabled. The launcher requires Python 3. The optional local validation-login import also requires `secret-tool` and that specific existing keyring item.

## Start and stop

From the repository:

```sh
./tools/dev.py
```

Select **Connect Plex (this session)**, open the browser sign-in, authorize Qoda, then choose a library. Search applies to that library. Browse pages contain at most 100 tracks; search returns up to 100 matches. **Play** explicitly selects and starts one track. Pause, resume and seek in seconds act on the backend. Resume after the engine has stopped starts the selected track again.

The window's **Hide** button and compositor close request hide the interface. Playback belongs to the Rust process. To show it again:

```sh
quickshell ipc --path ./ui call -- control show
```

The foreground launcher attempts to restart an exited UI up to three times while retaining the backend. QML reloads and separately started replacement windows reconnect to the same state. **Quit** stops the backend; Ctrl+C stops processes started by that launcher. A second launcher shows the existing window. To stop the backend from another terminal:

```sh
./tools/dev.py --quit
```

For this machine's previously authorized validation account, an explicit development option reuses its existing Secret Service credential in memory:

```sh
./tools/dev.py --use-validation-login
```

That import reads only `application=qoda-plex-legacy-validation`, passes the decoded credential over a private stdin pipe and does not copy it into application configuration. It does not alter or delete the validation item. Missing or locked credentials can require an explicit sign-in instead. Do not put Plex tokens in command arguments, environment variables or committed files.

The backend can run separately with `./tools/dev.py --backend-only`; start a replacement interface with `quickshell --path ./ui --no-duplicate`. `--credential-stdin` on the backend accepts a bounded JSON credential from a private pipe for development; normal sign-in does not require it.

## Implemented boundaries

- The backend uses an owner-only runtime directory and socket, a process lock and same-user peer checks. Protocol 1 requires a hello and an instance identifier for subsequent commands. Revisions and provider offsets use decimal strings. Snapshots carry display metadata and source-scoped track references, never Plex credentials or upstream media URLs.
- Catalog and media preparation run outside the coordinator. Superseded browse/preparation requests are cancelled; generation checks reject stale completions. Disconnect cancels active work and revokes media access.
- libmpv receives a revocable, unguessable loopback media grant. The backend authenticates HTTPS requests to Plex, refuses HTTP redirects and streams validated single byte ranges. Tokens remain in the HTTP layer. The proxy supports GET/HEAD and only exposes necessary media headers.
- Playback defaults to Desktop/PipeWire with ReplayGain off. The displayed position is a nominal libmpv audio clock, not a hardware presentation or listening-history guarantee. The displayed selected track is not used as a claim that its samples have reached the output.
- Only nonsecret source identity mappings are persisted in `$XDG_CONFIG_HOME/qoda/source-identities.json` (default `~/.config/qoda/`), using an atomic replacement with mode 0600. Sign-in and playback selection remain session-only. Startup stays paused and does not load music.

## Still to implement or qualify

Saved application sign-in and recovery, durable session restoration, queue management, playlists and Plex writes, artwork/cache storage, compact surface, MPRIS, desktop installation, gapless transitions, ReplayGain and native fidelity output remain outstanding. The browser PIN flow is implemented; the current live checks reuse the previously authorized compatible credential and do not qualify a fresh browser round trip or JWT support. Accessible HTTPS sources are shown; this does not assert that offline/inaccessible servers are absent from the account.

The first window is a track-browser slice; it is not the complete album-browser and persistent-queue design. Issue 21 remains open while real media playback and remaining acceptance are qualified. Real library music is never started by automated checks.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The 18 tests cover existing core state/fences, bounded private IPC (including a partial frame interrupted by an update), provider endpoint/metadata/secret handling, media range/host/grant revocation and cancellation, and native loading, pause and failure with generated silence through `ao=null`. Native tests never open a real audio output.

On the development machine, a separate read-only check connected to five real Plex music libraries, loaded a 100-track page and obtained search results. The actual native QML window connected, displayed the catalog, hid/reappeared, and reconnected after a UI process restart with the same backend instance and catalog. Startup remained paused, no QML runtime errors were recorded, and scoped processes were stopped. This is not a real-music playback, gapless or fidelity acceptance result.
