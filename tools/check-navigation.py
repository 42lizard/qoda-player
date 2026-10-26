#!/usr/bin/env python3
"""Native Previous-path checks with a private, fictional saved session.

Requires a built backend, a Wayland desktop and Quickshell. No credentials,
provider requests, audio loads or desktop config changes.
"""
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent


def check(unavailable=False):
    with tempfile.TemporaryDirectory(prefix="qoda-navigation-check-") as temporary:
        base = Path(temporary)
        original_runtime = os.environ["XDG_RUNTIME_DIR"]
        env = dict(os.environ)
        for name, directory in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")]:
            location = base / directory
            location.mkdir(mode=0o700)
            env[name] = str(location)
        env["DBUS_SESSION_BUS_ADDRESS"] = os.environ.get(
            "DBUS_SESSION_BUS_ADDRESS", "unix:path=" + original_runtime + "/bus")
        if not env.get("WAYLAND_DISPLAY", "").startswith("/"):
            env["WAYLAND_DISPLAY"] = original_runtime + "/" + env.get("WAYLAND_DISPLAY", "wayland-1")
        backend_env = dict(env, DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(base / "missing-bus"))
        session, source = str(uuid.uuid4()), str(uuid.uuid4())
        occurrences = [str(uuid.uuid4()) for _ in range(3)]
        tracks = [{"reference": {"source": source, "key": str(index)},
                   "title": "Fictional track " + str(index), "artist": "Fixture artist",
                   "album": "Fixture album", "duration_ms": None} for index in range(3)]
        queue = {
            "membership": {"session": session, "revision": "3", "entries": [
                {"id": occurrence, "track": track["reference"]}
                for occurrence, track in zip(occurrences, tracks)]},
            "revision": "9007199254740993", "metadata": dict(zip(occurrences, tracks)),
            "selected": occurrences[2], "traversed": occurrences[:2], "priority": [],
            "ended": False, "position_seconds": 12.0, "versions": {}, "order": occurrences,
            "shuffled": False, "repeat": "off", "route": occurrences * 34, "cursor": 101,
            "navigating": False, "recorded": True, "navigation_dropped": "0",
        }
        if unavailable:
            queue["unavailable"] = {occurrences[0]: "missing", occurrences[1]: "unsupported"}
        directory = base / "state/qoda"
        directory.mkdir(mode=0o700)
        database = directory / "session.sqlite3"
        with sqlite3.connect(database) as db:
            db.executescript("CREATE TABLE session(slot INTEGER PRIMARY KEY CHECK(slot=1),payload TEXT NOT NULL); PRAGMA application_id=1364149313; PRAGMA user_version=2;")
            if unavailable:
                db.execute("PRAGMA user_version=3")
            db.execute("INSERT INTO session VALUES(1,?)", [json.dumps({"format": 3 if unavailable else 2, "queue": queue})])
        database.chmod(0o600)
        process = ui = connection = stream = None
        state = None
        with (base / "backend.log").open("w+") as backend_log, (base / "ui.log").open("w+") as ui_log:
            try:
                def receive():
                    nonlocal state
                    line = stream.readline(16 * 1024 * 1024)
                    assert line, "Backend socket closed"
                    state = json.loads(line)
                    return state

                def until(predicate):
                    deadline = time.monotonic() + 10
                    while not predicate(state):
                        assert time.monotonic() < deadline, "Operation timed out"
                        receive()

                def start():
                    nonlocal process, connection, stream
                    process = subprocess.Popen([str(ROOT / "target/debug/qoda-backend")],
                                               env=backend_env, stdout=backend_log, stderr=backend_log)
                    for _ in range(100):
                        assert process.poll() is None, "Backend exited"
                        connection = socket.socket(socket.AF_UNIX)
                        connection.settimeout(10)
                        try:
                            connection.connect(env["XDG_RUNTIME_DIR"] + "/qoda/backend.sock")
                            break
                        except OSError:
                            connection.close()
                            time.sleep(0.1)
                    else:
                        raise AssertionError("Backend did not start")
                    stream = connection.makefile("rwb")
                    stream.write(b'{"op":"hello","protocol":1}\n')
                    stream.flush()
                    receive()
                    until(lambda value: value.get("storage", {}).get("state") == "saved")
                    assert not state.get("error"), state.get("error")
                    assert state["intent"] == "paused" and not state["sign_in"]["connected"]
                    assert state["queue"]["session"] == session

                def command(op, **fields):
                    previous = state["revision"]
                    frame = {"op": op, "instance": state["instance"], **fields}
                    stream.write((json.dumps(frame) + "\n").encode())
                    stream.flush()
                    until(lambda value: value["revision"] != previous)

                def fences():
                    return {"queue_session": state["navigation"]["session"],
                            "queue_revision": state["navigation"]["revision"]}

                def ipc(method):
                    result = subprocess.run(["quickshell", "ipc", "--path", str(ROOT / "ui"),
                                             "call", "--", "control", method], env=env,
                                            capture_output=True, text=True, timeout=5)
                    if result.returncode:
                        raise ValueError("UI not ready")
                    return result.stdout.strip()

                def quit_backend():
                    nonlocal stream, connection
                    stream.write((json.dumps({"op": "quit", "instance": state["instance"]}) + "\n").encode())
                    stream.flush()
                    process.wait(timeout=15)
                    assert process.returncode == 0
                    stream.close(); stream = None
                    connection.close(); connection = None

                start()
                # Output discovery uses control/sysfs metadata only. Applying a
                # route keeps this fictional queue paused and never opens PCM.
                command("refresh_outputs")
                until(lambda value: bool(value.get("output", {}).get("ready")))
                before_output_queue = state["queue"]
                command("set_output", mode="fidelity", device="plughw:0", **fences())
                until(lambda value: bool(value.get("error")))
                assert state["output"]["selection"] == {"mode": "desktop"}
                assert state["queue"] == before_output_queue and state["intent"] == "paused"
                devices = state["output"]["devices"]
                if devices and state["output"]["fidelity_available"]:
                    choice = {"mode": "fidelity", "device": devices[0]["id"]}
                    command("set_output", **choice, **fences())
                    until(lambda value: value["output"]["selection"] == choice and value["output"]["ready"])
                    assert not state.get("error"), state.get("error")
                    assert state["queue"] == before_output_queue and state["intent"] == "paused"
                    assert state["output"]["observed"] is None and state["output"]["qualification"] == "unverified"
                    quit_backend(); start()
                    assert state["output"]["selection"] == choice and state["intent"] == "paused"
                    assert state["queue"] == before_output_queue and state["output"]["observed"] is None
                    choice = {"mode": "fidelity_resampled", "device": devices[0]["id"], "assume_stereo": True}
                    command("set_output", **choice, **fences())
                    until(lambda value: value["output"]["selection"] == choice and value["output"]["ready"])
                    assert state["intent"] == "paused" and state["queue"] == before_output_queue
                    quit_backend(); start()
                    assert state["output"]["selection"] == choice and state["intent"] == "paused"
                    assert state["queue"] == before_output_queue and state["output"]["observed"] is None
                command("set_output", mode="desktop", **fences())
                until(lambda value: not value.get("error") and value["output"]["selection"] == {"mode": "desktop"})
                assert state["intent"] == "paused" and state["queue"] == before_output_queue
                print("Output discovery, route validation and explicit paused switching preserved the saved queue; available Fidelity selection survived paused restart without PCM opens", flush=True)
                if unavailable:
                    assert len(state["queue"]["unavailable_entries"]) == 2
                    before = state["queue"]
                    command("retry_unavailable", occurrence=occurrences[0], **fences())
                    until(lambda value: bool(value.get("error")))
                    assert state["queue"] == before and state["intent"] == "paused"
                    command("navigation_select", index="0", occurrence=occurrences[0], **fences())
                    until(lambda value: value["queue"]["selected"] == occurrences[0])
                    ui = subprocess.Popen(["quickshell", "--path", str(ROOT / "ui"), "--no-duplicate"], env=env, stdout=ui_log, stderr=ui_log)
                    for _ in range(80):
                        assert ui.poll() is None, "Unavailable UI exited"
                        try:
                            if json.loads(ipc("status"))["instance"] == state["instance"]: break
                        except ValueError: pass
                        time.sleep(.1)
                    else: raise AssertionError("Unavailable UI did not connect")
                    ipc("showUnavailable"); time.sleep(.2)
                    info = json.loads(ipc("status"))
                    assert info["unavailable"]["visible"] and info["unavailable"]["entries"] == 2
                    assert not info["unavailable"]["canRetry"]
                    command("queue_remove", occurrence=occurrences[1], **fences())
                    until(lambda value: len(value["queue"]["unavailable_entries"]) == 1)
                    assert state["intent"] == "paused"
                    ipc("showCompact"); ipc("showFull")
                    assert json.loads(ipc("status"))["unavailable"]["visible"]
                    quit_backend(); ui.wait(timeout=5); assert ui.returncode == 0
                    start()
                    assert len(state["queue"]["unavailable_entries"]) == 1
                    assert state["queue"]["selected_unavailable"] == "missing"
                    old = fences()
                    command("queue_remove", occurrence=occurrences[0], **fences())
                    until(lambda value: not value["queue"]["unavailable_entries"])
                    assert state["intent"] == "paused" and state["queue"]["selected"] == occurrences[2]
                    before = state["queue"]
                    command("retry_unavailable", occurrence=occurrences[0], **old)
                    until(lambda value: bool(value.get("error")))
                    assert state["queue"] == before
                    command("stop_clear", **fences())
                    until(lambda value: value["queue"]["membership_count"] == 0)
                    quit_backend()
                    ui_log.flush(); ui_log.seek(0)
                    errors = [line for line in ui_log.read().splitlines() if any(word in line for word in ["TypeError", "ReferenceError", "Cannot assign", "Failed to load", "SyntaxError", "Required property", "Binding loop"])]
                    assert not errors, errors
                    print("Native unavailable rows remained inspectable offline, disabled Retry until sign-in, survived compact/full and paused restart, and supported identity-fenced removal without audio or Plex", flush=True)
                    return
                assert state["navigation"]["total"] == "102"
                assert state["navigation"]["offset"] == "100"
                assert len(state["navigation"]["entries"]) == 2
                before = state["queue"]
                command("navigation_page", offset="0", **fences())
                until(lambda value: value["navigation"]["offset"] == "0")
                assert len(state["navigation"]["entries"]) == 50 and state["queue"] == before
                stale = fences()
                command("navigation_select", index="0", occurrence=occurrences[0], **fences())
                until(lambda value: value["queue"]["selected"] == occurrences[0])
                assert state["intent"] == "paused" and state["queue"]["position_seconds"] == 0
                assert state["navigation"]["total"] == "102" and state["queue"]["navigating"]
                before = state["queue"]
                command("navigation_select", index="1", occurrence=occurrences[1], **stale)
                until(lambda value: bool(value.get("error")))
                assert state["queue"] == before
                command("navigation_select", index="18446744073709551615", occurrence=occurrences[0], **fences())
                until(lambda value: "no longer" in (value.get("error") or ""))
                assert state["queue"] == before
                command("next", **fences())
                until(lambda value: value["queue"]["selected"] == occurrences[1])
                command("previous", **fences())
                until(lambda value: value["queue"]["selected"] == occurrences[0])
                assert state["intent"] == "paused" and state["navigation"]["total"] == "102"
                before = state["queue"]
                command("navigation_page", offset="18446744073709551615", **fences())
                until(lambda value: value["navigation"]["offset"] == "100")
                assert state["queue"] == before
                command("navigation_page", offset=None, **fences())
                until(lambda value: value["navigation"]["following_latest"])
                print("Private restored path paged at 50 rows; exact revisit stayed paused; stale/invalid rows rejected; Previous/Next retained 102 entries", flush=True)

                ui = subprocess.Popen(["quickshell", "--path", str(ROOT / "ui"), "--no-duplicate"],
                                      env=env, stdout=ui_log, stderr=ui_log)
                for _ in range(80):
                    assert ui.poll() is None, "QML exited"
                    try:
                        info = json.loads(ipc("status"))
                        if info["instance"] == state["instance"]:
                            break
                    except ValueError:
                        pass
                    time.sleep(0.1)
                else:
                    raise AssertionError("UI did not connect")
                ipc("showNavigation")
                time.sleep(0.2)
                info = json.loads(ipc("status"))
                assert info["navigation"]["visible"] and info["navigation"]["entries"] == 2
                assert info["navigation"]["viewportHeight"] > 100
                ipc("showCompact"); ipc("hide"); ipc("show"); ipc("showFull")
                assert json.loads(ipc("status"))["navigation"]["visible"]
                quit_backend()
                ui.wait(timeout=5)
                assert ui.returncode == 0
                start()
                assert state["navigation"]["total"] == "102" and state["navigation"]["following_latest"]
                assert state["queue"]["selected"] == occurrences[0]
                command("stop_clear", **fences())
                until(lambda value: value["navigation"]["total"] == "0")
                assert state["navigation"]["entries"] == [] and state["navigation"]["following_latest"]
                assert state["intent"] == "paused"
                quit_backend()
                ui_log.flush(); ui_log.seek(0)
                errors = [line for line in ui_log.read().splitlines() if any(word in line for word in [
                    "TypeError", "ReferenceError", "Cannot assign", "Failed to load", "SyntaxError", "Required property", "Binding loop"])]
                assert not errors, errors
                print("Native Previous-path viewport, compact/hide/restore, paused restart and session-clear reset passed without QML errors; no credentials, Plex or audio loads", flush=True)
            finally:
                if ui and ui.poll() is None:
                    ui.terminate(); ui.wait(timeout=5)
                if process and process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait(timeout=5)
                if stream:
                    stream.close()
                if connection:
                    connection.close()


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--unavailable", action="store_true", help="check persisted unavailable-entry inspection and removal")
    check(parser.parse_args().unavailable)
