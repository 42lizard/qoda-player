import importlib.util
import json
import multiprocessing
import os
import shutil
import socket
import threading
import time
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load("qoda_install", "tools/install.py")
launcher = load("qoda_launch", "tools/launch.py")


class Installation(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="qoda-package-")
        self.base = Path(self.temporary.name)
        self.prefix = self.base / 'prefix with spaces % $ ` "'
        self.config = self.base / "config"
        self.runtime = self.base / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.environment = mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.runtime)})
        self.environment.start()
        self.service = mock.Mock()
        self.service.active.return_value = False
        self.service.UNITS = launcher.UNITS
        self.service.control_lock = launcher.control_lock
        self.module = mock.patch.object(installer, "launcher_module", return_value=self.service)
        self.module.start()
        self.generation = 1
        self.build = mock.patch.object(installer, "stage_release", side_effect=self.stage)
        self.build_mock = self.build.start()

    def tearDown(self):
        self.build.stop()
        self.module.stop()
        self.environment.stop()
        self.temporary.cleanup()

    def stage(self, path):
        path.mkdir()
        (path / "qoda-backend").write_text(f"fictional release {self.generation}\n")
        (path / "qoda-backend").chmod(0o755)
        (path / "ui").mkdir()
        (path / "ui/shell.qml").write_text("// fictional UI\n")
        return f"0.1.0-fixture{self.generation}"

    def install(self):
        return installer.install(self.prefix, self.config)

    def test_repeat_upgrade_remove_retains_user_data_and_prior_releases(self):
        unrelated = self.config / "qoda/session-preference"
        unrelated.parent.mkdir(parents=True)
        unrelated.write_text("retained")
        current = self.install()
        self.assertEqual(current.readlink().as_posix(), "releases/0.1.0-fixture1")
        self.install()
        self.generation = 2
        self.install()
        self.assertEqual(current.readlink().as_posix(), "releases/0.1.0-fixture2")
        self.assertTrue((current.parent / "releases/0.1.0-fixture1/qoda-backend").exists())
        installer.remove(self.prefix, self.config)
        self.assertFalse(current.parent.exists())
        for domain, names in installer.EXTERNAL.items():
            anchor = self.prefix if domain == "prefix" else self.config
            for name in names:
                self.assertFalse((anchor / name).exists())
        self.assertEqual(unrelated.read_text(), "retained")
        for call in self.service.manager.call_args_list:
            self.assertEqual(call.args, ("daemon-reload",))

    def test_unowned_conflict_refuses_before_build(self):
        path = self.prefix / "bin/qoda"
        path.parent.mkdir(parents=True)
        path.write_text("user command")
        with self.assertRaises(ValueError):
            self.install()
        self.build_mock.assert_not_called()
        self.assertEqual(path.read_text(), "user command")

    def test_modified_files_extra_payload_modes_and_newer_manifest_are_preserved(self):
        for change in ["content", "mode", "extra", "directory", "schema", "manifest-list", "pointer"]:
            with self.subTest(change=change):
                current = self.install()
                root = current.parent
                if change == "content":
                    (self.prefix / "bin/qoda").write_text("edited")
                elif change == "mode":
                    (self.prefix / "bin/qoda").chmod(0o600)
                elif change == "extra":
                    (root / "personal.txt").write_text("retained")
                elif change == "directory":
                    (root / "user-directory").mkdir()
                elif change == "manifest-list":
                    (root / installer.RECORD).write_text("[]")
                elif change == "schema":
                    path = root / installer.RECORD
                    value = json.loads(path.read_text())
                    value["schema"] = 2
                    path.write_text(json.dumps(value))
                else:
                    current.unlink()
                    current.symlink_to("releases/unknown")
                before = installer.inventory(root)
                before_record = (root / installer.RECORD).read_bytes()
                with self.assertRaises((ValueError, OSError)):
                    self.install()
                with self.assertRaises((ValueError, OSError)):
                    installer.remove(self.prefix, self.config)
                self.assertEqual(installer.inventory(root), before)
                self.assertEqual((root / installer.RECORD).read_bytes(), before_record)
                # This is an isolated temporary profile, never the real desktop.
                shutil.rmtree(root)
                shutil.rmtree(self.prefix / "bin")
                shutil.rmtree(self.prefix / "share")
                shutil.rmtree(self.config / "systemd")

    def test_symlinked_ancestor_is_not_followed(self):
        outside = self.base / "outside"
        outside.mkdir()
        self.prefix.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.install()
        self.assertEqual(list(outside.iterdir()), [])

    def test_failed_external_write_rolls_back_complete_previous_install(self):
        current = self.install()
        old_inventory = installer.inventory(current.parent)
        old_launcher = (self.prefix / "bin/qoda").read_bytes()
        self.generation = 2
        atomic = installer.atomic_file
        calls = 0
        def fail_once(*args):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("injected disk error")
            return atomic(*args)
        with mock.patch.object(installer, "atomic_file", side_effect=fail_once):
            with self.assertRaises(OSError):
                self.install()
        self.assertEqual(installer.inventory(current.parent), old_inventory)
        self.assertEqual((self.prefix / "bin/qoda").read_bytes(), old_launcher)
        installer.owned(current.parent, {"prefix": self.prefix, "config": self.config})

    def test_failed_rollback_retains_previous_payload_and_integration_copies(self):
        current = self.install()
        original = (self.prefix / "bin/qoda").read_bytes()
        self.generation = 2
        atomic = installer.atomic_file
        calls = 0
        def disk_failure(*args):
            nonlocal calls
            calls += 1
            if calls >= 2:
                raise OSError("persistent disk failure")
            return atomic(*args)
        with mock.patch.object(installer, "atomic_file", side_effect=disk_failure):
            with self.assertRaises(OSError):
                self.install()
        retained = list((self.prefix / "lib").glob(".qoda-stage-*"))
        self.assertEqual(len(retained), 1)
        self.assertTrue((retained[0] / "recovery-needed").is_file())
        self.assertTrue((retained[0] / "previous/releases/0.1.0-fixture1/qoda-backend").is_file())
        self.assertEqual((retained[0] / "external/prefix/bin/qoda").read_bytes(), original)

    def test_interrupted_switch_retains_recovery_and_next_invocation_reports_it(self):
        current = self.install()
        self.generation = 2
        atomic = installer.atomic_file
        def interrupted(*args):
            atomic(*args)
            os._exit(23)
        def child():
            with mock.patch.object(installer, "atomic_file", side_effect=interrupted):
                self.install()
        process = multiprocessing.get_context("fork").Process(target=child)
        process.start()
        process.join(timeout=10)
        try:
            self.assertEqual(process.exitcode, 23)
            retained = list((self.prefix / "lib").glob(".qoda-stage-*"))
            self.assertEqual(len(retained), 1)
            self.assertTrue((retained[0] / "previous/releases/0.1.0-fixture1/qoda-backend").exists())
            with self.assertRaisesRegex(ValueError, "Unfinished Qoda transaction"):
                self.install()
            self.assertTrue(current.exists())
        finally:
            if process.is_alive():
                process.kill()
                process.join()

    def test_failed_reload_rolls_back_install_and_removal(self):
        current = self.install()
        before = installer.inventory(current.parent)
        self.generation = 2
        self.service.manager.side_effect = subprocess.CalledProcessError(1, "systemctl")
        with self.assertRaises(subprocess.CalledProcessError):
            self.install()
        self.assertEqual(installer.inventory(current.parent), before)
        with self.assertRaises(subprocess.CalledProcessError):
            installer.remove(self.prefix, self.config)
        self.assertEqual(installer.inventory(current.parent), before)
        installer.owned(current.parent, {"prefix": self.prefix, "config": self.config})

    def test_running_player_blocks_switch_without_stopping(self):
        current = self.install()
        before = installer.inventory(current.parent)
        self.service.active.return_value = True
        self.generation = 2
        with self.assertRaisesRegex(ValueError, "Quit Qoda"):
            self.install()
        with self.assertRaisesRegex(ValueError, "Quit Qoda"):
            installer.remove(self.prefix, self.config)
        self.assertEqual(installer.inventory(current.parent), before)
        self.assertFalse(any("stop" in call.args for call in self.service.manager.call_args_list))

    def test_desktop_and_units_have_no_activation_and_escape_paths(self):
        self.install()
        unit = (self.config / "systemd/user/qoda-backend.service").read_text()
        self.assertIn("KillSignal=SIGINT", unit)
        self.assertIn("Restart=on-failure", unit)
        self.assertIn("StartLimitBurst=3", unit)
        self.assertIn("Requisite=graphical-session.target", unit)
        self.assertNotIn("[Install]", unit)
        self.assertIn("%%", unit)
        desktop = (self.prefix / "share/applications/qoda-player.desktop").read_text()
        self.assertIn("Actions=Toggle;Compact;Quit;", desktop)
        self.assertIn("%%", desktop)
        self.assertIn("\\\\$", desktop)
        self.assertFalse((self.config / "autostart").exists())
    @unittest.skipUnless(shutil.which("gio"), "GIO desktop launcher unavailable")
    def test_desktop_exec_reaches_exact_launcher_under_special_character_prefix(self):
        self.install()
        result = self.base / "argv.json"
        (self.prefix / "bin/qoda").write_text("#!/usr/bin/python3\nimport json,sys\nfrom pathlib import Path\nPath(" + repr(str(result)) + ").write_text(json.dumps(sys.argv[1:]))\n")
        subprocess.run(["gio", "launch", str(self.prefix / "share/applications/qoda-player.desktop")], check=True, capture_output=True, timeout=5,
                       env=dict(os.environ, DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(self.base / "absent-bus")))
        deadline = time.monotonic() + 3
        while not result.exists() and time.monotonic() < deadline:
            time.sleep(.05)
        self.assertEqual(json.loads(result.read_text()), ["show"])

    @unittest.skipUnless(all(shutil.which(name) for name in ["quickshell", "systemd-analyze", "desktop-file-validate"]), "full Quickshell runtime and native validators required")
    def test_native_desktop_and_unit_parsers_accept_generated_special_paths(self):
        self.install()
        subprocess.run(["desktop-file-validate", str(self.prefix / "share/applications/qoda-player.desktop")], check=True, capture_output=True)
        result = subprocess.run(["systemd-analyze", "--user", "verify", *[str(self.config / name) for name in installer.EXTERNAL["config"]]], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)


