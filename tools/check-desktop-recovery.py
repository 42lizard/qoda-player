#!/usr/bin/env python3
"""Exercise coordinator MPRIS recovery on a private bus with no service activation.

Build the backend first. Uses empty private state and no UI, credentials,
Plex, audio loads or desktop configuration changes.
"""
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
NAME = "org.mpris.MediaPlayer2.qoda"


def check():
    with tempfile.TemporaryDirectory(prefix="qoda-desktop-recovery-") as temporary:
        base = Path(temporary)
        env = dict(os.environ)
        for name, directory in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")]:
            location = base / directory
            location.mkdir(mode=0o700)
            env[name] = str(location)
        bus_path = base / "bus"
        address = "unix:path=" + str(bus_path)
        env["DBUS_SESSION_BUS_ADDRESS"] = address
        config = base / "bus.conf"
        config.write_text(f"<busconfig><type>session</type><listen>{address}</listen><auth>EXTERNAL</auth><policy context='default'><allow user='{os.getuid()}'/><allow own='*'/><allow send_destination='*'/><allow receive_sender='*'/></policy></busconfig>")
        process = daemon = connection = stream = listener = accepted = None
        state = None
        with (base / "backend.log").open("w+") as log:
            try:
                def receive_until(predicate):
                    nonlocal state
                    deadline = time.monotonic() + 8
                    while state is None or not predicate(state):
                        assert time.monotonic() < deadline, "Coordinator operation timed out"
                        line = stream.readline(16 * 1024 * 1024)
                        assert line, "Backend socket closed"
                        state = json.loads(line)

                def send(op, **fields):
                    frame = {"op": op, "instance": state["instance"], **fields}
                    stream.write((json.dumps(frame) + "\n").encode())
                    stream.flush()

                def bus(*arguments):
                    result = subprocess.run(["/usr/bin/busctl", "--address=" + address,
                                             "--timeout=3", *arguments], capture_output=True,
                                            text=True, timeout=5)
                    assert result.returncode == 0, result.stderr
                    return result.stdout.strip()

                def start_bus():
                    nonlocal daemon
                    bus_path.unlink(missing_ok=True)
                    daemon = subprocess.Popen(["/usr/bin/dbus-daemon", "--nofork", "--print-address=1",
                                               "--config-file=" + str(config)], stdout=subprocess.PIPE,
                                              stderr=subprocess.DEVNULL, text=True)
                    assert select.select([daemon.stdout], [], [], 3)[0], "Private bus did not start"
                    assert daemon.stdout.readline().strip().startswith(address)

                def stop_bus():
                    nonlocal daemon
                    daemon.terminate(); daemon.wait(timeout=5)
                    daemon.stdout.close(); daemon = None
                    bus_path.unlink(missing_ok=True)

                def hung_bus():
                    nonlocal listener
                    bus_path.unlink(missing_ok=True)
                    listener = socket.socket(socket.AF_UNIX)
                    listener.bind(str(bus_path)); listener.listen()
                    listener.settimeout(5)

                def close_hung_bus():
                    nonlocal listener, accepted
                    if accepted:
                        accepted.close(); accepted = None
                    if listener:
                        listener.close(); listener = None
                    bus_path.unlink(missing_ok=True)

                process = subprocess.Popen([str(ROOT / "target/debug/qoda-backend")], env=env,
                                           stdout=log, stderr=log)
                for _ in range(100):
                    assert process.poll() is None, "Backend exited"
                    connection = socket.socket(socket.AF_UNIX); connection.settimeout(8)
                    try:
                        connection.connect(env["XDG_RUNTIME_DIR"] + "/qoda/backend.sock")
                        break
                    except OSError:
                        connection.close(); time.sleep(0.1)
                else:
                    raise AssertionError("Backend did not start")
                stream = connection.makefile("rwb")
                stream.write(b'{"op":"hello","protocol":1}\n'); stream.flush()
                receive_until(lambda value: value.get("mpris", {}).get("state") == "unavailable")
                assert state["intent"] == "paused" and not state["sign_in"]["connected"]
                instance = state["instance"]
                hung_bus()
                send("retry_desktop")
                receive_until(lambda value: value["mpris"]["state"] == "connecting")
                accepted, _ = listener.accept()
                send("retry_desktop")  # One pending attempt; duplicate requests do not restart it.
                started = time.monotonic()
                send("pause")
                send("set_repeat", mode="all", queue_session=state["queue"]["session"],
                     queue_revision=state["queue"]["revision"])
                receive_until(lambda value: value["queue"]["repeat"] == "all")
                assert time.monotonic() - started < 2
                assert state["mpris"]["state"] == "connecting" and state["intent"] == "paused"
                receive_until(lambda value: value["mpris"]["state"] == "unavailable")
                close_hung_bus()
                before = state["queue"]
                start_bus(); send("retry_desktop")
                receive_until(lambda value: value["mpris"]["state"] == "available")
                assert state["queue"] == before and state["instance"] == instance
                owner = bus("call", "org.freedesktop.DBus", "/org/freedesktop/DBus",
                            "org.freedesktop.DBus", "GetNameOwner", "s", NAME)
                send("retry_desktop")
                bus("set-property", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", "Shuffle", "b", "true")
                receive_until(lambda value: value["queue"]["shuffled"])
                assert bus("call", "org.freedesktop.DBus", "/org/freedesktop/DBus",
                           "org.freedesktop.DBus", "GetNameOwner", "s", NAME) == owner
                print("A hung retry left transport responsive; timeout/retry recovered desktop control on the same backend without changing the queue", flush=True)
                before = state["queue"]
                stop_bus()
                receive_until(lambda value: value["mpris"]["state"] == "unavailable")
                assert state["queue"] == before and state["intent"] == "paused"
                start_bus(); send("retry_desktop")
                receive_until(lambda value: value["mpris"]["state"] == "available")
                assert state["queue"] == before and state["instance"] == instance
                assert bus("get-property", NAME, "/org/mpris/MediaPlayer2",
                           "org.mpris.MediaPlayer2.Player", "PlaybackStatus") == 's "Stopped"'
                stop_bus()
                receive_until(lambda value: value["mpris"]["state"] == "unavailable")
                hung_bus(); send("retry_desktop")
                receive_until(lambda value: value["mpris"]["state"] == "connecting")
                accepted, _ = listener.accept()
                started = time.monotonic(); send("quit")
                process.wait(timeout=5)
                assert process.returncode == 0 and time.monotonic() - started < 5
                close_hung_bus()
                print("Real private-bus loss/restart recovered on the same instance; Quit cancelled a pending handshake; no credentials, Plex, audio or desktop services", flush=True)
            finally:
                if process and process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait(timeout=5)
                if daemon:
                    daemon.terminate(); daemon.wait(timeout=5); daemon.stdout.close()
                if accepted:
                    accepted.close()
                if listener:
                    listener.close()
                if stream:
                    stream.close()
                if connection:
                    connection.close()


if __name__ == "__main__":
    check()
