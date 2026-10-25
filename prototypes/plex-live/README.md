# Disposable live Plex validation harness

**Work in progress. The live validation ticket remains open.** This is planning evidence, not the production Plex backend or a completed compatibility matrix.

The initial live probe successfully created a device-key PIN (HTTP 201). Browser authorization has not yet completed in the captured record, so no PMS version, library, account renewal, playlist capability, or audio-media compatibility has been accepted.

## Run

```sh
python3 prototypes/plex-live/validate.py
```

Requirements: Python 3 with `requests`, OpenSSL with Ed25519 support, Linux `memfd_create`, `secret-tool` and an available Secret Service, and Brave. A fresh device key is generated in memory; a fresh PIN is requested from Plex's documented clients service. The script opens Plex's authorization page in Brave and prints a **local validator URL only**, never the PIN, device JWT, private key or account token.

1. In Brave, complete the **Qoda Plex Validator** authorization directly with Plex. No password/code/token is copied into chat or the terminal.
2. Return to the printed local page and choose **one authorized server**. Server names, addresses, identifiers and raw responses remain local/in memory. No server is automatically selected.
3. Review the bounded read-only observations. This scope reads music metadata and a small artwork response prefix; it does not transfer music files, edit playlists, change server settings or report playback/history.

Authorized credentials are stored in a dedicated Secret Service item identified by `application=qoda-plex-validation` and a fresh per-run identifier. No unrelated application's keyring entry is read. If storage is unavailable, the narrow harness records session-only mode; actual fallback/unlock acceptance remains unverified. Do not copy this minimal behavior as the production sign-in UX.

Use the local **Clear local validation credentials** action after checks finish; it deletes this run's local item and clears its in-memory account references. Cleanup races, keyring failure and stale-work invalidation have not been qualified in this minimal harness, so this action is not accepted production sign-out behavior. Remote device revocation is separate: remove **Qoda Plex Validator** in Plex Authorized Devices. Ctrl+C stops the local helper; it does not silently revoke the device or promise deletion of a previously stored keyring item. No plaintext credential file is used.

## Deliberately narrow first pass

- Fresh Ed25519 JWK PIN creation, signed-device-JWT polling and completed browser exchange when actually authorized.
- Own-item Secret Service round trip and immediate nonce-based renewal when authorized.
- Account resource discovery; human selection of one owned/shared server.
- Advertised certificate-verified HTTPS endpoints, trying direct before relay. No HTTP opt-in, insecure certificate bypass or silent legacy-token fallback is enabled.
- PMS identity; representative music-library pages (at most two libraries, two requested five-track pages each), schema field names and optional gain-related field presence only.
- A bounded authenticated artwork prefix request. No raw personal fixture or artwork is exported.

The metadata candidate requests API `1.2.3`; a response failure on an older server does not establish that version's capabilities or authorize inventing compatibility. The current first-party reference says API `1.2.3` is supported in PMS >= 1.43.4, whereas an omitted header requests legacy `0.0`. Requested version, document version and observed server version must be distinguished in the final matrix.

No seven-day expiry wait, remote revocation, shared-permission changes, controlled smart/read-only fixture, disposable playlist CRUD/reorder/duplicates, original-media preparation/Range/reconnect, playback reporting, analyzed/unanalyzed gain provenance, or full locked-keyring/sign-out matrix has been exercised by the initial pass. Those cases remain requirements of the open live-validation ticket, not implied passes. Playlist mutation requires a separately designated disposable playlist scope; this harness contains no mutation code.

## Safe evidence

`evidence/progress.json` exports a whitelist of operation names/outcomes, HTTP status, schemas/counts and tested public version strings. It does not export account/server/library names or identifiers, addresses, raw bodies, tokens, keys, PINs or signed URLs. The running helper updates that file as work proceeds; committed snapshots must be reviewed before publishing. Public comments must describe pending access honestly.

`evidence/preflight.json` records separate non-account checks: the default keyring was unlocked; a unique fictional marker was stored, retrieved and deleted from its own item; an isolated subprocess with an unreachable bus failed without returning a secret. The actual desktop keyring was never locked/stopped. These checks do not establish real account credential storage, locked-keyring UX, or session-only fallback correctness.

An ephemeral two-stage local sign-in helper may be generated for the human, but is not committed: browser sign-in and server selection produce no `.env` values, CI secrets or chat credentials. No setup wizard can replace actual browser authorization.

## Primary references

- [Plex Media Server reference](https://developer.plex.tv/pms/): Ed25519 JWK PIN flow, device JWT headers/claims, nonce renewal and API version negotiation. Reference reviewed 2026-10-25; document version 1.2.3.
- [Plex resource discovery](https://support.plex.tv/articles/206721658-using-plex-tv-resources-information-to-troubleshoot-app-connections/): advertised connections are candidates, not proof of reachability.
- [Secret Service specification](https://specifications.freedesktop.org/secret-service/latest/): credential storage service; only this harness's own item is used.

Canonical task: [Verify live Plex authentication and server contracts](https://github.com/42lizard/qoda-player/issues/16). The final specification stays blocked until actual supported scope and the missing measurements are resolved.
