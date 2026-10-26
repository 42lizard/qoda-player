#!/usr/bin/env python3
"""Installed Qoda entry point. Services own processes; activation never plays."""
import argparse
from contextlib import contextmanager
import fcntl
import json
import math
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import time

ROOT = Path(__file__).resolve().parent
UNITS = ("qoda-backend.service", "qoda-ui.service")
ACTIONS = {"search": "showSearch", "queue": "showQueue", "show": "showFull", "toggle": "toggle", "compact": "showCompact", "hide": "hide"}


@contextmanager
def control_lock():
    if not os.environ.get("XDG_RUNTIME_DIR"):
        raise ValueError("XDG_RUNTIME_DIR is required; launch from the logged-in desktop")
    runtime = Path(os.environ["XDG_RUNTIME_DIR"])
    if not runtime.is_absolute():
        raise ValueError("XDG_RUNTIME_DIR must be absolute")
    info = runtime.lstat()
    if info.st_uid != os.getuid() or not stat.S_ISDIR(info.st_mode):
        raise ValueError("A user-owned XDG_RUNTIME_DIR is required")
    directory = runtime / "qoda"
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError("Qoda runtime directory must be private and user-owned")
    fd = os.open(directory / "control.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise ValueError("Unsafe Qoda control lock")
        deadline = time.monotonic() + 10
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError("Another Qoda launch/install is still running; retry")
                time.sleep(.05)
        yield
    finally:
        os.close(fd)


def manager(*args, check=True):
    return subprocess.run(["systemctl", "--user", *args], capture_output=True, text=True, timeout=10, check=check)


def active(unit):
    result = manager("show", "--property=ActiveState", "--value", unit)
    return result.stdout.strip() in {"active", "activating", "reloading", "deactivating"}


def backend_exit():
    result = manager("show", "--property=LoadState", "--property=Result", "--property=ExecMainCode", "--property=ExecMainStatus", UNITS[0])
    fields = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
    code, status = fields.get("ExecMainCode"), fields.get("ExecMainStatus")
    if fields.get("LoadState") == "not-found":
        outcome = "not_installed"
    elif code == "1" and status == "74":
        outcome = "checkpoint_failed"
    elif code == "1" and status == "75":
        outcome = "backup_warning"
    elif code == "1" and status == "76":
        outcome = "runtime_cleanup_failed"
    elif code == "1" and status in {"78", "127"}:
        outcome = "configuration_failed"
    elif fields.get("Result") == "success" and status == "0" and code in {"0", "1"}:
        outcome = "not_started" if code == "0" else "clean"
    else:
        outcome = "unconfirmed"
    return {"outcome": outcome, "result": fields.get("Result"), "code": code, "status": status}


def verify_shutdown():
    outcome = backend_exit()["outcome"]
    if outcome == "checkpoint_failed":
        raise ValueError("Qoda stopped, but the final session checkpoint failed. Recent changes may be unsaved; inspect journalctl --user -u qoda-backend before restarting")
    if outcome == "backup_warning":
        raise ValueError("Qoda stopped and the session was saved, but its backup update failed. Inspect journalctl --user -u qoda-backend")
    if outcome == "runtime_cleanup_failed":
        raise ValueError("Qoda stopped, but runtime socket cleanup failed; inspect journalctl --user -u qoda-backend before restarting")
    if outcome == "configuration_failed":
        raise ValueError("Qoda startup configuration or dependencies require repair; inspect journalctl --user -u qoda-backend")
    if outcome not in {"clean", "not_started"}:
        raise ValueError("Qoda stop/checkpoint outcome is unconfirmed; inspect journalctl --user -u qoda-backend")


def snapshot():
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(.5)
        connection.connect(str(Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/backend.sock"))
        _, uid, _ = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        if uid != os.getuid():
            raise ValueError("Backend peer belongs to another user")
        connection.sendall(b'{"op":"hello","protocol":1}\n')
        with connection.makefile("rb") as stream:
            line = stream.readline(16 * 1024 * 1024 + 1)
        if len(line) > 16 * 1024 * 1024 or not line.endswith(b"\n"):
            raise ValueError("Backend greeting exceeds the protocol limit")
        value = json.loads(line)
        if not isinstance(value, dict) or type(value.get("protocol")) is not int or value["protocol"] != 1 or not isinstance(value.get("instance"), str) or not 0 < len(value["instance"]) <= 128:
            raise ValueError("Backend protocol mismatch; stop Qoda before changing releases")
        return value


def ipc(action):
    return subprocess.run(["quickshell", "ipc", "--path", str(ROOT.parent.parent / "current/ui"), "call", "--", "control", ACTIONS[action]],
                          capture_output=True, timeout=1, check=False).returncode == 0


def verify_session():
    if not os.environ.get("WAYLAND_DISPLAY"):
        raise ValueError("Launch Qoda from an active Wayland desktop session")
    values = dict(line.split("=", 1) for line in manager("show-environment").stdout.splitlines() if "=" in line)
    for name in ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]:
        if os.environ.get(name) and values.get(name) != os.environ[name]:
            raise ValueError("User service environment differs from this desktop; run dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS in the desktop session")


def owner_path():
    return Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/ui-owner.json"


def read_owner():
    fd = os.open(owner_path(), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_nlink != 1 or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise ValueError("UI owner record is unsafe; preserved")
        content = stream.read(4097)
    if len(content) > 4096:
        raise ValueError("UI owner record is oversized; preserved")
    value = json.loads(content)
    if not isinstance(value, dict) or set(value) != {"format", "release", "instance", "expires"} or type(value["format"]) is not int or value["format"] != 1 or type(value["expires"]) not in {int, float} or not math.isfinite(value["expires"]) or not all(isinstance(value[key], str) and 0 < len(value[key]) <= 128 for key in ["release", "instance"]):
        raise ValueError("UI owner record is unsupported or corrupt; preserved")
    return value


def write_owner(instance, expires):
    path = owner_path()
    if path.exists() or path.is_symlink():
        read_owner()
    content = json.dumps({"format": 1, "release": ROOT.name, "instance": instance, "expires": expires}).encode()
    import tempfile
    fd, temporary = tempfile.mkstemp(prefix=".ui-owner-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def matching_backend(instance):
    if snapshot()["instance"] != instance:
        raise ValueError("Backend instance changed; cancelled stale UI activation")


def check_deadline(expires):
    if expires is not None and (not math.isfinite(expires) or time.monotonic() * 1000 >= expires):
        raise ValueError("UI activation expired; no new UI was started")


def run_ui():
    record = read_owner()
    check_deadline(record["expires"])
    if record["release"] != ROOT.name:
        raise ValueError("UI owner record belongs to a different release")
    matching_backend(record["instance"])
    check_deadline(record["expires"])
    child = subprocess.Popen(["quickshell", "--path", str(ROOT.parent.parent / "current/ui"), "--no-duplicate"])
    try:
        while child.poll() is None:
            matching_backend(record["instance"])
            time.sleep(.25)
        if child.returncode:
            raise ValueError("Quickshell exited; inspect journalctl --user -u qoda-ui")
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=3)


def launch(action, expected_instance=None, expires=None):
    with control_lock():
        check_deadline(expires)
        current = ROOT.parent.parent / "current"
        if current.is_symlink() and current.resolve() != ROOT:
            raise ValueError("Release changed while waiting; retry the launcher")
        if action == "status":
            states = {unit: active(unit) for unit in UNITS}
            print(json.dumps({"release": ROOT.name, "services": states, "last_backend_exit": backend_exit()}))
            return
        if action == "quit":
            # No socket activation exists; a deliberate stop cannot resurrect Qoda.
            stopped = manager("stop", *reversed(UNITS), check=False)
            if any(active(unit) for unit in UNITS):
                raise ValueError("Qoda is still stopping; inspect journalctl --user -u qoda-backend")
            verify_shutdown()
            if stopped.returncode != 0:
                raise ValueError("Service stop failed; inspect journalctl --user -u qoda-backend -u qoda-ui")
            if (Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/backend.sock").exists():
                raise ValueError("An unmanaged backend remains; stop the development player separately")
            return
        running = active(UNITS[0])
        if action == "hide":
            if running and active(UNITS[1]) and not ipc(action):
                raise ValueError("UI did not respond; inspect journalctl --user -u qoda-ui")
            return
        for pattern in [".qoda-stage-*/recovery-needed", ".qoda-remove-*/recovery-needed"]:
            if any(ROOT.parent.parent.parent.glob(pattern)):
                raise ValueError("An interrupted installation needs recovery; inspect the prefix lib directory before launching")
        if expected_instance:
            if not running:
                raise ValueError("Expected backend is no longer active; activation cancelled")
            matching_backend(expected_instance)
        # A development backend must not be silently adopted by installed units.
        if not running:
            try:
                snapshot()
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            else:
                raise ValueError("An unmanaged Qoda backend is running; quit it before using this installation")
        verify_session()
        if not expected_instance:
            manager("start", UNITS[0])
        deadline = min(time.monotonic() + 10, expires / 1000) if expires is not None else time.monotonic() + 10
        while True:
            try:
                check_deadline(expires)
                ready = snapshot()
                if expected_instance and ready["instance"] != expected_instance:
                    raise ValueError("Backend instance changed; activation cancelled")
                break
            except (OSError, TimeoutError):
                if time.monotonic() >= deadline or not active(UNITS[0]):
                    raise ValueError("Backend not ready; inspect journalctl --user -u qoda-backend") from None
                time.sleep(.1)
        ui_running = active(UNITS[1])
        if not ui_running:
            check_deadline(expires)
            write_owner(ready["instance"], deadline * 1000)
        if expected_instance:
            matching_backend(expected_instance)
        check_deadline(expires)
        manager("start", UNITS[1])
        requested = "show" if action == "toggle" and not ui_running else action
        while True:
            check_deadline(expires)
            if expected_instance:
                matching_backend(expected_instance)
            if ipc(requested):
                break
            if time.monotonic() >= deadline or not active(UNITS[1]):
                raise ValueError("UI not ready; inspect journalctl --user -u qoda-ui")
            time.sleep(.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=[*ACTIONS, "status", "quit", "run-ui"], nargs="?", default="show")
    parser.add_argument("--expected-instance", help=argparse.SUPPRESS)
    parser.add_argument("--deadline", type=float, help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if args.action == "run-ui":
            run_ui()
        else:
            launch(args.action, args.expected_instance, args.deadline)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
