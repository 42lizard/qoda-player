#!/usr/bin/env python3
"""Build and check a disposable package with no host user-manager operations."""
import importlib.util
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
        # Only filesystem installation is exercised: never reload/start host units.
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
                command.write_text("#!/usr/bin/python3\nimport sys\nassert sys.argv[1:4] == ['--user', 'show', '--property=ActiveState']\nprint('inactive')\n")
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
                installed_data = sorted(path.relative_to(base / "data") for path in (base / "data").rglob("*") if path.is_file())
                if not installed_data:
                    raise AssertionError("Graceful stop did not retain session data")
                installer.install(prefix, config)
                if current.resolve().name != release:
                    raise AssertionError("Identical locked rebuild changed the release identity")
                installer.remove(prefix, config)
                if not all((base / "data" / path).exists() for path in installed_data):
                    raise AssertionError("Uninstall removed private session data")
                print("Isolated release install/rebuild, native syntax, installed launcher, paused backend, active-switch refusal, graceful stop and retained-data uninstall passed")
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
