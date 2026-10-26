# Offline catalog startup

Qoda can start with the last saved account's library discovery and browse cached
tracks, albums, album details and artists without restoring credentials or
contacting Plex. Cached cross-library search covers at most the stored tracks,
with the existing result/source limits. The queue and nominal position restore
paused; opening the application never starts music.

Cached library choices are labelled “cached”. The banner and individual page or
search states identify offline, stale, unverified and incomplete metadata. A
missing page is a cache miss, not evidence that a library contains no music.
Expired pages may remain readable; their timestamps are not refreshed by offline
reads. Playlists still require a live connection.

## Authorization and reconnect

Cached discovery is a display allowlist, not a media provider. Its read path has
no provider/network fallback or media preparation API. Cached-source queue/play,
playlist and edit actions remain unavailable, including in an account with other
reachable libraries. Real playback still uses the current live provider and its
source/account authorization. Cached metadata never creates a media grant.

Cold startup does not read the keyring. Connect / restore sign-in opens Account
settings for an explicit action. An existing session credential can be retried
with Reconnect Plex. Reconnect preserves the queue and position, pauses playback,
revokes media grants, cancels speculative output/recovery and fences late
account/catalog/album/search results. Resume remains explicit.

After verified account/resource discovery, cached sources from the same account
are retained only for still-entitled servers whose discovery was unreachable.
A missing resource, successful section refresh omitting a library, or explicit
section authorization denial does not retain the old source. Sources from another
account are discarded even if a source UUID were reused. A partial connection
labels the live/cached library counts and unavailable-server count; selecting a
cached source stays read-only. A retained queue entry on a known unreachable
source fails as a network outage, rather than being marked removed. This is a conservative local implementation;
shared/restricted live-server behavior still needs #16 qualification.

## Storage and cleanup

Discovery contains a version, noncredential SHA-256 account partition, bounded
library IDs/display names, and a discovery timestamp. It shares the private
`catalog-v1/catalog.sqlite3` cache and its budget, cleanup marker, worker epoch,
source invalidation and file-identity guards. It contains no credentials, server
URLs, authenticated requests or playable grants. Limits: 128 unique libraries,
1024-byte display fields, 256 KiB encoded discovery. Offline reads recheck the
active persisted partition rather than trusting an old in-memory allowlist.

SQLite cache schema 1 migrates transactionally to schema 2 with an empty discovery
table. It does not invent an account for old pages; the first verified discovery
replaces that unpartitioned cache. One successful connection after upgrading is
therefore needed before offline discovery becomes available. Future database or
discovery versions, malformed/oversized discovery and invalid/aliased saved source
identity maps are preserved and not used as guessed identity. A partition replacement records durable purge intent before writing. An
interrupted/failed replacement clears the prior partition before offline restart
and disables current cache access until cleanup; live provider browsing can still
work. Stable source-ID mappings remain separate from cached catalog metadata.

Sign-out/disconnect removes discovery with the other account cache data and
invalidates pending work. Clearing metadata or setting its budget to zero removes
offline discovery immediately and on disk; restoring the budget does not revive
it. Reconnect saves a verified discovery again. Clearing artwork is independent
and preserves catalog discovery. No permanent desktop setup or automatic sign-in
is installed.

## Verification

`./tools/check.sh` covers cold/expired discovery, missing pages, partition
replacement and late writes, same-account source pruning, revocation, clear and
budget-zero restart, schema-1 migration, malformed/newer/oversized identities,
partial-server reconciliation, source entitlements and reconnect fences. QML
checks cover explicit connection actions and cached source labels.

After building the backend, `python3 tools/check-offline.py` uses a private
synthetic cache/session and the production native UI to verify cached browse and
search, disabled actions, queue/position restoration, independent artwork cleanup,
metadata clearing across restart and preservation of newer discovery. No keyring,
Plex, media preparation or audio is used; owned processes are stopped.
