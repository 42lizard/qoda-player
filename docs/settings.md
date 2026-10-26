# Settings and focus

Account, audio and cache controls share a right-edge modal drawer with a fixed
heading/category row and a vertically scrollable body. They do not reduce browser
or transport space. The existing settings controls stay instantiated, preserving
local drafts across category and compact/full switches. Opening settings cancels
any reorder gesture. Opening queue/navigation or a browser explicitly closes
settings; background snapshot updates never open a modal.

The library rail exposes Connect Plex when disconnected. Menu also provides all
three settings categories. Audio/device/cache problems show a Review settings
notice in the full player. Account sign-in links remain in Account settings.

Ctrl+K, compact Search and the Search IPC command select cross-library Search,
close other drawers/menu, and focus the field after dismissal. New navigation or
settings requests cancel a pending focus request. Escape closes settings or the
narrow queue drawer; during a drag it cancels the gesture first. It does not hide
the full player. Expanding compact mode restores the previous settings category;
using compact Search opens Search instead.

Qt Quick tests cover scrolling and Escape/draft retention. Production offscreen
fixtures cover wide/narrow settings, content bounds, browser space and deferred
Search focus. Isolated native surface checks cover compact/full category
retention and Search focus after dismissal. Physical keyboard and multiple-output
qualification remain tracked in issue #18.

Older installations may have a `0755` Qoda configuration directory created by
the source-identity saver. Audio/output settings safely tighten an owned directory
to `0700` before loading, provided it is not writable by others. The operation
uses the opened directory descriptor and preserves existing files. Symlinked,
foreign-owned or group/world-writable directories remain unavailable; cache
directory validation stays strict. This migration prevents valid legacy installs
from silently disabling both output selection and ReplayGain.
