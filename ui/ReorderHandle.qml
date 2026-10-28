import QtQuick
import QtQuick.Controls
import QtQuick.Window

Item {
    id: handle
    required property var controller
    required property string itemId
    required property var viewport
    property bool movable: true
    property string pressedId: ""
    property double pressedGeneration: -1
    property int previousIndex: -1
    property int pinnedIndex: -1
    readonly property var windowOverlay: viewport ? viewport.Overlay.overlay : null
    implicitWidth: 26; implicitHeight: 26
    enabled: movable && !!controller && controller.enabled
    Accessible.name: I18n.t("Drag to reorder; move buttons are also available")
    function unpin() {
        const view = viewport, previous = previousIndex, pinned = pinnedIndex, state = controller, epoch = pressedGeneration;
        Qt.callLater(() => {
            if (view && state && state.generation === epoch && !state.active && view.currentIndex === pinned)
                view.currentIndex = Math.min(previous, view.count - 1);
        });
    }
    function abortGesture() {
        if (ghost.Drag.active) ghost.Drag.cancel();
        if (controller) controller.cancel(pressedId, pressedGeneration);
        pressedId = ""; unpin();
    }
    QtObject {
        id: payload
        readonly property var controller: handle.controller
        readonly property string itemId: handle.pressedId
        readonly property double generation: handle.pressedGeneration
    }
    Rectangle {
        anchors.fill: parent; radius: 4
        color: mouse.containsMouse ? Theme.selected : Theme.button
        opacity: handle.enabled ? 1 : 0.35
        Text { anchors.centerIn: parent; text: "≡"; color: Theme.muted; font.pixelSize: 20 }
    }
    Rectangle {
        id: ghost
        parent: handle.windowOverlay && handle.windowOverlay.visible ? handle.windowOverlay : handle.viewport && handle.viewport.Window.window ? handle.viewport.Window.window.contentItem : handle
        width: 30; height: 30; radius: 4; z: 10000
        color: Theme.selected; border.color: Theme.accent
        visible: mouse.drag.active && !!handle.controller && handle.controller.active && handle.pressedId === handle.controller.itemId && handle.pressedGeneration === handle.controller.generation
        Text { anchors.centerIn: parent; text: "≡"; color: Theme.foreground; font.pixelSize: 20 }
        Drag.active: mouse.drag.active && !!handle.controller && handle.controller.active && handle.pressedId === handle.controller.itemId && handle.pressedGeneration === handle.controller.generation
        Drag.dragType: Drag.Internal
        Drag.source: payload
        Drag.keys: handle.controller ? [handle.controller.scope] : []
        Drag.supportedActions: Qt.MoveAction
        Drag.proposedAction: Qt.MoveAction
        Drag.hotSpot: Qt.point(width / 2, height / 2)
        function refreshDrag() {
            if (!Drag.active) return;
            const point = Drag.hotSpot;
            Drag.hotSpot = Qt.point(point.x + 0.01, point.y);
            Drag.hotSpot = point;
        }
    }
    MouseArea {
        id: mouse
        objectName: "reorderPointer"
        anchors.fill: parent
        hoverEnabled: true; preventStealing: true
        acceptedButtons: Qt.LeftButton
        cursorShape: drag.active ? Qt.ClosedHandCursor : Qt.OpenHandCursor
        drag.target: ghost
        onPressed: event => {
            if (!handle.controller.begin(handle.itemId)) { event.accepted = false; return; }
            handle.pressedId = handle.itemId; handle.pressedGeneration = handle.controller.generation;
            handle.previousIndex = handle.viewport.currentIndex;
            handle.pinnedIndex = handle.controller.rows.findIndex(row => row.id === handle.itemId);
            handle.viewport.currentIndex = handle.pinnedIndex;
            mouse.forceActiveFocus();
            const origin = handle.mapToItem(ghost.parent, handle.width / 2, handle.height / 2);
            ghost.x = origin.x - ghost.width / 2; ghost.y = origin.y - ghost.height / 2;
            handle.controller.visual = ghost;
            const pointer = mouse.mapToItem(handle.viewport, event.x, event.y);
            handle.controller.pointerX = pointer.x; handle.controller.pointerY = pointer.y;
        }
        onPositionChanged: event => {
            if (!handle.controller || !handle.controller.active || (handle.pressedId !== handle.controller.itemId || handle.pressedGeneration !== handle.controller.generation)) return;
            const point = mouse.mapToItem(handle.viewport, event.x, event.y);
            handle.controller.pointerX = point.x; handle.controller.pointerY = point.y;
        }
        onReleased: {
            const accepted = ghost.Drag.active && ghost.Drag.drop() === Qt.MoveAction;
            if (handle.controller) handle.controller.finish(accepted, handle.pressedId, handle.pressedGeneration);
            handle.pressedId = ""; handle.unpin();
        }
        onCanceled: handle.abortGesture()
        Keys.onShortcutOverride: event => { event.accepted = !!handle.controller && handle.controller.active; }
        Keys.onPressed: event => {
            if (!handle.controller || !handle.controller.active) { event.accepted = false; return; }
            if (event.key === Qt.Key_Escape) handle.abortGesture();
            event.accepted = true;
        }
    }
    Component.onDestruction: if (controller) controller.cancel(pressedId, pressedGeneration)
    Connections {
        target: mouse.drag
        function onActiveChanged() {
            if (handle.controller && handle.pressedId === handle.controller.itemId && handle.pressedGeneration === handle.controller.generation)
                handle.controller.pointerActive = mouse.drag.active && handle.controller.active;
        }
    }
    Connections {
        target: handle.viewport
        function onWidthChanged() { if (handle.controller && handle.pressedId) handle.controller.cancel(handle.pressedId, handle.pressedGeneration); }
        function onHeightChanged() { if (handle.controller && handle.pressedId) handle.controller.cancel(handle.pressedId, handle.pressedGeneration); }
    }
    ToolTip.visible: mouse.containsMouse && !mouse.pressed
    ToolTip.text: I18n.t("Drag to reorder")
}
