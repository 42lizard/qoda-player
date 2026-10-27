import QtQuick

// Lossless protocol acceptance and command envelopes shared by both UI clients.
QtObject {
    property var snapshot: ({})
    property var allowedOperations: null
    property bool queueFenced: false
    property int serial: 0
    property string nonce: Date.now().toString(36) + "-" + Math.random().toString(36).slice(2)
    function counter(value) {
        return typeof value === "string" && /^(0|[1-9][0-9]*)$/.test(value) && (value.length < 20 || value.length === 20 && value <= "18446744073709551615");
    }
    function accept(value) {
        if (!value || value.protocol !== 1 || typeof value.instance !== "string" || !value.instance.length || value.instance.length > 128 || !counter(value.revision) || !counter(value.account_epoch)) return false;
        if (snapshot.instance === value.instance) {
            const previous = snapshot.revision;
            if (value.revision.length < previous.length || value.revision.length === previous.length && value.revision < previous) return true;
        }
        snapshot = value;
        return true;
    }
    function reset() { snapshot = ({}); }
    function request(op, fields, requiresQueue) {
        if (!snapshot.instance || typeof op !== "string" || !op.length || allowedOperations && !allowedOperations.includes(op)) return null;
        let data = Object.assign({}, fields || {});
        if (queueFenced || requiresQueue) {
            const queue = snapshot.queue;
            if (!queue || typeof queue.session !== "string" || !queue.session.length || !counter(queue.revision)) return null;
            data.queue_session = queue.session; data.queue_revision = queue.revision;
        }
        data.op = op; data.instance = snapshot.instance; data.account_epoch = snapshot.account_epoch;
        data.id = nonce + "-" + (++serial);
        return data;
    }
}
