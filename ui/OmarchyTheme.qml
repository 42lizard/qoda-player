import QtQuick
import Quickshell
import Quickshell.Io

Scope {
    id: source
    // Omarchy currently uses HOME/.local/state; the config path supports older releases.
    property string currentPath: Quickshell.env("HOME") + "/.local/state/omarchy/current/theme/colors.toml"
    property string legacyPath: (Quickshell.env("XDG_CONFIG_HOME") || Quickshell.env("HOME") + "/.config") + "/omarchy/current/theme/colors.toml"
    property string currentText: ""
    property string legacyText: ""
    readonly property string filePath: currentText ? currentPath : legacyText ? legacyPath : ""
    readonly property string fingerprint: filePath + "\n" + (currentText || legacyText)
    property string attempted: ""
    property string resolving: ""
    function refresh() {
        if (!filePath || resolver.running || fingerprint === attempted) return;
        resolving = fingerprint; attempted = fingerprint;
        resolver.command = ["omarchy", "theme", "color", "--file", filePath, "--all"];
        resolver.running = true;
        deadline.restart();
    }
    onFingerprintChanged: { if (!filePath) attempted = ""; refresh(); }
    FileView {
        id: current
        path: source.currentPath; watchChanges: true; printErrors: false
        onLoaded: source.currentText = text().length <= 65536 ? text() : ""
        onFileChanged: reload()
        onLoadFailed: source.currentText = ""
    }
    FileView {
        id: legacy
        path: source.legacyPath; watchChanges: true; printErrors: false
        onLoaded: source.legacyText = text().length <= 65536 ? text() : ""
        onFileChanged: reload()
        onLoadFailed: source.legacyText = ""
    }
    // Also handles atomic directory/symlink replacement and files created later.
    Timer { interval: 2500; running: true; repeat: true; onTriggered: { current.reload(); legacy.reload(); } }
    Timer { id: deadline; interval: 2000; onTriggered: if (resolver.running) { resolver.signal(15); killDeadline.restart(); } }
    Timer { id: killDeadline; interval: 500; onTriggered: if (resolver.running) resolver.signal(9) }
    Process {
        id: resolver
        stdout: StdioCollector { id: output }
        stderr: StdioCollector {}
        onExited: (code, status) => {
            deadline.stop(); killDeadline.stop();
            if (code === 0 && source.resolving === source.fingerprint) Theme.apply(output.text);
            Qt.callLater(source.refresh);
        }
    }
}