class ReleaseBuild(unittest.TestCase):
    def test_locked_remapped_build_is_stable_and_refuses_personal_paths(self):
        with tempfile.TemporaryDirectory(prefix="qoda-build-fixture-") as temporary:
            base = Path(temporary)
            source, target = base / "source", base / "target"
            (source / "ui").mkdir(parents=True)
            (source / "tools").mkdir()
            (source / "ui/shell.qml").write_text("// fictional UI\n")
            (source / "tools/launch.py").write_text("# fictional entry point\n")
            (source / "LICENSE").write_text("MIT\n")
            (source / "Cargo.toml").write_text('[workspace.package]\nversion="0.1.0"\n')
            (target / "release").mkdir(parents=True)
            binary = target / "release/qoda-backend"
            binary.write_bytes(b"fictional path-free binary")
            def command(args, **kwargs):
                return subprocess.CompletedProcess(args, 0, stdout="rustc 1.99.1 (fixture)\n")
            with mock.patch.object(installer, "ROOT", source), mock.patch.object(installer, "preflight"), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target), "RUSTFLAGS": "-C opt-level=2", "CARGO_ENCODED_RUSTFLAGS": ""}), mock.patch.object(installer.subprocess, "run", side_effect=command) as run:
                first = installer.stage_release(base / "release-one")
                second = installer.stage_release(base / "release-two")
                self.assertEqual(first, second)
                build = run.call_args_list[0]
                self.assertIn("--locked", build.args[0])
                flags = build.kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
                self.assertEqual(flags[:2], ["-C", "opt-level=2"])
                self.assertIn(f"--remap-path-prefix={source}=/build/qoda-player", flags)
                binary.write_bytes(str(Path.home()).encode())
                with self.assertRaisesRegex(ValueError, "personal build paths"):
                    installer.stage_release(base / "rejected-release")



