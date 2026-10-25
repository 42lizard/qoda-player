# Live Plex validation — bounded owned-server evidence

**Throwaway validation, not the production Plex backend. The live acceptance ticket remains open.**

Tested PMS: **1.43.5.11029-6bf8af3f0**, one explicitly authorized owned server. Server/library names, addresses, identifiers, titles, raw responses, PINs, keys and tokens are excluded from the public artifacts. The human completed both browser authorizations; their standing acceptance of recommended defaults selected the sole owned server and newly created UUID-named disposable fixtures. No pre-existing playlist or remote queue was a mutation target. No playback-history/timeline report or audible playback was sent.

## Outcomes

| Path | Observed result | Boundary |
| --- | --- | --- |
| Ed25519 JWK PIN + device JWT | Browser exchange and nonce renewal 200; cloud discovery 200 | Cloud authentication works |
| JWT against this PMS | 401 with account/resource JWT, minimal/documented claims and omitted/explicit API headers | JWT-only PMS access is not qualified here |
| Separate legacy browser PIN | Opaque token stored and verified in its own keyring item; PMS and resource access 200 | Explicit compatibility experiment, no silent token substitution |
| Discovery/catalog | Five music libraries; adjacent requested five-track pages with correct offsets and no overlap | First library sampled; not a whole-catalog scan |
| Search/artwork | Music search returns tracks; authenticated JPEG prefix 200 | No real artwork-change/conditional-invalidation proof; sampled response had no ETag/Last-Modified |
| Plain playlist | Create, read, rename, append distinct track, reorder occurrence, remove occurrence, delete verified | Only fresh UUID test objects; all fixture playlists removed |
| Duplicate playlist creation | Existing-track append, repeated-ID create URI and repeated-queue import each collapsed duplicates | Do not infer a new occurrence from HTTP 200; actual duplicate playlist-entry operations remain unverified |
| Smart fixture | Metadata flagged smart; attempted plain-item URI edit returned 400 | Not a general permission-loss/read-only-account test; smart PUT can edit rules |
| Original preparation | `Generic` profile changed the decision response from 400 to 200, `mdeDecisionCode=1000`, `directplay` | Selected stereo FLAC only; not a universal client-profile guarantee |
| Range/seek primitive | 206 for bounded start/later ranges; a fresh connection returned the same later bytes | In-memory bytes only; not a simulated network-drop or real-time handoff test |
| Gain metadata | `gain`, `peak`, `albumGain`, `albumPeak`, loudness-related fields present | Units/provenance unverified; no tag found in bounded prefix does not establish tag absence |
| Own JWT lifecycle | Expired signed device JWT and identical nonce replay rejected 422; own device removal verified; issued JWT then 401 and renewal 422 | Retired only the fresh JWT test device; actual seven-day account-token expiry not waited out |
| Credential cleanup | Retired JWT item absent; its helper stopped; separate working legacy item retained | No unrelated device/keyring entry touched |
| Keyring encoding | Old JSON envelope was observed with invalid literal PEM control characters; versioned ASCII envelope verified, restored private key renewed successfully; six fixture round trips matched | Base64 is serialization, not encryption; root text-transformation cause is not assigned to a vendor |
| Isolated locked keyring | Private bus/temp XDG collection rejected locked access with `IsLocked`; real desktop keyring stayed unlocked | Headless unlock attempt returned 0 but did not unlock; failure retained, actual unlock UI/session-only integration unverified |

Full observations are in `evidence/*.json`. Initial failures are preserved. This is a compatibility record, not an all-pass suite or universal support claim. API-header response success is distinguished from proof of every negotiated-version semantic.

## Run / reuse

Requirements: Python 3 (`requests`, `dbus` for private-keyring fixtures), OpenSSL Ed25519, `secret-tool`, Secret Service, Brave, and `ffprobe` for the optional bounded media-prefix tag observation. No application install or desktop configuration change is needed.

```sh
# Fresh recommended JWK/JWT browser experiment (known PMS failure on this stack):
python3 prototypes/plex-live/validate.py

# Fresh explicitly labeled compatibility browser experiment:
python3 prototypes/plex-live/compatibility.py

# Reuse the retained, authorized compatibility credential for read-only checks:
python3 prototypes/plex-live/read_probe.py --legacy
```

