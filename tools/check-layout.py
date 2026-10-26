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
        property string captureDone: ""
        function captureStatus(): string { return captureDone; }
        function resize(w: int, h: int): void {
            window.contentItem.Window.window.width = w;
            window.contentItem.Window.window.height = h;
        }
        function capture(path: string): void { captureDone = ""; fullFrame.grabToImage(result => { if (result.saveToFile(path)) captureDone = path; }); }
        function captureSettings(path: string): void { captureDone = ""; settingsSheet.contentItem.grabToImage(result => { if (result.saveToFile(path)) captureDone = path; }); }
        function settingsGeometry(): string { function walk(item) { return {type: String(item), x:item.x,y:item.y,w:item.width,h:item.height,ih:item.implicitHeight,visible:item.visible,children:(item.children || []).map(walk)}; } return JSON.stringify(walk(settingsSheet.contentItem)); }
        function closeSettings(): void { root.closeSettings(); }
        function captureQueue(path: string): void { captureDone = ""; queuePane.grabToImage(result => { if (result.saveToFile(path)) captureDone = path; }); }
        function closeQueue(): void { queueDrawer.close(); }
        function queueDragStart(id: string): bool { queueTabs.currentIndex = 0; return queueReorder.begin(id); }
        function queueDragDrop(id: string, after: bool): bool { return queueReorder.propose(id, after) && queueReorder.finish(true); }
        function queueFenceChange(): void { const next = Object.assign({}, root.snapshot); next.queue = Object.assign({}, next.queue, {revision: "2"}); root.snapshot = next; }
        function playlistDragFixture(): void {
            const next = Object.assign({}, root.snapshot);
            next.playlists = {busy: false, uncertain: false, list: null, message: "Fixture playlist", selected: {revision: "fixture-r1", playlist: {reference: {source: "fixture", key: "playlist"}, title: "Fixture playlist", can_edit: true, smart: false}, entries: [
                {id: "item-1", title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry", track: {reference: {source: "fixture", key: "42"}}},
                {id: "item-2", title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry", track: {reference: {source: "fixture", key: "42"}}},
                {id: "item-3", title: "Night walking", artist: "Mira Vale", album: "Soft geometry", track: {reference: {source: "fixture", key: "43"}}}]}};
            root.snapshot = next; root.playlistEntries = next.playlists.selected.entries;
            const library = root.libraryId; root.libraryId = ""; libraryTabs.currentIndex = 1; root.libraryId = library;
        }
        function playlistDragStart(id: string): bool { return playlistReorder.begin(id); }
        function playlistDragDrop(id: string, after: bool): bool { return playlistReorder.propose(id, after) && playlistReorder.finish(true); }
        function playlistDragRestricted(mode: string): void {
            const next = JSON.parse(JSON.stringify(root.snapshot));
            next.playlists.busy = mode === "busy"; next.playlists.uncertain = mode === "uncertain";
            next.playlists.selected.playlist.can_edit = mode !== "readonly";
            next.playlists.selected.playlist.smart = mode === "smart";
            root.snapshot = next;
        }

        function libraryFixture(): void {
            const next = Object.assign({}, root.snapshot);
            next.libraries = [{id: "fixture", title: "Music", server: "Home library"}, {id: "sameprefix-A", title: "Music", server: "Studio library"}, {id: "sameprefix-B", title: "Music", server: "Studio library"}];
            root.snapshot = next; root.libraryModel = next.libraries;
            root.libraryId = ""; libraryTabs.currentIndex = 2; root.libraryId = "fixture";
        }
        function openLibraries(): void { libraryPicker.children.find(item => item.objectName === "libraryCombo").popup.open(); }
        function captureLibraries(path: string): void { captureDone = ""; libraryPicker.children.find(item => item.objectName === "libraryCombo").popup.contentItem.grabToImage(result => { if (result.saveToFile(path)) captureDone = path; }); }
        function chooseLibrary(source: string): void { libraryPicker.chosen(source); }
        function scrollRail(): void { railScroll.contentItem.contentY = Math.max(0, railScroll.contentHeight - railScroll.availableHeight); }
        function conversionFixture(): void {
            const next = Object.assign({}, root.snapshot);
            next.output = Object.assign({}, next.output, {selection: {mode: "fidelity_resampled", device: "fixture", assume_stereo: true}, processing: {source: {rate: 44100, bits: 24}, target: {rate: 48000, bits: 16}, sample: "s16"}, source: {rate: 44100, bits: 24}, observed: {rate: 48000, format: "s16", valid_bits: 16, stereo_layout: false, stereo_assumed: true}});
            root.snapshot = next; root.showSettings("audio");
        }
        function searchFixture(): void {
            const next = Object.assign({}, root.snapshot);
            next.search = {revision: "9007199254740993", query: "fixture", busy: false, partial: true, message: "2 matches · 2/3 libraries ready · partial results", sources: [{library: "Archive", server: "Studio", phase: "timeout", count: 0, has_more: false, error: "Fixture server unavailable", cache: {stale: false}}], results: [
                {track: {reference: {source: "source-A", key: "42"}, title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry"}, library: "Music", server: "Home", actionable: true},
                {track: {reference: {source: "source-B", key: "42"}, title: "Soft geometry", artist: "Mira Vale", album: "Soft geometry"}, library: "Music", server: "Studio", actionable: true}]};
            next.recovery = {phase: "idle"};
            root.snapshot = next; root.globalQuery = "fixture"; libraryTabs.currentIndex = 4;
        }

        function recoveryFixture(): void { const next = Object.assign({}, root.snapshot); next.recovery = {phase: "exhausted", attempts: 2, limit: 2, message: "Connection interrupted; recovery stopped. Retry or Skip explicitly."}; root.snapshot = next; root.closeSettings(); }
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
            "entries": [{"id": str(index), "selected": index == 0, "can_move": index > 0,
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

                def capture(label, queue=False, settings=False, libraries=False):
                    path = output / (label + ".png")
                    path.unlink(missing_ok=True)
                    ipc("captureLibraries" if libraries else "captureSettings" if settings else "captureQueue" if queue else "capture", path)
                    for _ in range(50):
                        if ipc("captureStatus") == str(path) and path.exists() and path.stat().st_size > 0:
                            return
                        time.sleep(0.05)
                    log.flush(); log.seek(0)
                    raise AssertionError("Capture did not complete: " + label + "\n" + log.read())

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
                capture("audio-wide", settings=True)
                ipc("resize", 540, 760)
                time.sleep(0.15)
                capture("audio-small", settings=True)
                settings_geometry = json.loads(ipc("settingsGeometry"))
                def assert_bounds(item):
                    if not item["visible"]:
                        return
                    for child in item["children"]:
                        if child["visible"] and child["w"] > 0 and child["h"] > 0:
                            assert child["w"] <= item["w"] + 1, (item, child)
                            assert_bounds(child)
                assert_bounds(settings_geometry)
                ipc("showAccount")
                time.sleep(0.15)
                capture("account-small", settings=True)
                ipc("showCache")
                time.sleep(0.15)
                capture("cache-small", settings=True)
                ipc("conversionFixture")
                time.sleep(0.15)
                capture("hq-conversion-small", settings=True)
                ipc("resize", 540, 440)
                time.sleep(0.15)
                assert status()["settings"]["scrollable"]
                capture("hq-conversion-short", settings=True)
                ipc("resize", 540, 760)
                time.sleep(0.15)
                ipc("scrollRail")
                time.sleep(0.15)
                rail = status()["geometry"]
                assert status()["settings"]["visible"]
                assert rail["browserHeight"] > 250
                assert rail["accountTop"] >= 0 and rail["accountBottom"] <= rail["railHeight"] + 1, rail
                capture("audio-rail-scrolled", settings=True)
                ipc("showSearch")
                focused = wait_for(lambda value: value["searchFocused"] and not value["settings"]["visible"])
                assert focused["tab"] == 4 and focused["form"] == "full"
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
                ipc("resize", 1320, 820)
                time.sleep(0.15)
                assert ipc("queueDragStart", "0") == "false"
                assert ipc("queueDragStart", "1") == "true"
                capture("queue-drag-proposal", queue=True)
                assert ipc("queueDragDrop", "2", "true") == "true"
                deadline = time.monotonic() + 3
                while len([request for request in requests if request["op"] == "queue_move"]) < 1:
                    assert time.monotonic() < deadline
                    time.sleep(0.01)
                move = [request for request in requests if request["op"] == "queue_move"][0]
                assert move["occurrence"] == "1" and move["before"] is None
                assert move["queue_revision"] == "1" and move["queue_session"] == "fixture"
                assert move["account_epoch"] == "0" and move["instance"] == "fixture"
                assert [entry["id"] for entry in status()["queue"]["entries"]] == ["0", "1", "2"]
                assert ipc("queueDragStart", "1") == "true"
                ipc("queueFenceChange")
                assert not status()["reorder"]["queueActive"]
                assert ipc("queueDragDrop", "2", "true") == "false"
                ipc("playlistDragFixture")
                time.sleep(0.15)
                capture("playlist-drag-wide")
                assert ipc("playlistDragStart", "item-3") == "true"
                assert ipc("playlistDragDrop", "item-1", "false") == "true"
                deadline = time.monotonic() + 3
                while len([request for request in requests if request["op"] == "playlist_edit"]) < 1:
                    assert time.monotonic() < deadline
                    time.sleep(0.01)
                edit = [request for request in requests if request["op"] == "playlist_edit"][0]
                assert edit["reference"] == {"source": "fixture", "key": "playlist"}
                assert edit["expected_revision"] == "fixture-r1"
                assert edit["edit"] == {"action": "move", "item_id": "item-3", "after": None}
                assert edit["account_epoch"] == "0" and edit["instance"] == "fixture"
                assert status()["playlists"]["order"] == ["item-1", "item-2", "item-3"]
                for restriction in ["readonly", "smart", "busy", "uncertain"]:
                    ipc("playlistDragRestricted", restriction)
                    assert not status()["reorder"]["playlistEnabled"]
                    assert ipc("playlistDragStart", "item-1") == "false"
                ipc("playlistDragRestricted", "ready")
                ipc("resize", 540, 760)
                time.sleep(0.15)
                capture("playlist-drag-small")
                assert len([request for request in requests if request["op"] == "queue_move"]) == 1
                assert len([request for request in requests if request["op"] == "playlist_edit"]) == 1
                before_library = len(requests)
                ipc("libraryFixture")
                ipc("openLibraries")
                wait_for(lambda value: value["libraryPicker"]["open"])
                capture("library-choices-small", libraries=True)
                ipc("showSearch")
                wait_for(lambda value: value["searchFocused"] and not value["libraryPicker"]["open"])
                ipc("libraryFixture")
                ipc("chooseLibrary", "sameprefix-B")
                chosen = wait_for(lambda value: value["libraryPicker"]["source"] == "sameprefix-B")
                assert chosen["libraryPicker"]["label"] == "Music · Studio library · sameprefix-B"
                deadline = time.monotonic() + 3
                while len(requests) < before_library + 1:
                    assert time.monotonic() < deadline
                    time.sleep(0.01)
                assert [request["op"] for request in requests[before_library:]] == ["album_list"]
                assert requests[-1]["source"] == "sameprefix-B"
                capture("library-selected-small")
                assert all(request["op"] in ["hello", "queue_move", "playlist_edit", "album_list"] for request in requests), requests
                log.flush()
                log.seek(0)
                errors = [line for line in log.read().splitlines() if any(text in line for text in
                          ["TypeError", "ReferenceError", "Unable to assign", "Cannot assign",
                           "Failed to load", "SyntaxError", "Binding loop"])]
                assert not errors, "QML runtime errors: " + repr(errors)
                print("Wide/narrow/small full-window rendering, queue drawer, Previous viewport and retained browser/queue state passed")
                print("Settings retained browser space; narrow content bounds and delayed Search focus passed")
                print("Production queue/playlist drag wiring preserved frozen fences and backend-owned order; stale/restricted moves refused")
                print("Source-labelled duplicate libraries, popup dismissal/Search focus and one-source browse without playback passed")
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
