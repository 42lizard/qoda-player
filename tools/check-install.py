#!/usr/bin/env python3
"""Build and check a disposable package with no host user-manager operations."""
import importlib.util
import hashlib
import shutil
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "tools" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check():
    installer = load("install_check", "install.py")
    launcher = load("launch_check", "launch.py")
    with tempfile.TemporaryDirectory(prefix="qoda-installed-check-") as temporary:
        base = Path(temporary)
        runtime = base / "runtime"
        runtime.mkdir(mode=0o700)
        config, prefix = base / "config", base / "prefix with spaces"
        environment = {"XDG_RUNTIME_DIR": str(runtime), "XDG_CONFIG_HOME": str(config),
                       "XDG_DATA_HOME": str(base / "data"), "XDG_STATE_HOME": str(base / "state"),
                       "XDG_CACHE_HOME": str(base / "cache"),
                       "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(base / "absent-test-bus")}
        # All manager operations are replaced; host units are never reloaded or started.
        manager = mock.Mock()
        manager.UNITS = launcher.UNITS
        manager.control_lock = launcher.control_lock
        manager.active.return_value = False
        def reload_only(*args):
            if args != ("daemon-reload",):
                raise AssertionError("Unexpected service operation in isolated check")
        manager.manager.side_effect = reload_only
        process = None
        with mock.patch.dict(os.environ, environment), mock.patch.object(installer, "launcher_module", return_value=manager):
            try:
                current = installer.install(prefix, config)
                release = current.resolve().name
                subprocess.run(["desktop-file-validate", str(prefix / "share/applications/qoda-player.desktop")], check=True)
                subprocess.run(["systemd-analyze", "--user", "verify", *[str(config / name) for name in installer.EXTERNAL["config"]]], check=True)
                # Execute the installed wrapper with a test-only manager command.
                fakebin = base / "fakebin"
                fakebin.mkdir()
                command = fakebin / "systemctl"
                command.write_text("#!/usr/bin/python3\nimport sys\nassert sys.argv[1:3] == ['--user', 'show']\nprint('inactive' if '--property=ActiveState' in sys.argv else 'LoadState=loaded\\nResult=success\\nExecMainCode=0\\nExecMainStatus=0')\n")
                command.chmod(0o755)
                result = subprocess.run([str(prefix / "bin/qoda"), "status"], capture_output=True, text=True, check=True,
                                        env=dict(os.environ, PATH=str(fakebin) + os.pathsep + os.environ["PATH"]))
                if json.loads(result.stdout)["release"] != release:
                    raise AssertionError("Installed launcher resolved a different release")
                with (base / "backend.log").open("wb") as log:
                    process = subprocess.Popen([str(current / "qoda-backend")], stdin=subprocess.DEVNULL, stdout=log, stderr=log)
                    deadline = time.monotonic() + 10
                    while True:
                        try:
                            state = launcher.snapshot()
                            break
                        except OSError:
                            if process.poll() is not None or time.monotonic() >= deadline:
                                raise AssertionError("Isolated installed backend did not become ready") from None
                            time.sleep(.1)
                    if state["intent"] != "paused" or state.get("selection") is not None or state["queue"]["entries"]:
                        raise AssertionError("Fresh installed backend was not empty and paused")
                    try:
                        installer.install(prefix, config)
                    except ValueError as error:
                        if "socket remains" not in str(error):
                            raise
                    else:
                        raise AssertionError("Installer changed a running backend release")
                    process.send_signal(signal.SIGINT)
                    if process.wait(timeout=15) != 0:
                        raise AssertionError("Installed backend failed its graceful stop")
                installed_data = sorted(path.relative_to(base) for directory in ["data", "state", "cache"]
                                        for path in (base / directory).rglob("*") if path.is_file())
                if not (base / "state/qoda/session.sqlite3").is_file():
                    raise AssertionError("Graceful stop did not retain session state")
                installer.install(prefix, config)
                if current.resolve().name != release:
                    raise AssertionError("Identical locked rebuild changed the release identity")
                # Exercise checkpointed switching using real packaged processes,
                # while the adapter owns no host systemd or UI process.
                last_exit=0
                def native_manager(*args,check=True):
                    nonlocal process,last_exit
                    if args==("daemon-reload",):
                        return subprocess.CompletedProcess(args,0,stdout="")
                    if args[0]=="show":
                        return subprocess.CompletedProcess(args,0,stdout="FragmentPath="+str(config / "systemd/user" / args[-1])+"\nDropInPaths=\n")
                    if args==("start",launcher.UNITS[0]):
                        with (base / "native-update.log").open("ab") as log:
                            process=subprocess.Popen([str(current / "qoda-backend"),"--ui-launcher",str(prefix / "bin/qoda")],stdin=subprocess.DEVNULL,stdout=log,stderr=log)
                        return subprocess.CompletedProcess(args,0,stdout="")
                    if args[0]=="stop" and all(unit in launcher.UNITS for unit in args[1:]):
                        if launcher.UNITS[0] in args and process and process.poll() is None:
                            process.send_signal(signal.SIGINT);last_exit=process.wait(timeout=15)
                        return subprocess.CompletedProcess(args,last_exit,stdout="")
                    raise AssertionError("Forbidden host/native manager operation: "+repr(args))
                def verify_exit():
                    if last_exit!=0:raise ValueError("Private native backend checkpoint failed")
                manager.manager.side_effect=native_manager
                manager.active.side_effect=lambda unit:unit==launcher.UNITS[0] and process is not None and process.poll() is None
                manager.pause_checkpoint.side_effect=launcher.pause_checkpoint
                manager.verify_shutdown.side_effect=verify_exit
                manager.snapshot.side_effect=launcher.snapshot
                manager.ipc.side_effect=AssertionError("No UI is started by the private manager adapter")
                native_manager("start",launcher.UNITS[0])
                deadline=time.monotonic()+10
                while True:
                    try:before=launcher.snapshot();break
                    except OSError:
                        if time.monotonic()>deadline:raise
                        time.sleep(.05)
                original=current.resolve()
                def candidate(destination):
                    shutil.copytree(original,destination)
                    metadata=json.loads((destination / "release.json").read_text())
                    metadata["qualification"]="private empty-profile release-switch fixture"
                    (destination / "release.json").write_text(json.dumps(metadata,sort_keys=True)+"\n")
                    return "0.1.0-native-fixture-"+hashlib.sha256(json.dumps(installer.inventory(destination),sort_keys=True).encode()).hexdigest()[:16]
                with mock.patch.object(installer,"stage_release",side_effect=candidate):
                    installer.install(prefix,config,checkpointed=True)
                after=launcher.snapshot()
                if current.resolve()==original or after["instance"]==before["instance"] or after["intent"]!="paused":
                    raise AssertionError("Native release update did not restart a distinct paused instance")
                installer.install(prefix,config,rollback_release=release)
                rolled=launcher.snapshot()
                if current.resolve()!=original or rolled["instance"]==after["instance"] or rolled["intent"]!="paused":
                    raise AssertionError("Compatible rollback did not select and restart the original paused release")
                native_manager("stop",launcher.UNITS[0]);verify_exit()
                installer.remove(prefix, config)
                if not all((base / path).exists() for path in installed_data):
                    raise AssertionError("Uninstall removed private session data")
                print("Isolated release install/rebuild, native syntax, installed launcher, checkpointed native update, compatible rollback, paused restart and retained-data uninstall passed")
                print("No Plex, credentials, music or host service/configuration changes; owned test processes stopped")
            finally:
                if process and process.poll() is None:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == "__main__":
    check()