The browser helpers print a loopback status URL and provide an **Open Plex sign-in** link. Sign in directly with Plex; no password, token or code is pasted into chat or the terminal. A fresh launch registers a fresh test client, so reuse the retained credential for subsequent reads instead of needlessly signing in again.

Credentials live only in memory and dedicated keyring items (`application=qoda-plex-validation` or `application=qoda-plex-legacy-validation`, plus their per-run identity). The codec stores a versioned Base64 envelope inside Secret Service and verifies the returned bytes. No plaintext credential file, GitHub secret or `.env` value is created. Auth/key material is never a command-line argument or diagnostic output. The separate retired JWT item has been cleared; the compatibility item is deliberately retained for continued authorized work.

The initial helpers are limited diagnostic surfaces, not accepted production unlock/sign-out UX. Their complete failure/race/persistence matrix remains required. Remote revocation is distinct from local cleanup; `auth_lifecycle.py` is a one-shot test against its own stored JWT device and was completed before clearing that device's local item. Do not expect it to re-run after retirement without a fresh test-device authorization.

Other scripts perform bounded experiments:

- `auth_diagnostic.py`: private-key restoration, documented-claim renewal, server identity/timing and header/scope comparisons. Retired JWT credential is now intentionally absent.
- `playlist_probe.py`: explicitly designated **new** UUID fixture playlists only, guarded by returned IDs and expected names. No automatic mutation retry. Cleanup manifests are private runtime files; all playlists from the observed runs were deleted and checked as missing.
- `duplicate_queue_probe.py`: new temporary queue only; verifies queue duplicates and tries importing them into its own disposable playlist. The imported playlist was deleted. **Clearing that temporary queue returned 200 but its follow-up read still showed contents; empty-state cleanup is unverified.** No existing queue was cleared. Future runs retain the returned own queue ID in a private runtime manifest if cleanup verification fails; never guess/sweep other queue IDs. A Plex play queue is not Qoda's authoritative local queue.
- `media_probe.py`: selected original decision and bounded in-memory FLAC Range requests, then a bounded-prefix tag probe. No media file is saved or playback reported. The initial timeout and 400 decision are recorded; profile comparison isolated the successful request.
- `isolated_keyring.py`: fictional data on its own private DBus and temporary XDG directories; never locks/stops the desktop keyring. Both the locked-access success and headless-unlock failure are retained.

## Remaining acceptance gates

No shared-server/access-loss fixture is available in this authorized account. No existing track was designated for history-altering timeline/scrobble tests. Do not modify sharing, other users, real listening history or library files to manufacture a pass.

Still required: shared/restricted/read-only permission changes; playback reporting on a controlled track; actual outage/failover/drop recovery; real artwork replacement/invalidation; broader formats/album ordering/metadata cases; actual account-token expiry behavior; complete keyring unlock/session-only/sign-out races and durable cleanup; verified Plex gain units/provenance or deliberate non-use; actual duplicate playlist-entry behavior if supported; and the temporary-queue cleanup discrepancy. These remain explicit in the open ticket and final specification gate.

The practical first-release candidate from this setup is **explicit legacy browser authorization**, original playback with a valid client profile, source-scoped occurrence IDs, refresh/reconcile after playlist edits, and embedded/verified-only ReplayGain. This is input to the final contract; it does not silently replace the accepted provider/storage/fidelity policies or imply universal compatibility.

## References

- [Plex PMS reference](https://developer.plex.tv/pms/): JWK/JWT flow, nonce, playlist/play-queue routes, occurrence IDs and playback decisions. Reference document 1.2.3, reviewed 2026-10-25.
- [Plex resource discovery](https://support.plex.tv/articles/206721658-using-plex-tv-resources-information-to-troubleshoot-app-connections/).
- [Secret Service specification](https://specifications.freedesktop.org/secret-service/latest/).
- [Python-PlexAPI source](https://github.com/pkkid/python-plexapi/blob/master/plexapi/myplex.py): community reference-client device-selection/removal route, not a Plex support guarantee. Only this harness's exact non-server device was targeted in the observed removal.

Canonical task: [Verify live Plex authentication and server contracts](https://github.com/42lizard/qoda-player/issues/16). This ticket and its final-specification blocking relationship remain open.
