# Internal QML drag reordering

Research for #35, 2026-10-26. Public documentation currently identifies itself as
Qt **6.12.0**, matching the installed major/minor reported by the parent workflow.
Source checks use Qt's `v6.12.0` tag, resolved to
`4332fbbb4c340463dc8cee36dda0c85d932122d8`. No production code, tests, desktop input,
or native drag session was changed or exercised for this note.

## Supported event pattern and release order

Qt documents `MouseArea.drag.target` moving a visual item with
`Drag.active: mouseArea.drag.active`; `Drag.Internal` is the default internal
scene path. Position changes emit drag moves. `Drag.hotSpot` changes also request
a move. `Drag.drop()` ends the sequence with a drop and returns the accepted
action; setting active false or calling `cancel()` ends it without a drop.
[Drag documentation](https://doc.qt.io/qt-6/qml-qtquick-drag.html).

In the pinned implementation, `MouseArea.mouseReleaseEvent()` emits `released`
through `setPressed(false)` **before** clearing `MouseArea.drag.active`. Therefore
call `ghost.Drag.drop()` from `onReleased` while drag-active is still true. Do not
bind the drag lifetime solely to `MouseArea.pressed`: that becomes false before
the release handler. A stolen grab clears drag-active and emits `canceled`.
[MouseArea release](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickmousearea.cpp#L756-L780),
[pressed/released ordering](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickmousearea.cpp#L1230-L1291),
[grab cancellation](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickmousearea.cpp#L890-L912).

`drop()` first flushes a queued move. It then synchronously delivers the drop.
Calling `drop()`/`cancel()` or changing active from a drag-event handler is guarded
against with warnings. Thus a DropArea should only validate/store its accepted
candidate and accept MoveAction; emit the backend mutation after `drop()` returns
to the release handler, using copied payload. This avoids reentrant model changes
cancelling a drag inside Qt's event delivery.
[Pinned drop/cancel implementation](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickdrag.cpp#L668-L741).

## Ghost placement and stationary-pointer scrolling

Qt's view tutorial moves a delegate's **content**, leaving its view-positioned
root in place, removes anchors and reparents the content above the view. It does
not drag the ListView-managed root. `Window.contentItem` is the scene's root item.
[Dragging view items](https://doc.qt.io/qt-6/qtquick-tutorials-dynamicview-dynamicview2-example.html),
[Window contentItem](https://doc.qt.io/qt-6/qml-qtquick-window.html#contentItem-prop).

For Qoda, use a ghost owned by a stable controller and visually parented to the
actual handle's `Window.contentItem`. Map its initial position into that item;
remove position anchors before moving it. Keep the source delegate's layout slot.
This separates ghost coordinates from `ListView.contentY`, so edge scrolling does
not move the drag point with the list. Persist payload in the controller, not in
a possibly recycled delegate; cancel safely if the input source disappears.
These are architectural recommendations derived from the tutorial, not a
promise that Qt retains every offscreen delegate during a drag.

Use the independent ghost as `Drag.source` or a plain QtObject payload. Do not
use an ancestor Item containing the targets: DropArea explicitly rejects such a
source to avoid dragging its own target subtree.
[Ancestor-source refusal](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickdroparea.cpp#L224-L240).

### Keeping the source gesture alive during long scrolling

Do not rely on a mouse grab to retain a non-current delegate: the reviewed
ListView offscreen-release path has no grab exemption. Public docs warn that
delegates may disappear, but explicitly exclude `currentItem` from pooling/reuse.
[Delegate lifecycle](https://doc.qt.io/qt-6/qml-qtquick-listview.html#reusing-items),
[offscreen release path](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquicklistview.cpp#L824-L866).

For a stable model, temporarily setting `currentIndex` to the source provides a
separate model-object reference; releasing its visible-list reference therefore
keeps the underlying delegate. Offscreen `currentItem` may be render-culled, which
does not destroy it. Existing pointer grabbers receive updates without a fresh
spatial hit test.
[Current-item reference](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickitemview.cpp#L1829-L1853),
[referenced-object retention](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/qmlmodels/qqmldelegatemodel.cpp#L615-L635),
[render culling](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickitem.cpp#L7101-L7110),
[grabber delivery](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/util/qquickdeliveryagent.cpp#L2353-L2397).

Recommended pin: first disable highlight following and use
`highlightRangeMode: ListView.NoHighlightRange`, then set currentIndex to the source
without moving backend order. Otherwise highlight/range tracking can scroll or
change the current item. Prevent keyboard current-index changes while dragging;
restore the previous UI current index/focus after release processing, preferably
deferred until the source release handler returns.
[Current index/highlight contract](https://doc.qt.io/qt-6/qml-qtquick-listview.html#currentIndex-prop),
[conditional viewport tracking](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickitemview.cpp#L1415-L1430).

This protects ordinary offscreen virtualization, not model reset/replacement,
source removal, delegate replacement or view destruction. Keep identity-fence
cancellation and external payload storage. Add a long stationary-pointer drag
regression crossing several viewports beyond cacheBuffer: assert the same source
currentItem survives, its original MouseArea stays grabbed, and release commits
once. Do not claim that highlightFollowsCurrentItem alone guarantees lifetime.

Do not assume stationary-pointer scrolling refreshes DropArea selection:
DropArea signals reflect drag events and its stored coordinates are those of the
last event. [DropArea contract](https://doc.qt.io/qt-6/qml-qtquick-droparea.html).
The pinned source schedules moves for the dragged item's geometry/parent and
hotspot changes. `setHotSpot` ignores equal values; a changed value queues a
coalesced event that reads the current hotspot when delivered. No extra native
pointer event is required.
[Pinned source scheduling](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickdrag.cpp#L65-L143),
[hotspot setter](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickdrag.cpp#L346-L357).

On each received move, Qt's delivery agent re-hit-tests current scene geometry,
updates targets, and sends leave/move/enter events accordingly; merely changing
underlying geometry does not execute this routine.
[Drag delivery and hit testing](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/util/qquickdeliveryagent.cpp#L2684-L2869).

A practical refresh after contentY changes is to save `Drag.hotSpot`, assign a
slightly different point, then restore the saved point, outside DropArea handlers.
In the pinned implementation this queues a move at the unchanged final scene
point; `drop()` also flushes it before release. This workaround is an inference
from source scheduling, **not** a separate public refresh API. Require a test
where only the list scrolls under an unmoving ghost, including immediate release.
Never mutate backend order from `onEntered`; transient refresh events are preview.

## Proposed bounded controller

Set `preventStealing: true` on the dedicated handle before press; Flickable can
otherwise steal it, and changing this after theft only affects the next press.
Leave ordinary row/list flicking outside the handle.
[MouseArea grab policy](https://doc.qt.io/qt-6/qml-qtquick-mousearea.html#preventStealing-prop).
Use a small drag handle and explicit internal MoveAction/key matching, without
`Drag.Automatic`, `startDrag()`, file URLs or OS MIME payloads. Snapshot the source
occurrence/item, container identity, queue/playlist revision, source/account fence
and permitted-move status at drag start. Preview a before-item ID or append slot;
resolve no authoritative order from a mutable delegate index. Commit once on
accepted release; stale backend fences remain authoritative. Retain keyboard
move controls, pinned-row rules and playlist read-only/uncertain-mutation gates.

Use one repeating timer only while dragging near the list's top/bottom edge.
A fixed 30–50 ms interval and bounded pixel step are Qoda policy, not Qt's API
requirements. Clamp contentY to `originY..max(originY, originY+contentHeight-height)`;
stop when inactive/hidden, outside the edge, at a limit, on source destruction or
fence change. Refresh hit targets only after actual scroll changes. Timer timing
is animation-clock based (usually no better than 16 ms), so tests should await
state rather than count exact ticks. [Timer contract](https://doc.qt.io/qt-6/qml-qtqml-timer.html).

Wire Escape, `MouseArea.onCanceled`, window/surface loss and identity changes to
one cancellation path: stop timer, cancel internal Drag, clear candidate and
controller-active flag, and emit no mutation. Consume Escape only while dragging;
otherwise preserve drawer/player Escape behavior. Restore prior focus if it
still exists. Internal Drag has no documented automatic Escape handler.

## Offscreen verification and version limits

Qt Quick Test provides `mouseDrag`, `mousePress`, `mouseMove`, `mouseRelease`,
`keyClick`, `waitForPolish` and asynchronous comparisons. Mouse/key injection
requires `when: windowShown`; item coordinates are mapped into the item's window.
For edge-scroll tests, map the press point to `Window.contentItem` and use that
stable item for later moves/releases, with LeftButton held. A scrolling delegate
would change the synthetic scene coordinates on each call. TestCase's `wait()`
processes events, unlike a blocking sleep.
[Qt Quick Test methods](https://doc.qt.io/qt-6/qml-qttest-testcase.html).

`mouseDrag()` performs press, threshold/intermediate moves and release. Use the
individual methods to hold the pointer still while the scroll timer runs or to
inject Escape before release.
[Pinned QtTest drag helper](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/qmltest/TestCase.qml#L1590-L1630),
[window-event delivery](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/qmltest/quicktestevent.cpp#L119-L200).

Recommended regressions: accepted upward/downward/append drop emits exactly one
fenced mutation; ordinary click emits none; Escape then release emits none;
stationary-pointer edge scroll changes candidate while ghost scene position stays
fixed; release immediately after scroll uses the refreshed target; stale revision,
source removal, hidden surface, pinned rows and read-only playlists cancel/refuse.
Run with Qt 6 offscreen/software as the repository already does. Those tests
qualify internal event logic, not physical keyboard/pointer or native layer-shell
focus. The hotspot refresh/release ordering needs rerunning when Qt changes.
The 6.12 docs' read-only label on attached Drag.active conflicts with its binding
example; the pinned header explicitly exposes WRITE setActive. MouseArea.drag.active
itself is read-only. [Attached property declaration](https://github.com/qt/qtdeclarative/blob/4332fbbb4c340463dc8cee36dda0c85d932122d8/src/quick/items/qquickdrag_p.h#L214-L231).
