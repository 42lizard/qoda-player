import QtQuick

QtObject {
    property string backend: ""
    property string token: ""
    function reset() { backend = ""; token = ""; }
    function consume(instance, request) {
        if (!instance) return false;
        if (backend !== instance) { backend = instance; token = request || ""; return false; }
        if (!request || request === token) return false;
        token = request;
        return true;
    }
}
