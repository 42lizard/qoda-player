pragma Singleton
import QtQuick
import "i18n/Catalog.js" as Catalog

QtObject {
    // Qt supplies the ordered user UI-language preferences at application start.
    property var preferences: Qt.locale().uiLanguages
    readonly property string language: Catalog.language(preferences)
    function t(source, args) { return Catalog.translate(source, language, args || []); }
    function count(one, many, value) { return t(value === 1 ? one : many, [number(value, 0)]); }
    function number(value, precision) { return Number(value).toLocaleString(Qt.locale(), "f", precision === undefined ? 0 : precision); }
    function state(value) {
        const labels = {off: "Off", on: "On", one: "One", all: "All", track: "Track", album: "Album", auto: "Auto", unity: "Unity", integer: "Integer", float: "Float", live: "Live", loading: "Loading", ready: "Ready", failed: "Failed", timeout: "Timeout", cached: "Cached", pending: "Pending", refreshing: "Refreshing", missing: "Missing", unsupported: "Unsupported", version_changed: "Media version changed"};
        return t(labels[value] || String(value || ""));
    }
    function message(value) { return Catalog.message(value, language); }
}
