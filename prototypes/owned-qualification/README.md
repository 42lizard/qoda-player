# Remaining owned-server qualification (#16)

Validation branch only; do not merge instrumentation into production. Existing
owned-browser-PIN authorization stays intact. Fresh-client qualification uses a
separate random client identifier and a freshly approved strong browser PIN.
Credentials and device/resource responses stay in memory and are never logged.
No music, playlist, artwork, played-state or timeline mutations are part of the
fresh-auth lifecycle test.

Before a selected validation-device removal, require: client identifier absent
from the before list; one new numeric device ID with that exact client/product;
a polled token distinct from the original; the creation and polled PIN's client identifier/ID bind that token to the new
client and exact device; if the device list returns a token it must match. The
measured device list omits tokens, so token equality there is not a prerequisite; the same account identity; and a client rather than PMS device.
Only DELETE /devices/{that-new-id}.xml is allowed. No account signout, JWT
migration, all-device removal or changes to the original keyring item. Observe
child cloud/PMS denial separately from device disappearance and parent access;
do not assume immediate PMS revocation or universal expiry behavior.

Private Secret Service tests use a separate bus, writable scratch XDG paths and
an owned daemon. Precreate its keyrings directory, use a read-only host filesystem
namespace, remove display/session activation access, and never change HOME or
operate on the desktop daemon/collections. Use synthetic markers only.

Controlled stream interruption requires a separately named synthetic fault in the
validation transport, at most 60 seconds of null-output rendering, a pinned owned
track version and explicit fences. Do not turn fresh Range connections into a
recovery pass or repeat prior playlist writes merely to relabel retained evidence.

A separately labeled short-PIN SDK control may use the already authorized owned
credential to link a new disposable PIN. It does not replace the measured human
browser exchange or qualify browser UX. The first fresh browser attempt was
retained as a stopped test because its harness incorrectly required a token field
that devices.xml omits; no deletion occurred in that attempt. Do not convert that
stopped observation into a revocation pass. Neither device disappearance nor a
successful delete alone proves server-side token denial.
