import QtQuick

QtObject {
    id: root
    property bool connected: false
    property string instance: ""
    property bool pending: false
    property string owner: ""
    property string error: ""
    property int timeoutMs: 10000
    signal sendQuit()
    signal exitUi()
    property Timer deadline: Timer {
        interval: root.timeoutMs
        onTriggered: {
            root.pending = false;
            root.error = "Quit was not acknowledged. Retry Quit or use qoda quit.";
        }
    }
    function begin() {
        if (pending) return;
        error = "";
        if (!connected || !instance) { exitUi(); return; }
        pending = true;
        owner = instance;
        sendQuit();
        deadline.restart();
    }
    function observe(value) {
        if (value && (value.shutdown || pending && value.instance !== owner)) {
            deadline.stop(); pending = false; exitUi(); return true;
        }
        return false;
    }
}
