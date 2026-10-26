#!/usr/bin/env python3
"""Qualify coordinator MPRIS activation on a private bus, without host services."""
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
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
    with tempfile.TemporaryDirectory(prefix="qoda-raise-") as temporary:
        base = Path(temporary)
        environment = dict(os.environ)
        for key, name in [("XDG_RUNTIME_DIR", "runtime"), ("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data")]:
            path = base / name
            path.mkdir(mode=0o700)
            environment[key] = str(path)
        address = "unix:path=" + str(base / "bus")
        environment["DBUS_SESSION_BUS_ADDRESS"] = address
        config = base / "bus.conf"
        config.write_text(f"<busconfig><type>session</type><listen>{address}</listen><auth>EXTERNAL</auth><policy context='default'><allow user='{os.getuid()}'/><allow own='*'/><allow send_destination='*'/><allow receive_sender='*'/></policy></busconfig>")
        launcher = base / "UI launcher"
        launcher.write_text("#!/usr/bin/python3\nimport json,os,sys,time\nfrom pathlib import Path\nbase=Path(__file__).parent\nwith (base/'calls').open('a') as log: log.write(json.dumps(sys.argv[1:])+'\\n')\nif (base/'fail').exists(): raise SystemExit(1)\nif (base/'hold').exists():\n (base/'pid').write_text(str(os.getpid()))\n time.sleep(30)\n")
        launcher.chmod(0o700)
        daemon = process = pending = None
        with (base / "backend.log").open("w+") as log:
            try:
                daemon = subprocess.Popen(["dbus-daemon", "--nofork", "--print-address=1", "--config-file=" + str(config)], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
                assert select.select([daemon.stdout], [], [], 3)[0], "Private bus did not start"
                assert daemon.stdout.readline().strip().startswith(address)
                process = subprocess.Popen([str(target / "debug/qoda-backend"), "--ui-launcher", str(launcher)], env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=log)

                def request(op="hello", **fields):
                    with socket.socket(socket.AF_UNIX) as connection:
                        connection.settimeout(3)
                        connection.connect(str(base / "runtime/qoda/backend.sock"))
                        connection.sendall(b'{"op":"hello","protocol":1}\n')
                        with connection.makefile("rb") as stream:
                            state = json.loads(stream.readline(16 * 1024 * 1024))
                            if op != "hello":
                                frame = {"op": op, "instance": state["instance"], **fields}
                                if op == "set_repeat":
                                    frame.update(queue_session=state["queue"]["session"], queue_revision=state["queue"]["revision"])
                                connection.sendall((json.dumps(frame) + "\n").encode())
                                deadline = time.monotonic() + 5
                                while True:
                                    line = stream.readline(16 * 1024 * 1024)
                                    if not line and op == "quit":
                                        return state
                                    assert line, "Backend closed before confirming the operation"
                                    state = json.loads(line)
                                    if op == "quit" and state.get("shutdown") or op == "set_repeat" and state["queue"]["repeat"] == fields["mode"]:
                                        return state
                                    assert time.monotonic() < deadline
                            return state

                def until(predicate):
                    deadline = time.monotonic() + 8
                    while time.monotonic() < deadline:
                        assert process.poll() is None, "Backend exited"
                        try:
                            state = request()
                            if predicate(state):
                                return state
                        except OSError:
                            pass
                        time.sleep(.05)
                    raise AssertionError("Activation fixture did not settle")

                def bus(*arguments, check=True):
                    result = subprocess.run(["busctl", "--address=" + address, "--timeout=13", *arguments], capture_output=True, text=True, timeout=15)
                    if check:
                        assert result.returncode == 0, result.stderr
                    return result

                state = until(lambda value: value.get("mpris", {}).get("state") == "available")
                assert state["ui"]["can_raise"] and state["intent"] == "paused" and not state["queue"]["entries"]
                instance = state["instance"]
                assert bus("get-property", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2", "CanRaise").stdout.strip() == "b true"
                bus("call", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2", "Raise")
                args = json.loads((base / "calls").read_text().splitlines()[-1])
                assert args[:4] == ["show", "--expected-instance", instance, "--deadline"]
                assert float(args[4]) <= time.monotonic() * 1000 + 10000
                after = request()
                assert after["intent"] == "paused" and after["queue"] == state["queue"]
                assert after["ui"]["raise_token"] is None
                (base / "fail").touch()
                assert bus("call", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2", "Raise", check=False).returncode != 0
                until(lambda value: bool(value["ui"].get("launch_error")))
                (base / "fail").unlink()
                (base / "hold").touch()
                pending = subprocess.Popen(["busctl", "--address=" + address, "--timeout=13", "call", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2", "Raise"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                deadline = time.monotonic() + 5
                while not (base / "pid").exists():
                    assert time.monotonic() < deadline
                    time.sleep(.02)
                helper = int((base / "pid").read_text())
                started = time.monotonic()
                bus("set-property", NAME, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", "Shuffle", "b", "true")
                assert request("set_repeat", mode="all")["intent"] == "paused"
                assert time.monotonic() - started < 2, "Pending launch blocked the coordinator"
                request("quit")
                assert process.wait(timeout=10) == 0
                pending.communicate(timeout=5)
                assert pending.returncode != 0
                deadline = time.monotonic() + 5
                while Path(f"/proc/{helper}").exists():
                    assert time.monotonic() < deadline, "Activation helper survived Quit"
                    time.sleep(.02)
                print("Private-bus absent-UI Raise, instance/deadline argv, failure reporting, responsive coordinator and Quit cancellation passed")
                print("No host services, UI, Plex, credentials or media; owned processes stopped")
            finally:
                if pending and pending.poll() is None:
                    pending.terminate()
                    pending.communicate(timeout=5)
                if process and process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                if daemon and daemon.poll() is None:
                    daemon.terminate()
                    daemon.wait(timeout=5)
                if daemon and daemon.stdout:
                    daemon.stdout.close()
                if pending:
                    if pending.stdout: pending.stdout.close()
                    if pending.stderr: pending.stderr.close()


if __name__ == "__main__":
    check()
