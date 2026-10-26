#!/usr/bin/env python3
"""Cold-start offline browsing with private synthetic cache/session data.

Requires a built backend, Quickshell and a Wayland desktop. No keyring access,
provider requests, media preparation, audio playback or desktop configuration.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent


def encoded(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()


def check():
    with tempfile.TemporaryDirectory(prefix="qoda-offline-check-") as temporary:
        base = Path(temporary)
        env = dict(os.environ)
        old_runtime = env["XDG_RUNTIME_DIR"]
        for name, directory in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")]:
            path = base / directory
            path.mkdir(mode=0o700)
            env[name] = str(path)
        if not env.get("WAYLAND_DISPLAY", "").startswith("/"):
            env["WAYLAND_DISPLAY"] = old_runtime + "/" + env.get("WAYLAND_DISPLAY", "wayland-1")
        env["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + str(base / "missing-bus")
        sources = [str(uuid.uuid4()), str(uuid.uuid4())]
        tracks = [{"reference": {"source": sources[0], "key": str(i)}, "title": "Fictional track " + str(i),
                   "artist": "Fixture artist", "album": "Fixture album", "duration_ms": None} for i in range(3)]
        discovery = {"version": 1, "account": "a" * 64, "fetched": int(time.time()) - 86400,
                     "libraries": [{"id": source, "title": "Music", "server": "Fixture " + str(i)} for i, source in enumerate(sources)]}
        cache = base / "cache/qoda/catalog-v1"
        cache.mkdir(parents=True, mode=0o700)
        database = cache / "catalog.sqlite3"
        with sqlite3.connect(database) as db:
            db.executescript("""CREATE TABLE pages(key TEXT PRIMARY KEY,source TEXT NOT NULL,payload BLOB NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL);
                CREATE TABLE tracks(source TEXT NOT NULL,key TEXT NOT NULL,payload BLOB NOT NULL,title TEXT NOT NULL,artist TEXT NOT NULL,album TEXT NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(source,key));
                CREATE TABLE discovery(id INTEGER PRIMARY KEY CHECK(id=1),payload BLOB NOT NULL,bytes INTEGER NOT NULL);
                PRAGMA application_id=1364149315;PRAGMA user_version=2;""")
            blob = encoded(discovery)
            db.execute("INSERT INTO discovery VALUES(1,?,?)", (blob, len(blob) + 128))
            page = {"source": sources[0], "offset": "0", "total": None, "query": "", "tracks": tracks, "has_more": False}
            query = {"kind": "tracks", "source": sources[0], "offset": 0, "query": ""}
            blob = encoded({"kind": "tracks", "page": page})
            db.execute("INSERT INTO pages VALUES(?,?,?,?,?,?)", (hashlib.sha256(encoded(query)).hexdigest(), sources[0], blob, 1, 1, len(blob) + 128))
            for track in tracks:
                blob = encoded(track)
                db.execute("INSERT INTO tracks VALUES(?,?,?,?,?,?,?,?,?)", (sources[0], track["reference"]["key"], blob, track["title"].lower(), track["artist"].lower(), track["album"].lower(), 1, 1, len(blob) + 128))
        database.chmod(0o600)
        session = str(uuid.uuid4())
        occurrences = [str(uuid.uuid4()) for _ in tracks]
        queue = {"membership": {"session": session, "revision": "3", "entries": [{"id": occurrence, "track": track["reference"]} for occurrence, track in zip(occurrences, tracks)]},
                 "revision": "9007199254740993", "metadata": dict(zip(occurrences, tracks)), "selected": occurrences[2], "traversed": occurrences[:2], "priority": [], "ended": False,
                 "position_seconds": 12.0, "versions": {}, "order": occurrences, "shuffled": False, "repeat": "off", "route": occurrences * 34, "cursor": 101, "navigating": False, "recorded": True, "navigation_dropped": "0"}
        state_dir = base / "state/qoda"
        state_dir.mkdir(mode=0o700)
        with sqlite3.connect(state_dir / "session.sqlite3") as db:
            db.executescript("CREATE TABLE session(slot INTEGER PRIMARY KEY CHECK(slot=1),payload TEXT NOT NULL);PRAGMA application_id=1364149313;PRAGMA user_version=2;")
            db.execute("INSERT INTO session VALUES(1,?)", [encoded({"format": 2, "queue": queue}).decode()])
        (state_dir / "session.sqlite3").chmod(0o600)
        ui_path = base / "ui"
        shutil.copytree(ROOT / "ui", ui_path)
        shell = ui_path / "shell.qml"
        shell.write_text(shell.read_text().replace("        function status(): string", '        function fixtureSearch(): void { root.globalQuery = "Fictional"; root.focusSearch(); search.text = "Fictional"; root.searchCatalog(); }\n        function status(): string'))
        process = ui = connection = stream = None
        snapshot = None
        with (base / "backend.log").open("w+") as backend_log, (base / "ui.log").open("w+") as ui_log:
            def receive():
                nonlocal snapshot
                line = stream.readline(16 * 1024 * 1024)
                assert line, "Backend socket closed"
                snapshot = json.loads(line)

            def until(predicate):
                deadline = time.monotonic() + 12
                while not predicate(snapshot):
                    assert time.monotonic() < deadline, "Backend operation timed out: " + repr({key: snapshot.get(key) for key in ["libraries", "discovery", "metadata_cache_action", "metadata_cache", "status", "error"]})
                    receive()

            def start():
                nonlocal process, connection, stream
                process = subprocess.Popen([str(ROOT / "target/debug/qoda-backend")], env=env, stdout=backend_log, stderr=backend_log)
                for _ in range(100):
                    assert process.poll() is None, "Backend exited"
                    connection = socket.socket(socket.AF_UNIX)
                    connection.settimeout(12)
                    try:
                        connection.connect(env["XDG_RUNTIME_DIR"] + "/qoda/backend.sock")
                        break
                    except OSError:
                        connection.close(); time.sleep(0.1)
                else:
                    raise AssertionError("Backend did not start")
                stream = connection.makefile("rwb")
                stream.write(b'{"op":"hello","protocol":1}\n'); stream.flush(); receive()
                assert snapshot["intent"] == "paused" and not snapshot["sign_in"]["connected"]
                assert snapshot["queue"]["session"] == session
                assert snapshot["queue"]["position_seconds"] == 12
                assert not snapshot["presentation"]["known"]

            def command(op, **fields):
                previous = snapshot["revision"]
                stream.write(encoded({"op": op, "instance": snapshot["instance"], "account_epoch": snapshot["account_epoch"], **fields}) + b"\n")
                stream.flush()
                until(lambda s: s["revision"] != previous)

            def fences():
                return {"queue_session": snapshot["queue"]["session"], "queue_revision": snapshot["queue"]["revision"]}

            def ipc(method):
                result = subprocess.run(["quickshell", "ipc", "--path", str(ui_path), "call", "--", "control", method], env=env, text=True, capture_output=True, timeout=5)
                if result.returncode:
                    raise ValueError("UI not ready")
                return result.stdout.strip()

            def ui_until(predicate):
                for _ in range(80):
                    assert ui.poll() is None, "UI exited"
                    try:
                        state = json.loads(ipc("status"))
                        if predicate(state):
                            return state
                    except (ValueError, json.JSONDecodeError):
                        pass
                    time.sleep(0.1)
                raise AssertionError("UI did not settle")

            def stop():
                nonlocal stream, connection
                stream.write(encoded({"op": "quit", "instance": snapshot["instance"]}) + b"\n"); stream.flush()
                process.wait(timeout=15)
                assert process.returncode == 0
                stream.close(); connection.close(); stream = connection = None

            try:
                start()
                assert len(snapshot["libraries"]) == 2 and all(l["offline"] for l in snapshot["libraries"])
                assert snapshot["discovery"]["origin"] == "offline"
                before = snapshot["queue"]
                command("browse", source=sources[0], offset="0", query="")
                until(lambda s: s.get("page") is not None)
                assert len(snapshot["page"]["tracks"]) == 3
                assert snapshot["catalog"]["cache"]["origin"] == "offline"
                ui = subprocess.Popen(["quickshell", "--path", str(ui_path), "--no-duplicate"], env=env, stdout=ui_log, stderr=ui_log)
                ui_until(lambda s: s["tracks"] == 3 and s["libraries"] == 2)
                ipc("browseTracks")
                state = ui_until(lambda s: s["tab"] == 0)
                assert not state["trackPageReady"] and "cached" in state["libraryPicker"]["label"]
                ipc("fixtureSearch")
                state = ui_until(lambda s: s.get("search", {}).get("query") == "Fictional" and not s["search"]["busy"])
                assert len(state["search"]["results"]) == 3 and state["search"]["partial"]
                assert all(not row["actionable"] for row in state["search"]["results"])
                for op in ["enqueue", "play", "resume", "reconnect"]:
                    command(op, source=sources[0], key="0", **fences())
                    assert snapshot["intent"] == "paused" and snapshot["queue"] == before
                command("browse", source=sources[1], offset="0", query="")
                until(lambda s: not s["catalog"]["busy"])
                assert snapshot["queue"] == before
                stop(); ui.wait(timeout=8); ui = None
                start()
                assert len(snapshot["libraries"]) == 2
                command("clear_artwork_cache")
                until(lambda s: s.get("cache_action") is None)
                assert len(snapshot["libraries"]) == 2 and snapshot["discovery"]["origin"] == "offline"
                command("clear_metadata_cache")
                until(lambda s: not s["libraries"] and s.get("metadata_cache_action") is None)
                assert snapshot["libraries"] == [] and snapshot["queue"] == before
                stop(); start()
                assert snapshot["libraries"] == [] and snapshot["discovery"]["origin"] == "none"
                stop()
                # Newer discovery payloads are preserved without exposing a library.
                future = encoded({**discovery, "version": 2})
                with sqlite3.connect(database) as db:
                    db.execute("INSERT INTO discovery VALUES(1,?,?)", (future, len(future) + 128))
                start()
                assert snapshot["libraries"] == []
                stop()
                with sqlite3.connect(database) as db:
                    assert db.execute("SELECT payload FROM discovery").fetchone()[0] == future
                ui_log.flush(); ui_log.seek(0)
                errors = [line for line in ui_log.read().splitlines() if any(word in line for word in ["TypeError", "ReferenceError", "Unable to assign", "Failed to load", "Binding loop"])]
                assert not errors, errors
                print("Cold offline discovery, expired cache browsing/search and source-labelled read-only UI passed")
                print("Queue/position restored paused; cached actions and reconnect without credentials could not start playback or mutate the queue")
                print("Cache clearing survived restart; newer discovery was preserved and never displayed")
                print("Private synthetic data only; no keyring, Plex, media preparation or audio; owned processes stopped")
            finally:
                for child in [ui, process]:
                    if child and child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            child.kill(); child.wait(timeout=5)
                if stream:
                    stream.close()
                if connection:
                    connection.close()


if __name__ == "__main__":
    check()
