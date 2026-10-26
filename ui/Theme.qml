pragma Singleton
import QtQuick

QtObject {
    id: theme
    // Only validated resolved colors enter this object. Without Omarchy, keep A.
    property var colors: ({})
    readonly property bool system: !!colors.background
    readonly property color background: colors.background || "#111513"
    readonly property color foreground: colors.foreground || "#e8eee8"
    readonly property color accent: colors.accent || "#c6eb9d"
    readonly property color danger: colors.urgent || colors.red || "#f2a89b"
    readonly property color surface: system ? blend(background, foreground, 0.04) : "#191f1c"
    readonly property color alternate: system ? blend(background, foreground, 0.02) : "#161c18"
    readonly property color button: system ? blend(background, foreground, 0.09) : "#242e27"
    readonly property color border: system ? blend(background, foreground, 0.22) : "#303a33"
    readonly property color selected: system ? blend(background, accent, 0.18) : "#303f32"
    readonly property color muted: system ? blend(background, foreground, 0.76) : "#a4b0a7"
    readonly property color accentText: contrast(accent, "#000000") >= contrast(accent, "#ffffff") ? "#000000" : "#ffffff"
    function blend(a, b, amount) {
        return Qt.rgba(a.r * (1 - amount) + b.r * amount, a.g * (1 - amount) + b.g * amount, a.b * (1 - amount) + b.b * amount, 1);
    }
    function luminance(hex) {
        const s = String(hex).slice(1);
        const channels = [0, 2, 4].map(i => parseInt(s.slice(i, i + 2), 16) / 255).map(v => v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4));
        return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
    }
    function contrast(a, b) {
        const x = luminance(a), y = luminance(b);
        return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
    }
    function apply(resolved) {
        if (resolved.length > 65536) return false;
        const next = {};
        for (const line of resolved.split("\n")) {
            const parts = line.split("\t");
            if (parts.length === 2 && /^[a-z_0-9]+$/.test(parts[0]) && /^#[0-9a-fA-F]{6}$/.test(parts[1])) next[parts[0]] = parts[1];
        }
        if (!next.background || !next.foreground) return false;
        next.accent = next.accent || next.blue || next.foreground;
        colors = next;
        return true;
    }
}
