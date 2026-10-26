import QtQuick

QtObject {
    id: state
    property var snapshot: ({})
    readonly property var selectedTrack: (snapshot.queue && snapshot.queue.entries || []).find(row => row.selected)?.track || snapshot.selection || null
    readonly property bool playing: !!snapshot.presentation && !!snapshot.presentation.known && snapshot.presentation.status === "Playing"
    property int serial: 0
    property string nonce: Date.now().toString(36)
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
    function request(op) {
        if (!snapshot.instance || !["previous", "next", "pause", "resume", "quit"].includes(op)) return null;
        const queue = snapshot.queue;
        if (!queue || typeof queue.session !== "string" || !queue.session.length || !counter(queue.revision) || !counter(snapshot.account_epoch)) return null;
        return {op: op, instance: snapshot.instance, account_epoch: snapshot.account_epoch, queue_session: queue.session, queue_revision: queue.revision, id: "widget-" + nonce + "-" + (++serial)};
    }
}
