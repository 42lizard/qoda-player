#!/usr/bin/env python3
"""Render the production full window against a fictional local socket.

Requires Quickshell and Qt's offscreen/software platform, not a compositor,
Plex, credentials or an audio backend. Only the layer-shell compact surface is
stubbed because Qt offscreen cannot instantiate PanelWindow. Production full
window, browser, drawer and transport are used unchanged. Test-only IPC capture
and resize hooks are added to a temporary UI copy. Pass --output to keep PNGs.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
COMPACT_STUB = """import QtQuick
Item {
    visible: false
    property var snapshot; property var selectedTrack; property bool connected
    property var targetScreen; property bool interactive; property var screen: null
    readonly property bool backingWindowVisible: false
    readonly property int keyboardMode: 0
    signal searchRequested(); signal expandRequested(); signal hideRequested()
    signal quitRequested(); signal transportRequested(string op); signal queueRequested(string op)
}
"""
HOOKS = """
        function resize(w: int, h: int): void {
            window.contentItem.Window.window.width = w;
            window.contentItem.Window.window.height = h;
        }
        function capture(path: string): void { fullFrame.grabToImage(result => result.saveToFile(path)); }
        function captureQueue(path: string): void { queuePane.grabToImage(result => result.saveToFile(path)); }
        function closeQueue(): void { queueDrawer.close(); }
        function scrollRail(): void { railScroll.contentItem.contentY = Math.max(0, railScroll.contentHeight - railScroll.availableHeight); }
        function searchFixture(): void {
            const next = Object.assign({}, root.snapshot);
            next.search = {revision: "9007199254740993", query: "fixture", busy: false, partial: true, message: "2 matches · 2/3 libraries ready · partial results", sources: [{library: "Archive", server: "Studio", phase: "timeout", count: 0, has_more: false, error: "Fixture server unavailable", cache: {stale: false}}], results: [
                {track: {reference: {source: "source-A", key: "42"}, title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry"}, library: "Music", server: "Home", actionable: true},
                {track: {reference: {source: "source-B", key: "42"}, title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry"}, library: "Music", server: "Studio", actionable: true}]};
            next.recovery = {phase: "idle"};
            root.snapshot = next; root.globalQuery = "fixture"; libraryTabs.currentIndex = 4;
        }

        function recoveryFixture(): void { const next = Object.assign({}, root.snapshot); next.recovery = {phase: "exhausted", attempts: 2, limit: 2, message: "Connection interrupted; recovery stopped. Retry or Skip explicitly."}; root.snapshot = next; root.audioExpanded = false; }
"""


def fixture():
    def track(index):
        return {"reference": {"source": "fixture", "key": str(index)},
                "title": ["Soft geometry", "Night walking", "Open water"][index % 3],
                "artist": "Mira Vale", "album": "Soft geometry", "duration_ms": "240000"}

    albums = [{"reference": {"source": "fixture", "key": str(index)},
               "title": ["Soft geometry", "Night walking", "Open water", "Low sun",
                         "Still here", "Morning light"][index % 6],
               "artist": ["Mira Vale", "Northbound", "Lena Moss"][index % 3],
               "year": 2024, "track_count": "9"} for index in range(9)]
    return {
        "protocol": 1, "instance": "fixture", "revision": "1", "account_epoch": "0",
        "status": "Ready", "intent": "paused", "selection": track(0),
        "replaygain": {"mode": "auto", "available": True, "message": "Upcoming preparations only", "metadata_scope": "Embedded FLAC tags"},
        "output": {"selection": {"mode": "fidelity", "device": "fixture"}, "available": True, "ready": True, "fidelity_available": True, "devices": [{"id": "fixture", "name": "Fictional direct PCM"}], "message": "Changing output pauses; explicit Resume required", "qualification": "unverified"},
        "libraries": [{"id": "fixture", "title": "Music", "server": "Home library"}],
        "sign_in": {"connected": True, "saved": "present", "message": "",
                    "can_save": False, "keyring_action": None},
        "storage": {"state": "saved", "message": "Saved"},
        "mpris": {"state": "available", "message": "Available"},
        "gapless": {"enabled": True, "phase": "idle", "message": ""},
        "albums": {
            "page": {"source": "fixture", "offset": "0", "query": "", "albums": albums,
                     "has_more": False},
            "selected": None, "busy": False, "message": "9 albums · Home library",
            "covers": {str(index): (ROOT / "crates/qoda-provider/tests/fixtures/cover.jpg").as_uri()
                       for index in range(9)}},
        "queue": {
            "entries": [{"id": str(index), "selected": index == 0, "can_move": True,
                         "track": track(index)} for index in range(3)],
            "session": "fixture", "revision": "1", "position_seconds": 42,
            "membership_count": 3, "shuffled": False, "repeat": "off",
            "previous_available": False, "navigation_dropped": "0"},
        "navigation": {
            "entries": [{"index": str(index), "occurrence": str(index), "track": track(index),
                         "selected": index == 0} for index in range(2)],
            "session": "fixture", "revision": "1", "offset": "0", "total": "2",
            "following_latest": True, "has_previous": False, "has_next": False},
        "presentation": {"known": False, "can_play": False, "can_pause": False,
                         "can_next": True, "can_previous": False, "can_seek": False},
    }


def check(output=None):
    with tempfile.TemporaryDirectory(prefix="qoda-layout-") as temporary:
        base = Path(temporary)
        output = Path(output).resolve() if output else base / "captures"
        output.mkdir(parents=True, exist_ok=True)
        ui = base / "ui"
        shutil.copytree(ROOT / "ui", ui)
        (ui / "CompactPlayer.qml").write_text(COMPACT_STUB)
        shell = ui / "shell.qml"
        shell.write_text(shell.read_text().replace("        function status(): string", HOOKS + "        function status(): string"))
        runtime = base / "runtime"
        (runtime / "qoda").mkdir(parents=True, mode=0o700)
        state = fixture()
        requests = []
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(runtime / "qoda/backend.sock"))
        server.listen()

        def serve():
            connection, _ = server.accept()
            with connection, connection.makefile("rwb") as stream:
                requests.append(json.loads(stream.readline()))
                stream.write((json.dumps(state) + "\n").encode())
                stream.flush()
                for line in stream:
                    requests.append(json.loads(line))

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), QT_QPA_PLATFORM="offscreen",
                   QT_QUICK_BACKEND="software")
        process = None
        with (base / "ui.log").open("w+") as log:
            try:
                process = subprocess.Popen(["quickshell", "--path", str(ui), "--no-duplicate"],
                                           env=env, stdout=log, stderr=log)

                def ipc(method, *args):
                    result = subprocess.run(["quickshell", "ipc", "--path", str(ui), "call", "--",
                                             "control", method, *map(str, args)], env=env,
                                            text=True, capture_output=True, timeout=5)
                    if result.returncode:
                        raise ValueError(result.stderr)
                    return result.stdout.strip()

                def status():
                    return json.loads(ipc("status"))

                def wait_for(predicate):
                    for _ in range(60):
                        value = status()
                        if predicate(value):
                            return value
                        time.sleep(0.05)
                    raise AssertionError("UI state did not settle: " + repr(value))

                def capture(label, queue=False):
                    path = output / (label + ".png")
                    path.unlink(missing_ok=True)
                    ipc("captureQueue" if queue else "capture", path)
                    for _ in range(50):
                        if path.exists() and path.stat().st_size > 0:
                            return
                        time.sleep(0.05)
                    raise AssertionError("Capture did not complete: " + label)

                for _ in range(80):
                    if process.poll() is not None:
                        log.flush(); log.seek(0)
                        raise AssertionError("Fixture UI exited: " + log.read())
                    try:
                        if status()["instance"] == "fixture":
                            break
                    except (ValueError, json.JSONDecodeError):
                        pass
                    time.sleep(0.1)
                else:
                    raise AssertionError("Fixture UI did not connect")
                for label, width, height in [("wide", 1320, 820), ("narrow", 760, 820), ("small", 540, 760)]:
                    ipc("resize", width, height)
                    info = wait_for(lambda value: value["geometry"]["width"] == width and value["geometry"]["height"] == height and value["geometry"]["queueVisible"] == (label == "wide"))
                    geometry = info["geometry"]
                    assert geometry["width"] == width and geometry["height"] == height
                    assert geometry["browserWidth"] >= 300 and geometry["browserHeight"] > 300
                    assert geometry["queueVisible"] == (label == "wide")
                    assert info["tab"] == 2 and info["albums"]["count"] == 9
                    capture(label)
                    if label == "narrow":
                        ipc("showNavigation")
                        drawer = wait_for(lambda value: value["queueDrawerOpen"])
                        assert drawer["navigation"]["entries"] == 2
                        assert drawer["navigation"]["viewportHeight"] > 300
                        assert drawer["geometry"]["browserWidth"] == geometry["browserWidth"]
                        assert drawer["tab"] == info["tab"] and drawer["query"] == info["query"]
                        capture("drawer", queue=True)
                        ipc("hide")
                        wait_for(lambda value: not value["queueDrawerVisible"])
                        ipc("showFull")
                        time.sleep(0.25)
                        assert not status()["queueDrawerVisible"]
                        assert status()["tab"] == info["tab"]
                        ipc("showNavigation")
                        wait_for(lambda value: value["queueDrawerOpen"])
                        ipc("closeQueue")
                        wait_for(lambda value: not value["queueDrawerVisible"])
                ipc("resize", 1320, 820)
                time.sleep(0.15)
                restored = status()
                assert restored["geometry"]["queueVisible"] and restored["tab"] == 2
                assert restored["queue"] == info["queue"]
                ipc("showAudio")
                time.sleep(0.15)
                capture("audio-wide")
                ipc("resize", 540, 760)
                time.sleep(0.15)
                capture("audio-small")
                ipc("scrollRail")
                time.sleep(0.15)
                rail = status()["geometry"]
                assert rail["railContentHeight"] > rail["railHeight"]
                assert rail["accountTop"] >= 0 and rail["accountBottom"] <= rail["railHeight"] + 1, rail
                capture("audio-rail-scrolled")
                ipc("recoveryFixture")
                time.sleep(0.15)
                capture("recovery-small")
                ipc("searchFixture")
                ipc("resize", 1320, 820)
                time.sleep(0.15)
                assert status()["tab"] == 4 and len(status()["search"]["results"]) == 2
                capture("search-wide")
                ipc("resize", 540, 760)
                time.sleep(0.15)
                capture("search-small")
                assert state["intent"] == "paused"
                assert all(request["op"] == "hello" for request in requests), requests
                log.flush()
                log.seek(0)
                errors = [line for line in log.read().splitlines() if any(text in line for text in
                          ["TypeError", "ReferenceError", "Unable to assign", "Cannot assign",
                           "Failed to load", "SyntaxError", "Binding loop"])]
                assert not errors, "QML runtime errors: " + repr(errors)
                print("Wide/narrow/small full-window rendering, queue drawer, Previous viewport and retained browser/queue state passed")
                print("Fictional socket only; no backend, credentials, Plex, audio or compositor changes")
                if output != base / "captures":
                    print("Captures: " + str(output))
            finally:
                if process and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                server.close()
                worker.join(timeout=2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", help="Keep fictional UI PNGs in this directory")
    check(parser.parse_args().output)