class Launch(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="qoda-launch-")
        self.runtime = Path(self.temporary.name)
        self.environment = mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.runtime)})
        self.environment.start()
        self.session_patch = mock.patch.object(launcher, "verify_session")
        self.session_patch.start()

    def tearDown(self):
        self.session_patch.stop()
        self.environment.stop()
        self.temporary.cleanup()

    def test_hide_and_status_never_start_or_probe_a_backend(self):
        for action in ["hide", "status"]:
            with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot") as snapshot:
                launcher.launch(action)
                manager.assert_not_called()
                snapshot.assert_not_called()

    def test_first_toggle_shows_and_existing_toggle_toggles(self):
        for ui_running in [False, True]:
            with mock.patch.object(launcher, "active", side_effect=lambda unit: unit == launcher.UNITS[0] or ui_running), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot", return_value={"protocol": 1}), mock.patch.object(launcher, "ipc", return_value=True) as ipc:
                launcher.launch("toggle")
                ipc.assert_called_once_with("toggle" if ui_running else "show")
                self.assertEqual([call.args for call in manager.call_args_list], [("start", launcher.UNITS[0]), ("start", launcher.UNITS[1])])

    def test_unmanaged_backend_or_protocol_failure_never_starts_ui(self):
        for error in [None, ValueError("protocol mismatch")]:
            with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot", side_effect=error, return_value={"protocol": 1}):
                with self.assertRaises(ValueError):
                    launcher.launch("show")
                manager.assert_not_called()

    def test_interrupted_install_blocks_start_but_quit_remains_available(self):
        root = self.runtime / "prefix/lib/qoda-player/releases/fixture"
        root.mkdir(parents=True)
        stage = self.runtime / "prefix/lib/.qoda-stage-fixture"
        stage.mkdir()
        (stage / "recovery-needed").touch()
        with mock.patch.object(launcher, "ROOT", root), mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager") as manager:
            with self.assertRaisesRegex(ValueError, "interrupted installation"):
                launcher.launch("show")
            manager.assert_not_called()
            launcher.launch("quit")
            manager.assert_called_once_with("stop", "qoda-ui.service", "qoda-backend.service")

    def test_backend_start_failure_does_not_start_ui(self):
        states = iter([False, False])
        with mock.patch.object(launcher, "active", side_effect=lambda _: next(states)), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot", side_effect=ConnectionRefusedError):
            with self.assertRaisesRegex(ValueError, "Backend not ready"):
                launcher.launch("show")
        manager.assert_called_once_with("start", "qoda-backend.service")

    def test_quit_only_stops_owned_units(self):
        with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager") as manager:
            launcher.launch("quit")
        manager.assert_called_once_with("stop", "qoda-ui.service", "qoda-backend.service")

    def test_protocol_readiness_uses_real_local_socket_and_rejects_mismatch(self):
        directory = self.runtime / "qoda"
        directory.mkdir(mode=0o700)
        path = directory / "backend.sock"
        for protocol in [1, 2]:
            server = socket.socket(socket.AF_UNIX)
            server.bind(str(path))
            server.listen(1)
            observed = []
            def serve():
                with server, server.accept()[0] as connection:
                    observed.append(connection.recv(1024))
                    connection.sendall(json.dumps({"protocol": protocol, "instance": "fixture"}).encode() + b"\n")
            thread = threading.Thread(target=serve)
            thread.start()
            try:
                if protocol == 1:
                    self.assertEqual(launcher.snapshot()["instance"], "fixture")
                else:
                    with self.assertRaisesRegex(ValueError, "protocol mismatch"):
                        launcher.snapshot()
            finally:
                thread.join(timeout=2)
                path.unlink()
            self.assertEqual(observed, [b'{"op":"hello","protocol":1}\n'])

    def test_runtime_symlink_is_preserved(self):
        outside = self.runtime / "outside"
        outside.mkdir()
        (self.runtime / "qoda").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            launcher.launch("show")
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
