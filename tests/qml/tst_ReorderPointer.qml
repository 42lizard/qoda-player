import QtQuick
import QtQuick.Window
import QtQuick.Controls
import QtTest
import "../../ui" as Qoda

TestCase {
    id: test
    name: "ReorderPointer"
    width: 420; height: 400; visible: true; when: windowShown
    property bool pointerHeld: false
    Qoda.ReorderState {
        id: reorder
        kind: "queue"; account: "4"; instance: "instance-1"
        fence: ({queue_session: "session", queue_revision: "9007199254740993"})
        rows: []; enabled: true
    }
    Drawer {
        id: popup
        width: 400; height: 380; edge: Qt.RightEdge; modal: true; focus: true
        closePolicy: reorder.active ? Popup.NoAutoClose : Popup.CloseOnEscape | Popup.CloseOnPressOutside
    }
    SignalSpy { id: requests; target: reorder; signalName: "requested" }
    ListView {
        id: view
        width: 400; height: 280; clip: true; cacheBuffer: 0
        highlightFollowsCurrentItem: false
        highlightRangeMode: ListView.NoHighlightRange
        model: reorder.rows
        delegate: Rectangle {
            required property var modelData
            required property int index
            width: view.width; height: 64
            color: index % 2 ? "#303f32" : "#191f1c"
            Qoda.ReorderHandle {
                objectName: "handle"
                x: 10; y: 16
                width: 26; height: 26
                controller: reorder; itemId: modelData.id; viewport: view; movable: modelData.can_move
            }
            Text { x: 50; y: 20; text: modelData.id; color: "white" }
            Qoda.ReorderDrop { anchors.fill: parent; controller: reorder; targetId: modelData.id; movable: modelData.can_move }
        }
    }
    Qoda.ReorderScroll { id: edge; controller: reorder; viewport: view }
    function init() {
        reorder.cancel(); reorder.enabled = true;
        reorder.fence = {queue_session: "session", queue_revision: "9007199254740993"};
        reorder.rows = Array.from({length: 5}, (_, index) => ({id: "item-" + index, can_move: index > 0}));
        view.contentY = 0; view.currentIndex = -1;
        requests.clear(); waitForPolish(view); view.forceLayout();
    }
    function cleanup() {
        reorder.cancel();
        if (pointerHeld) mouseRelease(view.Window.window.contentItem, 1, 1, Qt.LeftButton, Qt.NoModifier, 0);
        pointerHeld = false;
        popup.close(); tryCompare(popup, "visible", false); view.parent = test;
        wait(1);
    }
    function release(surface, x, y, delay) {
        mouseRelease(surface, x, y, Qt.LeftButton, Qt.NoModifier, delay);
        pointerHeld = false;
    }
    function press(index) {
        const source = view.itemAtIndex(index);
        verify(!!source);
        const handle = findChild(source, "handle");
        const surface = view.Window.window.contentItem;
        const point = handle.mapToItem(surface, 13, 13);
        pointerHeld = true;
        mousePress(surface, point.x, point.y, Qt.LeftButton, Qt.NoModifier, 5);
        mouseMove(surface, point.x + 12, point.y, 5, Qt.LeftButton);
        mouseMove(surface, point.x + 26, point.y + 2, 5, Qt.LeftButton);
        wait(10);
        return surface;
    }
    function test_pointer_drop_emits_once_after_delivery_and_keeps_model_unchanged() {
        const before = JSON.stringify(reorder.rows);
        const surface = press(1); verify(reorder.active); verify(reorder.pointerActive);
        const target = view.itemAtIndex(2);
        const point = target.mapToItem(surface, 180, 48);
        mouseMove(surface, point.x, point.y, 5, Qt.LeftButton); wait(10);
        compare(requests.count, 0);
        release(surface, point.x, point.y, 5);
        compare(requests.count, 1);
        compare(requests.signalArguments[0][1].occurrence, "item-1"); compare(requests.signalArguments[0][1].before, "item-3");
        compare(requests.signalArguments[0][1].queue_revision, "9007199254740993");
        compare(JSON.stringify(reorder.rows), before); verify(!reorder.active);
    }
    function test_escape_and_revision_change_cancel_before_mouse_release() {
        let surface = press(1); verify(reorder.active);
        keyClick(Qt.Key_Escape); verify(!reorder.active);
        const target = view.itemAtIndex(2); const point = target.mapToItem(surface, 180, 48);
        mouseMove(surface, point.x, point.y, 5, Qt.LeftButton);
        release(surface, point.x, point.y, 5);
        compare(requests.count, 0);
        surface = press(1); verify(reorder.active);
        reorder.fence = {queue_session: "session", queue_revision: "9007199254740994"}; verify(!reorder.active);
        release(surface, point.x, point.y, 5); compare(requests.count, 0);
    }
    function test_disabled_surface_and_viewport_resize_cancel_without_request() {
        let surface = press(1); verify(reorder.active);
        view.width = 390; verify(!reorder.active);
        release(surface, 180, 170, 5); view.width = 400;
        surface = press(1); verify(reorder.active);
        reorder.enabled = false; verify(!reorder.active);
        release(surface, 180, 170, 5);
        compare(requests.count, 0);
    }
    function test_modal_drawer_drag_uses_overlay_and_escape_cancels_before_dismissal() {
        view.parent = popup.contentItem;
        popup.open(); tryCompare(popup, "opened", true);
        waitForPolish(view); view.forceLayout();
        const surface = press(1); verify(reorder.active); verify(reorder.pointerActive);
        verify(reorder.visual.parent === view.Overlay.overlay);
        keyClick(Qt.Key_Escape); verify(!reorder.active); verify(popup.opened);
        const point = view.mapToItem(surface, 180, 170);
        release(surface, point.x, point.y, 5); compare(requests.count, 0);
        keyClick(Qt.Key_Escape); tryCompare(popup, "opened", false);
        verify(test.visible);
    }
    function test_modal_drawer_accepts_one_drop_without_dismissing_the_surface() {
        view.parent = popup.contentItem;
        popup.open(); tryCompare(popup, "opened", true);
        waitForPolish(view); view.forceLayout();
        const surface = press(1); verify(reorder.active); verify(reorder.pointerActive);
        const point = view.itemAtIndex(2).mapToItem(surface, 180, 48);
        mouseMove(surface, point.x, point.y, 5, Qt.LeftButton); wait(10);
        release(surface, point.x, point.y, 5);
        compare(requests.count, 1); compare(requests.signalArguments[0][1].before, "item-3");
        verify(popup.opened); verify(test.visible);
    }
    function test_stationary_edge_scroll_retains_source_and_uses_current_drop_target() {
        reorder.rows = Array.from({length: 50}, (_, index) => ({id: "item-" + index, can_move: index > 0}));
        waitForPolish(view); view.forceLayout();
        const surface = press(1); verify(reorder.active);
        const point = view.mapToItem(surface, 180, view.height - 8);
        mouseMove(surface, point.x, point.y, 5, Qt.LeftButton); wait(10);
        const ghost = reorder.visual;
        const initial = ghost.mapToItem(surface, 15, 15);
        tryVerify(() => view.contentY >= view.height * 3 || !reorder.active, 4000);
        verify(reorder.active, "Source delegate disappeared during scrolling");
        edge.stop();
        compare(view.currentItem.modelData.id, "item-1");
        const current = ghost.mapToItem(surface, 15, 15);
        verify(Math.abs(current.x - initial.x) < 1 && Math.abs(current.y - initial.y) < 1, "Ghost moved with scrolling content");
        const contentY = view.contentY + view.height - 8;
        const index = view.indexAt(180, contentY);
        verify(index > 5);
        const after = contentY - index * 64 >= 32;
        const expected = reorder.rows[index + (after ? 1 : 0)].id;
        compare(requests.count, 0);
        release(surface, point.x, point.y, 0);
        compare(requests.count, 1); compare(requests.signalArguments[0][1].before, expected);
        compare(requests.signalArguments[0][1].occurrence, "item-1");
    }
}
