#!/usr/bin/env python3
"""Exercise production QML connection adapters with a private Unix peer, no Plex/audio."""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def check(widget):
    with tempfile.TemporaryDirectory(prefix="qoda-connection-") as directory:
        base = Path(directory)
        shutil.copytree(ROOT / "ui", base / "ui")
        runtime = base / "runtime"
        (runtime / "qoda").mkdir(parents=True, mode=0o700)
        # The production shell uses BackendConnection with role shell; WidgetClient
        # supplies the widget's operation policy. Neither fixture reimplements it.
        kind = "WidgetClient" if widget else "BackendConnection"
        role = "" if widget else 'role: "shell"'
        (base / "shell.qml").write_text('''import QtQuick
import Quickshell
import Quickshell.Io
import "ui" as Qoda
ShellRoot {
 Qoda.KIND { id: client; ROLE }
 IpcHandler { target: "probe"
  function status(): string { return JSON.stringify({ready:client.connected, snapshot:client.snapshot}); }
  function send(op:string): void { client.send(op, {value:42}, true); }
 }
}
'''.replace("KIND", kind).replace("ROLE", role))
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(runtime / "qoda/backend.sock")); server.listen(); server.settimeout(5)
        process = None
        connection = None
        with (base / "log").open("w+") as log:
            def ipc(method, *args):
                r = subprocess.run(["quickshell", "ipc", "--path", str(base), "call", "--", "probe", method, *args], env=env, capture_output=True, text=True, timeout=5)
                if r.returncode:
                    raise RuntimeError(r.stderr)
                return r.stdout.strip()

            def status():
                return json.loads(ipc("status"))

            def until(predicate):
                for _ in range(100):
                    if process.poll() is not None:
                        raise AssertionError("QML exited")
                    try:
                        if predicate(status()):
                            return
                    except RuntimeError:
                        pass
                    time.sleep(.03)
                raise AssertionError("Connection did not settle")

            def accept():
                peer, _ = server.accept(); peer.settimeout(2)
                hello = peer.makefile("rb", buffering=0).readline()
                assert json.loads(hello) == ({"op":"hello", "protocol":1} if widget else {"op":"hello", "protocol":1, "role":"shell"})
                return peer

            def frame(instance="A", revision="9007199254740993"):
                return {"protocol":1, "instance":instance, "revision":revision, "account_epoch":"9007199254740995", "queue":{"session":instance+"-queue", "revision":"18446744073709551614"}}

            def push(value):
                connection.sendall((json.dumps(value)+"\n").encode())

            try:
                process = subprocess.Popen(["quickshell", "--path", str(base), "--no-duplicate"], env=env, stdout=log, stderr=log)
                connection = accept()
                until(lambda s:not s["ready"])
                ipc("send", "next")
                connection.settimeout(.1)
                try:
                    assert not connection.recv(1), "Command before validated snapshot"
                except TimeoutError:
                    pass
                connection.settimeout(2)
                data = (json.dumps(frame())+"\n").encode()
                connection.sendall(data[:19]); time.sleep(.05)
                assert not status()["ready"]
                connection.sendall(data[19:])
                until(lambda s:s["ready"])
                push(frame(revision="9007199254740992"))
                push(frame(revision="9007199254740994"))
                until(lambda s:s["snapshot"].get("revision")=="9007199254740994")
                ipc("send", "next")
                first = json.loads(connection.makefile("rb", buffering=0).readline())
                assert first["instance"]=="A" and first["account_epoch"]=="9007199254740995" and first["queue_revision"]=="18446744073709551614" and first["value"]==42
                push(frame("B", "0")); until(lambda s:s["snapshot"].get("instance")=="B")
                ipc("send", "next")
                second = json.loads(connection.makefile("rb", buffering=0).readline())
                assert second["instance"]=="B" and second["queue_session"]=="B-queue" and second["id"]!=first["id"]
                if widget:
                    ipc("send", "auth_start")
                    connection.settimeout(.1)
                    try:
                        assert not connection.recv(1), "Widget accepted forbidden operation"
                    except TimeoutError:
                        pass
                    connection.settimeout(2)
                for invalid in [{**frame(), "protocol":2}, {**frame(), "revision":"01"}, {**frame(), "account_epoch":"18446744073709551616"}, {**frame(), "instance":7}, None]:
                    push(invalid)
                    assert connection.recv(1)==b"", "Invalid snapshot did not disconnect"
                    connection.close(); connection=None
                    until(lambda s:not s["ready"])
                    connection=accept()
                    push(frame("C", "0")); until(lambda s:s["ready"] and s["snapshot"].get("instance")=="C")
                connection.shutdown(socket.SHUT_RDWR); connection.close(); connection=None
                until(lambda s:not s["ready"])
                ipc("send", "resume")
                connection=accept(); push(frame("D", "0"))
                until(lambda s:s["ready"])
                ipc("send", "resume")
                last=json.loads(connection.makefile("rb", buffering=0).readline())
                assert last["instance"]=="D" and last["queue_session"]=="D-queue" and last["id"]!=second["id"]
            finally:
                if connection:
                    connection.close()
                server.close()
                if process:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait(timeout=5)
                log.flush(); log.seek(0)
                errors=log.read()
                if any(word in errors for word in ["Failed to load", "ReferenceError", "TypeError", "Binding loop"]):
                    raise AssertionError(errors)
        print(kind+": framing, validation, lossless fences, replacement and reconnect passed")


if __name__ == "__main__":
    check(False)
    check(True)
