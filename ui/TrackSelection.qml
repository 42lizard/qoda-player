import QtQuick

QtObject {
    id: selection
    property var tracks: []
    property string revision: ""
    property var indexes: []
    property int anchor: -1
    readonly property int count: indexes.length
    readonly property var references: indexes.filter(index => index >= 0 && index < tracks.length).map(index => tracks[index].reference)
    onRevisionChanged: clear()
    onTracksChanged: clear()

    function clear() { indexes = []; anchor = -1; }
    function contains(index) { return indexes.includes(index); }
    function toggle(index) {
        if (index < 0 || index >= tracks.length) return;
        indexes = contains(index) ? indexes.filter(value => value !== index) : indexes.concat([index]).sort((a, b) => a - b);
        anchor = index;
    }
    function choose(index, modifiers) {
        if (index < 0 || index >= tracks.length) return;
        const control = !!(modifiers & Qt.ControlModifier);
        if ((modifiers & Qt.ShiftModifier) && anchor >= 0) {
            let result = control ? indexes.slice() : [];
            for (let i = Math.min(anchor, index); i <= Math.max(anchor, index); ++i)
                if (!result.includes(i)) result.push(i);
            indexes = result.sort((a, b) => a - b);
        } else if (control) toggle(index);
        else { indexes = [index]; anchor = index; }
    }
    function selectAll() { indexes = tracks.map((track, index) => index); anchor = tracks.length ? 0 : -1; }
}
