import QtQuick

QtObject {
    id: state
    required property string kind
    required property string account
    required property string instance
    required property var fence
    required property var rows
    required property bool enabled
    readonly property string scope: "qoda-reorder-" + kind
    readonly property string signature: JSON.stringify({kind: kind, account: account, instance: instance, fence: fence, rows: (rows || []).map(row => [row.id, kind !== "queue" || !!row.can_move])})
    property double generation: 0
    property bool active: false
    property string itemId: ""
    property string frozenSignature: ""
    property var frozenFence: null
    property var candidate: null
    property bool pointerActive: false
    property real pointerX: -1000
    property real pointerY: -1000
    property var visual: null
    signal requested(string op, var fields)
    onSignatureChanged: if (active && signature !== frozenSignature) cancel()
    onEnabledChanged: if (!enabled) cancel()
    function movable(row) { return !!row && (kind !== "queue" || !!row.can_move); }
    function cancel(owner, epoch) {
        if ((owner !== undefined && owner !== itemId) || (epoch !== undefined && epoch !== generation)) return;
        active = false; itemId = ""; frozenFence = null; candidate = null;
        pointerActive = false; visual = null; pointerX = -1000; pointerY = -1000;
    }
    function begin(id) {
        cancel();
        const items = rows || [];
        const ids = items.map(row => row.id);
        if (!enabled || !instance || generation >= 9007199254740991 || !["queue", "playlist"].includes(kind) || !id || new Set(ids).size !== ids.length || !items.some(row => row.id === id && movable(row))) return false;
        if (!fence || (kind === "queue" ? typeof fence.queue_session !== "string" || typeof fence.queue_revision !== "string" : !fence.reference || typeof fence.expected_revision !== "string")) return false;
        ++generation;
        itemId = id; frozenSignature = signature;
        frozenFence = JSON.parse(JSON.stringify(fence));
        active = true;
        return true;
    }
    function current() { return active && enabled && signature === frozenSignature; }
    // DropArea only proposes. Finish runs after Drag.drop() returns, outside
    // Qt's drag-event delivery; no drag state or backend changes inside onDropped.
    function propose(targetId, after) {
        if (!current() || candidate || targetId === itemId) return false;
        const ids = (rows || []).filter(row => movable(row)).map(row => row.id);
        const original = JSON.stringify(ids);
        const remaining = ids.filter(id => id !== itemId);
        let at = targetId === null ? remaining.length : remaining.indexOf(targetId);
        if (at < 0) return false;
        if (targetId !== null && after) ++at;
        remaining.splice(at, 0, itemId);
        if (JSON.stringify(remaining) === original) return false;
        const fields = JSON.parse(JSON.stringify(frozenFence)); fields.account_epoch = account; fields.instance = instance;
        if (kind === "queue") {
            fields.occurrence = itemId; fields.before = remaining[at + 1] || null;
            candidate = {op: "queue_move", fields: fields};
        } else {
            fields.edit = {action: "move", item_id: itemId, after: remaining[at - 1] || null};
            candidate = {op: "playlist_edit", fields: fields};
        }
        return true;
    }
    function finish(accepted, owner, epoch) {
        if ((owner !== undefined && owner !== itemId) || (epoch !== undefined && epoch !== generation)) return false;
        const request = accepted && current() ? candidate : null;
        cancel(owner, epoch);
        if (!request) return false;
        requested(request.op, request.fields);
        return true;
    }
}
