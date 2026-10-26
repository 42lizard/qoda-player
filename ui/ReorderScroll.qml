import QtQuick

Timer {
    required property var controller
    required property var viewport
    interval: 30; repeat: true
    running: !!controller && controller.active && controller.pointerActive && !!viewport
    onTriggered: {
        if (controller.pointerX < 0 || controller.pointerX > viewport.width) return;
        const y = controller.pointerY;
        let delta = y >= -24 && y < 32 ? -12 : y > viewport.height - 32 && y <= viewport.height + 24 ? 12 : 0;
        if (!delta) return;
        const origin = viewport.originY || 0;
        const next = Math.max(origin, Math.min(Math.max(origin, origin + viewport.contentHeight - viewport.height), viewport.contentY + delta));
        if (next === viewport.contentY) return;
        viewport.contentY = next;
        // Qt re-hit-tests row geometry on DragMove. Queue a coalesced move at
        // the same hotspot so scrolling alone cannot retain an obsolete target.
        if (controller.visual) controller.visual.refreshDrag();
    }
}
