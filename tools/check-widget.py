#!/usr/bin/env python3
"""Exercise the actual Omarchy popup/widget against a fictional local backend.

Requires installed Omarchy, Quickshell and Wayland. Uses private runtime/config,
never contacts Plex or real Qoda, and stops every owned process.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("bar_widget", ROOT / "tools/bar-widget.py")
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


def check(output=None):
    with tempfile.TemporaryDirectory(prefix="qoda-widget-check-") as temporary:
        base = Path(temporary)
        env = dict(os.environ)
        old_runtime = env["XDG_RUNTIME_DIR"]
        for name, directory in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")]:
            path = base / directory
            path.mkdir(mode=0o700)
            env[name] = str(path)
        env["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + str(base / "no-bus")
        if not env.get("WAYLAND_DISPLAY", "").startswith("/"):
            env["WAYLAND_DISPLAY"] = old_runtime + "/" + env.get("WAYLAND_DISPLAY", "wayland-1")
        ui = base / "host"
        ui.mkdir()
        for name in ["Ui", "Commons"]:
            (ui / name).symlink_to(Path("/usr/share/omarchy/shell") / name, target_is_directory=True)
        plugin = installer.install(base / "payload")
        shutil.copytree(plugin, ui / "plugin")
        widget = ui / "plugin/BarWidget.qml"
        widget.write_text(widget.read_text().replace('    moduleName: "qoda.player"', '    readonly property var probeClient: client\n    readonly property var probePopup: popup\n    readonly property var probeControls: controls\n    readonly property var probeButton: iconButton\n    moduleName: "qoda.player"'))
        player = base / "player/ui"
        player.mkdir(parents=True)
        (player / "shell.qml").write_text('''import QtQuick
import Quickshell
import Quickshell.Io
ShellRoot {
    property string lastAction: ""
    FloatingWindow { id: window; visible: false; implicitWidth: 340; implicitHeight: 140; title: "Qoda fictional browser" }
    IpcHandler { target: "control"
        function showFull(): void { lastAction = "showFull"; window.visible = true; }
        function showSearch(): void { lastAction = "showSearch"; window.visible = true; }
        function showQueue(): void { lastAction = "showQueue"; window.visible = true; }
        function status(): string { return lastAction; }
    }
}
''')
        (ui / "shell.qml").write_text('''import QtQuick
import QtQuick.Window
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import "plugin" as Plugin
import qs.Ui
ShellRoot {
    property string captureDone: ""
    PanelWindow {
        id: window
        anchors { top: true; left: true; right: true }
        margins.top: 80
        implicitHeight: 36
        exclusionMode: ExclusionMode.Ignore
        color: "#202830"
        WlrLayershell.namespace: "qoda-widget-check"
        property QtObject host: PluginBarApi {
            pluginId: "qoda.player"; moduleName: "qoda.player"
            vertical: false; barSize: 36; position: "top"
            barForeground: "#eeeedd"; foreground: barForeground; fontFamily: "sans-serif"
            _requestPopout: owner => { if (activePopout && activePopout !== owner) activePopout.close(); activePopout = owner; }
            _releasePopout: owner => { if (activePopout === owner) activePopout = null; }
        }
        Plugin.BarWidget { id: widget; x: 200; bar: window.host; settings: ({playerPath: PLAYER_PATH}) }
    }
    IpcHandler { target: "fixture"
        function toggle(): void { widget.probeButton.triggerPress(Qt.LeftButton); }
        function close(): void { widget.close(); }
        function next(): void { widget.probeControls.queueRequested("next"); }
        function play(): void { widget.probeControls.transportRequested("resume"); }
        function expand(): void { widget.probeControls.expandRequested(); }
        function capture(path: string): void { captureDone = ""; widget.probeControls.grabToImage(result => { if (result.saveToFile(path)) captureDone = path; }); }
        function status(): string { return JSON.stringify({connected: widget.probeClient.connected, instance: widget.probeClient.snapshot.instance || "", revision: widget.probeClient.snapshot.revision || "", open: widget.popupOpen, mapped: widget.probePopup.backingWindowVisible, known: widget.probeControls.known, width: widget.probePopup.width, height: widget.probePopup.height, capture: captureDone, opening: widget.openingLibrary}); }
    }
}
'''.replace("PLAYER_PATH", json.dumps(str(player.parent))))
        runtime = base / "runtime/qoda"
        runtime.mkdir(mode=0o700)
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(runtime / "backend.sock"))
        server.listen()
        server.settimeout(0.2)
        stop = threading.Event()
        connections = []
        requests = []
        snapshot = {"protocol": 1, "instance": str(uuid.uuid4()), "revision": "9007199254740993", "account_epoch": "9007199254740995", "intent": "paused", "queue": {"session": str(uuid.uuid4()), "revision": "18446744073709551614", "entries": []}, "presentation": {"known": False, "can_play": False, "can_next": False}, "status": "Synthetic fixture only"}

        def serve():
            while not stop.is_set():
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                except OSError:
                    return
                connections.append(connection)
                try:
                    with connection.makefile("rb") as stream:
                        for line in stream:
                            request = json.loads(line)
                            requests.append(request)
                            if request["op"] == "hello":
                                connection.sendall((json.dumps(snapshot) + "\n").encode())
                except OSError:
                    pass

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        processes = []
        with (base / "ui.log").open("w+") as log:
            def ipc(path, target, method, *args):
                result = subprocess.run(["quickshell", "ipc", "--path", str(path), "call", "--", target, method, *map(str, args)], env=env, text=True, capture_output=True, timeout=5)
                if result.returncode:
                    raise ValueError(result.stderr)
                return result.stdout.strip()

            def status():
                return json.loads(ipc(ui, "fixture", "status"))

            def until(predicate):
                for _ in range(100):
                    assert processes[0].poll() is None, "Fixture host exited"
                    try:
                        state = status()
                        if predicate(state):
                            return state
                    except (ValueError, json.JSONDecodeError):
                        pass
                    time.sleep(0.05)
                raise AssertionError("Widget did not settle")

            def push():
                connections[-1].sendall((json.dumps(snapshot) + "\n").encode())

            try:
                processes.append(subprocess.Popen(["quickshell", "--path", str(ui), "--no-duplicate"], env=env, stdout=log, stderr=log))
                until(lambda s: s["connected"])
                assert requests == [{"op": "hello", "protocol": 1}], requests
                ipc(ui, "fixture", "toggle")
                state = until(lambda s: s["open"] and s["mapped"])
                assert not state["known"] and len(requests) == 1
                assert state["width"] <= 440 and state["height"] <= 390
                if output:
                    directory = Path(output).resolve(); directory.mkdir(parents=True, exist_ok=True)
                    image = directory / "widget.png"
                    ipc(ui, "fixture", "capture", image)
                    until(lambda s: s["capture"] == str(image))
                ipc(ui, "fixture", "close")
                until(lambda s: not s["open"] and not s["mapped"])
                ipc(ui, "fixture", "next")
                for _ in range(60):
                    if len(requests) > 1:
                        break
                    time.sleep(0.05)
                command = requests[-1]
                assert command["op"] == "next" and command["instance"] == snapshot["instance"]
                assert command["queue_revision"] == "18446744073709551614" and command["account_epoch"] == "9007199254740995"
                # Browser restoration is UI-only and adds no transport request.
                processes.append(subprocess.Popen(["quickshell", "--path", str(player), "--no-duplicate"], env=env, stdout=log, stderr=log))
                for _ in range(80):
                    try:
                        ipc(player, "control", "status"); break
                    except ValueError:
                        time.sleep(0.05)
                before = len(requests)
                ipc(ui, "fixture", "expand")
                for _ in range(80):
                    if ipc(player, "control", "status") == "showFull":
                        break
                    time.sleep(0.05)
                else:
                    raise AssertionError("Expand did not restore browser")
                assert len(requests) == before
                connections[-1].shutdown(socket.SHUT_RDWR)
                connections[-1].close()
                snapshot["instance"] = str(uuid.uuid4()); snapshot["revision"] = "0"
                until(lambda s: s["connected"] and s["instance"] == snapshot["instance"])
                assert requests[-1] == {"op": "hello", "protocol": 1}
                ipc(ui, "fixture", "play")
                for _ in range(60):
                    if requests[-1]["op"] == "resume":
                        break
                    time.sleep(0.05)
                assert requests[-1]["instance"] == snapshot["instance"]
                print("Anchored widget popup open/close and hidden-browser restoration passed without implicit playback")
                print("Lossless command fences, backend reconnect and non-raising subscriptions passed")
            finally:
                for process in reversed(processes):
                    if process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill(); process.wait(timeout=5)
                stop.set(); server.close()
                for connection in connections:
                    try:
                        connection.close()
                    except OSError:
                        pass
                worker.join(timeout=2)
                log.flush(); log.seek(0)
                text = log.read()
                errors = [line for line in text.splitlines() if any(value in line for value in ["Failed to load", "ReferenceError", "TypeError", "Unable to assign", "Binding loop"])]
                if errors or not processes or processes[0].returncode not in [0, -15]:
                    raise AssertionError("Widget QML errors:\n" + text)
        print("Fictional backend only; no credentials, Plex or audio; owned processes stopped")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output")
    check(parser.parse_args().output)
