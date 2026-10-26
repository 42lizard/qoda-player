# Isolating Plex authorization and Secret Service qualification

Researched 2026-10-26 for #16. Public documentation/source inspection only: no
account, token, credential, keyring, browser, playback or user-server operation
was performed. Strategies below are proposals, not measured qualification.

## Plex PIN authorization: official versus SDK evidence

Plex's published legacy flow creates a client identifier, requests a PIN, sends
the user to Plex's authorization application, then polls for an access token.
Its browser path uses `strong=true`; the short PIN path uses the link page.
The reviewed official description does not document an existing-token
`pins/link` endpoint or guarantee child-token isolation. JWT migration is a
different operation: the guide warns that migrating an existing legacy token
expires that token. Do not use JWK registration/migration for this experiment.
[Official authentication guide](https://developer.plex.tv/pms/#section/API-Info/Authenticating-with-Plex).

The maintained **Python PlexAPI** implementation does expose programmatic linking.
Pinned revision: `aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1`, inspected 2026-10-26.
This is primary evidence of that SDK's behavior, not a Plex service guarantee.

| Step | Exact SDK behavior |
| --- | --- |
| Create short PIN | `MyPlexPinLogin(oauth=False, headers=...)`: `POST https://plex.tv/api/v2/pins`, **no** `strong` parameter. Override `X-Plex-Client-Identifier`/`X-Plex-Product` with a fresh random identifier/distinct validation product. Response: `id`, `code`. |
| Authorize | `MyPlexAccount.link(pin)`: account `X-Plex-Token`, `PUT https://plex.tv/api/v2/pins/link`, form `code=<pin>`, form-urlencoded content type, product `Plex SSO`. Documented for four-character PINs; strong-PIN equivalence unestablished. |
| Poll | `GET https://plex.tv/api/v2/pins/{id}` with the PIN client's headers; no `code` query in this SDK. It reads `authToken`, with a one-second polling interval. |
| Locate candidate device | `GET https://plex.tv/devices.xml`; device fields include `id`, `clientIdentifier`, `product`, creation time and `token`. |
| Targeted removal | Device `.delete()` sends `DELETE https://plex.tv/devices/{id}.xml`. This is a device ID, not a PMS server identifier or client UUID. |

[PIN creation/polling](https://github.com/pkkid/python-plexapi/blob/aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1/plexapi/myplex.py#L1890-L1970),
[programmatic link](https://github.com/pkkid/python-plexapi/blob/aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1/plexapi/myplex.py#L1144-L1156),
[account headers](https://github.com/pkkid/python-plexapi/blob/aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1/plexapi/myplex.py#L242-L258),
[device fields/removal](https://github.com/pkkid/python-plexapi/blob/aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1/plexapi/myplex.py#L1636-L1710).

The polling response alone does not verify a new device/client association or
prove that its token differs from the authorizing token. A defensible experiment
would require, privately and in memory, all of the following before cleanup:

- Fresh client identifier absent from a before-device snapshot; exactly one new
  after-device record matches that identifier, expected product and new device ID.
- Polled token differs from the original token; if a device token is returned,
  verify its association rather than assuming it. Missing/ambiguous evidence
  means stop before any destructive cleanup request.
- The candidate is a newly created client, not the Plex Media Server, an existing
  desktop application, a shared account or a reused device identifier.
- Never log device-list bodies: they can include other devices' secrets. Retain
  only redacted booleans/counts and synthetic labels.

These checks reduce mis-targeting; they do not manufacture an official guarantee
of disjoint authorization domains. Resource-scoped PMS tokens are a separate
concept and may not have the same identity/lifecycle as the polled account token.

### Revocation scope and remaining unknowns

Official support describes removing **one selected authorized-device entry** and
signing that app back in to register it again. It does not specify API-level
revocation propagation, PMS cache/session expiry timing, or collateral behavior
for every token/device combination. No documented collateral effect on unrelated
devices was identified; that is not proof that an unqualified endpoint cannot
affect them. [Per-device removal](https://support.plex.tv/articles/115007577087-devices/).

**Forbid `MyPlexAccount.signout()` entirely in this experiment.** The SDK labels
it token-invalidating and implements `DELETE /api/v2/users/signout`; calling it
with the original account object would act on the authorizing token. Likewise
exclude all-device sign-out, password changes and JWT migration. Plex explicitly
distinguishes account changes/logout from intentionally signing out connected
devices. [SDK sign-out](https://github.com/pkkid/python-plexapi/blob/aa5bf6742eb806bbbebd2b65dee9fc47a81d92e1/plexapi/myplex.py#L154-L156),
[official token-lifecycle caveats](https://support.plex.tv/articles/reporting-security-issues/).

Recommended conclusion: programmatic short-PIN linking is a real maintained SDK
operation, not an invented endpoint, but it cannot qualify Qoda's **browser**
handoff or establish strong-PIN safety by itself. Prefer a fresh random-client
browser PIN approved by the human, then independently verify its device identity.
Any later targeted-removal test should use only that verified fresh device,
preserve the original sign-in, and record both removal and observed access results
without claiming universal token/session revocation. If disjoint ownership cannot
be established, retain the revocation gate rather than experiment on the original.

## Private GNOME Secret Service

GNOME Keyring **48.0** documents `--foreground`, `--components=secrets`, explicit
`--control-directory`, and `--unlock` (password on stdin; unlock or create login
keyring). `--login` is PAM's delayed-initialization mode; `--start` and `--replace`
can discover/initialize or replace another daemon. They are unnecessary for a
new isolated foreground instance.
[Upstream manual](https://gitlab.gnome.org/GNOME/gnome-keyring/-/blob/48.0/docs/gnome-keyring-daemon.xml#L60-136).

The pinned daemon's discovery function uses an explicit control directory and
does not fall through to inherited control paths when it is provided. Its secret
storage normally uses the XDG data directory's `keyrings`, but falls back to
`~/.gnome2/keyrings` if the new directory is absent and the old directory exists.
Changing only D-Bus or only XDG state is therefore insufficient isolation.
[Daemon discovery](https://gitlab.gnome.org/GNOME/gnome-keyring/-/blob/48.0/daemon/gkd-main.c#L562-L610),
[storage-path fallback](https://gitlab.gnome.org/GNOME/gnome-keyring/-/blob/48.0/pkcs11/gkm/gkm-util.c#L140-L176).

### Concrete synthetic qualification strategy

1. Create one owned mode-0700 scratch root, with private child home, data, config,
   state, cache, runtime and control directories. Pre-create its **data/keyrings**
   directory. Give child processes explicit absolute XDG paths and the private
   home; do not change the parent session's environment or actual user files.
   Runtime ownership/mode follows the [XDG specification](https://specifications.freedesktop.org/basedir/latest/).
2. Start an owned `dbus-daemon --nofork --config-file=<private XML>` on a Unix
   socket inside that root. Use an explicit session configuration with no
   standard service directories, included desktop configuration or systemd
   activation. `--session` alone selects the installed default configuration,
   which is broader than this test needs. Pass its explicit address only to
   fixture children. [D-Bus daemon configuration](https://dbus.freedesktop.org/doc/dbus-daemon.1.html).
3. Remove inherited daemon/control and D-Bus starter variables from the child
   environment; set its explicit private bus address. Leave DISPLAY, Wayland and
   session-ID integration unset so a prompt cannot borrow the active desktop.
   Start `gnome-keyring-daemon --foreground --components=secrets
   --control-directory=<private control> --unlock`; send a generated **synthetic**
   fixture password through a private stdin pipe and close it. All stdin bytes,
   including a newline, form the password. Track the owned process directly.
4. Before any Secret Service mutation, verify `org.freedesktop.secrets` ownership
   on that private bus belongs to the owned daemon. Verify its collection paths
   and default alias, not merely a friendly label. If needed, set the default
   alias only on this verified private service. Store/lookup/replace/delete a
   synthetic Qoda-shaped item through the actual vault/secret-tool boundary.
   Do not feed placeholder tokens into a live provider connection as part of
   these synthetic storage checks.
5. **Locked case:** call `org.freedesktop.Secret.Service.Lock` with only the known
   private collection path; verify `Collection.Locked=true` and application
   locked/error behavior. Lock/unlock may return a prompt; `/` means none. If a
   prompt is returned, cancel/dismiss it or keep the case unqualified rather than
   display it on the real desktop. A libsecret unlock call does not accept a raw
   password; do not invent such an API.
6. **Unlocked/restart case:** stop and join only the private daemon, restart it
   against the same scratch data with `--unlock` and the synthetic password, and
   verify actual lookup/persistence and unlocked state. This qualifies private
   restart/unlock, not an interactive desktop prompt or PAM login. Avoid assuming
   that a second daemon invocation silently unlocks an already running service.
7. **Unavailable case:** use the private bus without a Secret Service owner and
   without activation entries. Verify the bounded unavailable result. Separately
   an invalid private bus address can test connection failure; do not conflate
   those two conditions or remove a host service to simulate them.
8. Stop/join owned helpers, confirm their private bus names disappear, then
   remove only the owned scratch files. Never use `pkill`, desktop keyring locking,
   `--replace`, host default-collection deletion or broad attribute cleanup.

The standard defines collection/item locking, prompt objects, `/` sentinel and
`Locked` state; prompts require explicit client action and can be dismissed.
[Lock/unlock semantics](https://specifications.freedesktop.org/secret-service/latest/unlocking.html),
[service API and aliases](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Service.html),
[collection state](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Collection.html),
[prompt lifecycle](https://specifications.freedesktop.org/secret-service/latest/prompts.html),
[libsecret password/prompt boundary](https://gnome.pages.gitlab.gnome.org/libsecret/migrating-libgnome-keyring.html#locking-and-unlocking).

## Qualification boundary

Private-daemon synthetic tests exercise the real Secret Service provider and
application integration while leaving the normal desktop keyring out of scope.
They do **not** observe that desktop's current locked state, session-manager/PAM
behavior, user prompts, or a live Plex credential's expiry/revocation. A fresh
human-approved Plex device can later exercise browser/token integration inside
the isolated profile; that remains a separate explicitly bounded live run.
Preserve these evidence tiers in #16 rather than relabel private fixtures as
real default-keyring or browser acceptance.


## Implementation-owner qualification follow-up

`prototypes/owned-qualification/` retains the actual results separately. The
implemented keyring fixture does **not** set or repurpose HOME: a bwrap mount
namespace hides the real home/run directories, while explicit private XDG paths
and control directory are writable. The custom bus has no activation directories;
it must allow both send and receive replies. A first policy omitted receive
permission and failed before a daemon/credential operation; the corrected private
bus mechanism passed the retained matrix. Restarting the owned foreground daemon
with the same synthetic password actually unlocks it and restores the marker;
calling a second --unlock process against the live daemon did not in the older
retained experiment. This is no claim about desktop/PAM unlock.

The measured Plex devices.xml response omits token fields. A fresh browser
exchange acquired a distinct credential for the same account, but the first
harness stopped on its overly strict missing-token assertion. It did not remove
a device or establish revocation. Its uniquely reidentified fresh fixture was
subsequently removed in a separately retained cleanup; original device/cloud
access remained and no qualification devices remain.

A separate explicitly labeled SDK short-PIN control verified the creation and
polled PIN ID/client binding, distinct token, same account and exact newly added
non-server device. The device-list omission does not prevent that PIN-to-client-
to-device identity check. Targeted removal returned 200 and removed that exact
entry; child cloud access returned 401, but the owned PMS still accepted the child
token with 200 throughout a 30.8-second window. Parent cloud/PMS access remained
200. No immediate PMS revocation guarantee follows. The short control does not
replace the human browser observation, and neither control measures account expiry.
