import importlib.util
import hashlib
import sqlite3
import json
import multiprocessing
import os
import shutil
import socket
import threading
import time
from pathlib import Path
import subprocess
import sys
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
launch_module = load("qoda_launch", "tools/launch.py")
launcher = launch_module.Launch()


class Installation(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="qoda-package-")
        self.base = Path(self.temporary.name)
        self.prefix = self.base / 'prefix with spaces % $ ` "'
        self.config = self.base / "config"
        self.runtime = self.base / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.environment = mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.runtime), "XDG_STATE_HOME":str(self.base / "state"), "XDG_CACHE_HOME":str(self.base / "cache"), "XDG_CONFIG_HOME":str(self.config)})
        self.environment.start()
        self.service = mock.Mock()
        self.service.active.return_value = False
        self.service.units = launcher.units
        self.service.control_lock = launcher.control_lock
        self.service.for_release.return_value = self.service
        self.service.verify_ownership.side_effect = lambda directory: launch_module.Launch.verify_ownership(self.service, directory)
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
        shutil.copyfile(ROOT / "tools/launch.py", path / "launch.py")
        (path / "ui").mkdir()
        (path / "ui/shell.qml").write_text("// fictional UI\n")
        (path / "release.json").write_text(json.dumps({"version":"0.1.0","protocol":1,"storage_compatibility":installer.storage_evidence()}))
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

    def test_installed_launcher_status_and_quit_observe_checkpoint_failure(self):
        self.install()
        fakebin = self.base / "fakebin"
        fakebin.mkdir()
        events = self.base / "service-commands.jsonl"
        systemctl = fakebin / "systemctl"
        systemctl.write_text("#!/usr/bin/python3\nimport json,sys,os\nfrom pathlib import Path\nassert sys.argv[1] == '--user' and sys.argv[2] in ['show','stop']\nwith Path(" + repr(str(events)) + ").open('a') as log: log.write(json.dumps(sys.argv[1:])+'\\n')\nif '--property=FragmentPath' in sys.argv: print('FragmentPath='+str(Path(os.environ['XDG_CONFIG_HOME'])/'systemd/user'/sys.argv[-1])+'\\nDropInPaths=')\nelif sys.argv[2] == 'show':\n print('inactive' if '--property=ActiveState' in sys.argv else 'LoadState=loaded\\nResult=exit-code\\nExecMainCode=1\\nExecMainStatus=74')\n")
        systemctl.chmod(0o755)
        environment = dict(os.environ, PATH=str(fakebin) + os.pathsep + os.environ["PATH"])
        command = [str(self.prefix / "bin/qoda")]
        status = subprocess.run([*command, "status"], env=environment, capture_output=True, text=True, timeout=5, check=True)
        self.assertEqual(json.loads(status.stdout)["last_backend_exit"]["outcome"], "checkpoint_failed")
        stopped = subprocess.run([*command, "quit"], env=environment, capture_output=True, text=True, timeout=5)
        self.assertEqual(stopped.returncode, 1)
        self.assertIn("checkpoint failed", stopped.stderr)
        commands = [json.loads(line) for line in events.read_text().splitlines()]
        self.assertEqual([args for args in commands if args[1] == "stop"], [["--user", "stop", "qoda-ui.service", "qoda-backend.service"]])

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
        self.assertIn("RestartPreventExitStatus=74 75 76 78 127", unit)
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

    def running_manager(self, backend=True, ui=True):
        states = dict(zip(launcher.units, [backend, ui]))
        events = []
        self.service.active.side_effect = lambda unit: states[unit]
        def manager(*args, **kwargs):
            events.append(args)
            if args[0] == "show":
                unit=args[-1]
                return subprocess.CompletedProcess(args,0,stdout="FragmentPath="+str(self.config / "systemd/user" / unit)+"\nDropInPaths=\n")
            if args[0] in {"start","stop"}:
                for unit in args[1:]: states[unit]=args[0]=="start"
            return subprocess.CompletedProcess(args,0,stdout="")
        self.service.manager.side_effect=manager
        self.service.pause_checkpoint.side_effect=lambda:events.append(("pause-checkpoint",))
        self.service.snapshot.return_value={"protocol":1,"instance":"synthetic-restarted","intent":"paused","storage":{"state":"saved"},"archive":{"available":True}}
        self.service.ipc.return_value=True
        return states,events,manager

    def test_checkpointed_update_stages_before_stopping_and_restarts_paused(self):
        current=self.install();self.generation=2
        states,events,_=self.running_manager()
        stage=self.stage
        def build(path):
            self.assertTrue(states[launcher.units[0]])
            self.assertFalse(any(call[0]=="stop" for call in events))
            return stage(path)
        with mock.patch.object(installer,"stage_release",side_effect=build):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(current.resolve().name,"0.1.0-fixture2")
        self.assertLess(events.index(("stop",launcher.units[1])),events.index(("pause-checkpoint",)))
        self.assertLess(events.index(("pause-checkpoint",)),events.index(("stop",launcher.units[0])))
        self.assertLess(events.index(("stop",launcher.units[0])),events.index(("daemon-reload",)))
        self.assertTrue(all(states.values()))
        self.service.verify_shutdown.assert_called_once()
        self.service.write_owner.assert_called_once()
        self.service.ipc.assert_called_with("show")

    def test_checkpointed_stopped_update_and_compatible_rollback_never_start(self):
        current=self.install();self.generation=2
        states,events,_=self.running_manager(False,False)
        installer.install(self.prefix,self.config,checkpointed=True)
        builds=self.build_mock.call_count
        installer.install(self.prefix,self.config,rollback_release="0.1.0-fixture1")
        self.assertEqual(current.resolve().name,"0.1.0-fixture1")
        self.assertEqual(self.build_mock.call_count,builds)
        self.assertFalse(any(call[0] in {"start","stop"} for call in events))

    def test_checkpoint_failure_or_loaded_unit_override_keeps_code(self):
        current=self.install();before=installer.inventory(current.parent);self.generation=2
        states,events,_=self.running_manager()
        self.service.pause_checkpoint.side_effect=ValueError("synthetic checkpoint failure")
        with self.assertRaisesRegex(ValueError,"checkpoint failure"):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(installer.inventory(current.parent),before)
        self.assertTrue(states[launcher.units[0]])
        self.assertFalse(any(call[0]=="start" for call in events))
        self.running_manager()
        self.service.manager.side_effect=lambda *args,**kwargs:subprocess.CompletedProcess(args,0,stdout="FragmentPath=/other/unit\nDropInPaths=/override\n")
        with self.assertRaisesRegex(ValueError,"overridden or unowned"):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(installer.inventory(current.parent),before)

    def test_incompatible_reader_evidence_refuses_rollback_before_stopping(self):
        current=self.install();self.generation=2
        original=installer.storage_evidence()
        changed=json.loads(json.dumps(original));changed["files"]["synthetic-model-change"]="f"*64
        changed["fingerprint"]=hashlib.sha256(json.dumps(changed["files"],sort_keys=True).encode()).hexdigest()
        with mock.patch.object(installer,"storage_evidence",return_value=changed): self.install()
        states,events,_=self.running_manager()
        with self.assertRaisesRegex(ValueError,"readers/models"):
            installer.install(self.prefix,self.config,rollback_release="0.1.0-fixture1")
        self.assertEqual(current.resolve().name,"0.1.0-fixture2")
        self.assertFalse(any(call[0]=="stop" for call in events))

    def test_newer_database_or_settings_refuse_code_selection_and_preserve_bytes(self):
        current=self.install();self.generation=2;before=installer.inventory(current.parent)
        settings=self.config / "qoda/output.json";settings.parent.mkdir(parents=True,exist_ok=True)
        settings.write_text('{"version":99}');settings.chmod(0o600)
        self.running_manager(False,False)
        with self.assertRaisesRegex(ValueError,"Unsupported settings"):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(settings.read_text(),'{"version":99}');self.assertEqual(installer.inventory(current.parent),before)
        settings.unlink()
        database=self.base / "state/qoda/session.sqlite3";database.parent.mkdir(parents=True)
        connection=sqlite3.connect(database);connection.execute("PRAGMA application_id=1364141121");connection.execute("PRAGMA user_version=99");connection.close();database.chmod(0o600)
        data=database.read_bytes()
        with self.assertRaisesRegex(ValueError,"Unsupported or damaged"):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(database.read_bytes(),data);self.assertEqual(installer.inventory(current.parent),before)

    def test_failed_activation_rolls_back_and_restarts_only_identical_readers(self):
        current=self.install();self.generation=2
        states,events,manager=self.running_manager()
        failed=False
        def fail_ui_once(*args,**kwargs):
            nonlocal failed
            if args==("start",launcher.units[1]) and not failed:
                failed=True;raise subprocess.CalledProcessError(1,args)
            return manager(*args,**kwargs)
        self.service.manager.side_effect=fail_ui_once
        with self.assertRaises(subprocess.CalledProcessError): installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(current.resolve().name,"0.1.0-fixture1")
        self.assertTrue(all(states.values()))
        self.assertEqual(events.count(("start",launcher.units[0])),2)
        self.assertFalse(list((self.prefix / "lib").glob(".qoda-stage-*")))

    def test_failed_activation_with_changed_models_retains_both_payloads_and_blocks_restart(self):
        current=self.install();self.generation=2
        states,events,manager=self.running_manager()
        original=installer.storage_evidence();changed=json.loads(json.dumps(original));changed["files"]["changed-model"]="e"*64
        changed["fingerprint"]=hashlib.sha256(json.dumps(changed["files"],sort_keys=True).encode()).hexdigest()
        def fail_ui(*args,**kwargs):
            if args==("start",launcher.units[1]): raise subprocess.CalledProcessError(1,args)
            return manager(*args,**kwargs)
        self.service.manager.side_effect=fail_ui
        with mock.patch.object(installer,"storage_evidence",return_value=changed):
            with self.assertRaisesRegex(ValueError,"data compatibility is unproven"):
                installer.install(self.prefix,self.config,checkpointed=True)
        self.assertEqual(current.resolve().name,"0.1.0-fixture1")
        self.assertFalse(any(states.values()))
        retained=list((self.prefix / "lib").glob(".qoda-stage-*"));self.assertEqual(len(retained),1)
        self.assertTrue((retained[0] / "recovery-needed").exists())
        self.assertTrue((retained[0] / "failed-release/releases/0.1.0-fixture2/qoda-backend").exists())
        with self.assertRaisesRegex(ValueError,"Unfinished Qoda transaction"): self.install()

    def test_interrupted_update_can_restore_verified_code_without_restoring_data(self):
        current=self.install();self.generation=2;states,events,_=self.running_manager(False,False)
        data=self.base / "state/synthetic-retained-data";data.parent.mkdir();data.write_bytes(b"unchanged private fixture")
        atomic=installer.atomic_file
        def interrupted(*args): atomic(*args);os._exit(23)
        def child():
            with mock.patch.object(installer,"atomic_file",side_effect=interrupted): installer.install(self.prefix,self.config,checkpointed=True)
        process=multiprocessing.get_context("fork").Process(target=child);process.start();process.join(timeout=10)
        try:
            self.assertEqual(process.exitcode,23)
            retained=list((self.prefix / "lib").glob(".qoda-stage-*"));self.assertEqual(len(retained),1)
            installer.recover(self.prefix,self.config,retained[0])
            self.assertEqual(current.resolve().name,"0.1.0-fixture1")
            self.assertEqual(data.read_bytes(),b"unchanged private fixture")
            self.assertFalse((retained[0] / "recovery-needed").exists())
            self.assertTrue((retained[0] / "recovered-candidate/releases/0.1.0-fixture2/qoda-backend").exists())
            installer.owned(current.parent,{"prefix":self.prefix,"config":self.config})
            self.assertFalse(any(call[0]=="start" for call in events))
        finally:
            if process.is_alive():process.kill();process.join()

    def test_activation_lock_blocks_old_launch_and_quit_then_refuses_stale_release(self):
        current=self.install();self.generation=2
        states,events,_=self.running_manager()
        checkpoint_entered=threading.Event();continue_switch=threading.Event();finished=threading.Event();errors=[]
        def checkpoint():checkpoint_entered.set();self.assertTrue(continue_switch.wait(3))
        self.service.pause_checkpoint.side_effect=checkpoint
        def update():
            try:installer.install(self.prefix,self.config,checkpointed=True)
            except BaseException as error:errors.append(error)
        thread=threading.Thread(target=update);thread.start();self.assertTrue(checkpoint_entered.wait(3))
        old=current.resolve()
        def stale_launch():
            try:launcher.launch("quit")
            except ValueError as error:errors.append(error)
            finally:finished.set()
        with mock.patch.object(launcher,"root",old), mock.patch.object(launcher,"manager",self.service.manager), mock.patch.object(launcher,"active",self.service.active), mock.patch.object(launcher,"verify_shutdown",self.service.verify_shutdown):
            contender=threading.Thread(target=stale_launch);contender.start()
            self.assertFalse(finished.wait(.1));continue_switch.set();thread.join(5);contender.join(5)
        self.assertFalse(thread.is_alive());self.assertFalse(contender.is_alive())
        self.assertEqual(current.resolve().name,"0.1.0-fixture2")
        self.assertEqual(len(errors),1);self.assertRegex(str(errors[0]),"Release changed")

    def test_interruption_immediately_after_atomic_exchange_is_recoverable(self):
        current=self.install();self.generation=2;self.running_manager(False,False)
        exchange=installer.exchange_paths
        def interrupted(*args):exchange(*args);os._exit(24)
        def child():
            with mock.patch.object(installer,"exchange_paths",side_effect=interrupted):installer.install(self.prefix,self.config,checkpointed=True)
        process=multiprocessing.get_context("fork").Process(target=child);process.start();process.join(10)
        try:
            self.assertEqual(process.exitcode,24);self.assertTrue(current.exists())
            stage=list((self.prefix / "lib").glob(".qoda-stage-*"))[0]
            self.assertFalse((stage / "previous").exists())
            self.assertTrue((stage / "qoda-player/releases/0.1.0-fixture1/qoda-backend").exists())
            installer.recover(self.prefix,self.config,stage)
            self.assertEqual(current.resolve().name,"0.1.0-fixture1")
        finally:
            if process.is_alive():process.kill();process.join()

    def test_failed_directory_sync_after_exchange_restores_previous_tree(self):
        current=self.install();before=installer.inventory(current.parent);self.generation=2
        sync=installer.sync_directory;failed=False
        def fail_once(path):
            nonlocal failed
            if path==self.prefix / "lib" and not failed:
                failed=True;raise OSError("synthetic directory sync failure")
            return sync(path)
        with mock.patch.object(installer,"sync_directory",side_effect=fail_once):
            with self.assertRaisesRegex(OSError,"directory sync"):
                self.install()
        self.assertEqual(installer.inventory(current.parent),before)
        self.assertTrue(current.exists())

    def test_final_shutdown_failure_aborts_selection_after_stopping_owned_units(self):
        current=self.install();before=installer.inventory(current.parent);self.generation=2
        states,events,_=self.running_manager()
        self.service.verify_shutdown.side_effect=ValueError("synthetic final checkpoint failure")
        with self.assertRaisesRegex(ValueError,"final checkpoint failure"):
            installer.install(self.prefix,self.config,checkpointed=True)
        self.assertFalse(any(states.values()))
        self.assertFalse(any(call[0]=="start" for call in events))
        self.assertEqual(installer.inventory(current.parent),before)

    def test_launcher_refuses_different_effective_manager_profile(self):
        environment={"WAYLAND_DISPLAY":"synthetic-wayland","DBUS_SESSION_BUS_ADDRESS":"unix:path=/synthetic-bus","XDG_DATA_HOME":str(self.base / "data")}
        with mock.patch.dict(os.environ,environment):
            fields={name:os.environ[name] for name in ["WAYLAND_DISPLAY","XDG_RUNTIME_DIR","DBUS_SESSION_BUS_ADDRESS","XDG_CONFIG_HOME","XDG_STATE_HOME","XDG_CACHE_HOME","XDG_DATA_HOME"]}
            fields["XDG_STATE_HOME"]=str(self.base / "other-state")
            reply=subprocess.CompletedProcess([],0,stdout="\n".join(key+"="+value for key,value in fields.items()))
            with mock.patch.object(launcher,"manager",return_value=reply):
                with self.assertRaisesRegex(ValueError,"profile differs.*XDG_STATE_HOME"):launcher.verify_session()
            fields["XDG_STATE_HOME"]=os.environ["XDG_STATE_HOME"]
            reply=subprocess.CompletedProcess([],0,stdout="\n".join(key+"="+value for key,value in fields.items()))
            with mock.patch.object(launcher,"manager",return_value=reply):launcher.verify_session()


class ReleaseBuild(unittest.TestCase):
    def test_locked_remapped_build_is_stable_and_refuses_personal_paths(self):
        with tempfile.TemporaryDirectory(prefix="qoda-build-fixture-") as temporary:
            base = Path(temporary)
            source, target = base / "source", base / "target"
            (source / "ui").mkdir(parents=True)
            (source / "tools").mkdir()
            (source / "ui/shell.qml").write_text("// fictional UI\n")
            (source / "ui/i18n").mkdir()
            for asset in ["Catalog.js", "German.js", "de.json"]:
                (source / "ui/i18n" / asset).write_text("fictional localization asset: " + asset)
            (source / "tools/launch.py").write_text("# fictional entry point\n")
            (source / "LICENSE").write_text("MIT\n")
            (source / "Cargo.toml").write_text('[workspace.package]\nversion="0.1.0"\n')
            (target / "release").mkdir(parents=True)
            binary = target / "release/qoda-backend"
            binary.write_bytes(b"fictional path-free binary")
            def command(args, **kwargs):
                return subprocess.CompletedProcess(args, 0, stdout="rustc 1.99.1 (fixture)\n")
            contract = installer.storage_evidence()
            with mock.patch.object(installer, "ROOT", source), mock.patch.object(installer, "storage_evidence", return_value=contract), mock.patch.object(installer, "preflight"), mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target), "RUSTFLAGS": "-C opt-level=2", "CARGO_ENCODED_RUSTFLAGS": ""}), mock.patch.object(installer.subprocess, "run", side_effect=command) as run:
                first = installer.stage_release(base / "release-one")
                second = installer.stage_release(base / "release-two")
                self.assertEqual(first, second)
                for asset in ["Catalog.js", "German.js", "de.json"]:
                    self.assertEqual((base / "release-one/ui/i18n" / asset).read_bytes(), (source / "ui/i18n" / asset).read_bytes())
                (source / "ui/i18n/unreviewed.js").write_text("unexpected")
                with self.assertRaisesRegex(ValueError, "unexpected files"):
                    installer.stage_release(base / "rejected-ui")
                (source / "ui/i18n/unreviewed.js").unlink()
                build = run.call_args_list[0]
                self.assertIn("--locked", build.args[0])
                flags = build.kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
                self.assertEqual(flags[:2], ["-C", "opt-level=2"])
                self.assertIn(f"--remap-path-prefix={source}=/build/qoda-player", flags)
                binary.write_bytes(str(Path.home()).encode())
                with self.assertRaisesRegex(ValueError, "personal build paths"):
                    installer.stage_release(base / "rejected-release")


class DevelopmentShutdown(unittest.TestCase):
    def test_owned_backend_checkpoint_outcome_reaches_development_exit_status(self):
        with tempfile.TemporaryDirectory(prefix="qoda-dev-exit-") as runtime:
            with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": runtime}):
                development = load("qoda_dev_exit", "tools/dev.py")
            for status in [0, 74, 75, 76]:
                backend = mock.Mock(returncode=None)
                backend.poll.side_effect = lambda: backend.returncode
                def wait(**kwargs):
                    backend.returncode = status
                    return status
                backend.wait.side_effect = wait
                with mock.patch.object(sys, "argv", ["dev.py", "--backend-only"]), mock.patch.object(development, "request", side_effect=[ConnectionRefusedError(), {}]), mock.patch.object(development.subprocess, "run"), mock.patch.object(development.subprocess, "Popen", return_value=backend):
                    if status:
                        with self.assertRaises(SystemExit) as error:
                            development.main()
                        self.assertEqual(error.exception.code, status)
                    else:
                        development.main()
                backend.terminate.assert_not_called()



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
            with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager, mock.patch.object(launcher, "snapshot") as snapshot:
                launcher.launch(action)
                if action == "hide":
                    manager.assert_not_called()
                else:
                    self.assertEqual(manager.call_args.args[0], "show")
                snapshot.assert_not_called()

    def test_first_toggle_shows_and_existing_toggle_toggles(self):
        for ui_running in [False, True]:
            with mock.patch.object(launcher, "active", side_effect=lambda unit: unit == launcher.units[0] or ui_running), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager, mock.patch.object(launcher, "snapshot", return_value={"protocol": 1, "instance": "fixture"}), mock.patch.object(launcher, "ipc", return_value=True) as ipc:
                launcher.launch("toggle")
                ipc.assert_called_once_with("toggle" if ui_running else "show")
                self.assertEqual([call.args for call in manager.call_args_list], [("start", launcher.units[0]), ("start", launcher.units[1])])

    def test_fenced_activation_never_starts_a_backend_and_writes_private_owner(self):
        with mock.patch.object(launcher, "active", side_effect=lambda unit: unit == launcher.units[0]), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot", return_value={"instance": "fixture"}), mock.patch.object(launcher, "ipc", return_value=True) as ipc:
            launcher.launch("search", expected_instance="fixture", expires=time.monotonic() * 1000 + 5000)
        manager.assert_called_once_with("start", launcher.units[1])
        ipc.assert_called_once_with("search")
        owner = launcher.read_owner()
        self.assertEqual(owner["instance"], "fixture")
        self.assertEqual(launcher.owner_path().stat().st_mode & 0o777, 0o600)

    def test_missing_changed_and_expired_activation_cannot_start_services(self):
        for running, instance, expiry in [(False, "fixture", None), (True, "newer", None), (True, "fixture", time.monotonic() * 1000 - 1)]:
            with mock.patch.object(launcher, "active", return_value=running), mock.patch.object(launcher, "manager") as manager, mock.patch.object(launcher, "snapshot", return_value={"instance": instance}):
                with self.assertRaises(ValueError):
                    launcher.launch("show", expected_instance="fixture", expires=expiry)
                manager.assert_not_called()

    def test_unsupported_ui_owner_record_is_preserved(self):
        with launcher.control_lock():
            path = launcher.owner_path()
            original = b'{"format":2,"private":"preserved"}'
            path.write_bytes(original)
            path.chmod(0o600)
            with self.assertRaises(ValueError):
                launcher.write_owner("fixture", time.monotonic() * 1000 + 5000)
            self.assertEqual(path.read_bytes(), original)

    def test_nonregular_ui_handoff_is_rejected_without_blocking(self):
        with launcher.control_lock():
            os.mkfifo(launcher.owner_path(), 0o600)
        with self.assertRaisesRegex(ValueError, "unsafe"):
            launcher.read_owner()

    def test_ui_runner_terminates_only_its_ui_when_backend_changes(self):
        with launcher.control_lock():
            launcher.write_owner("fixture", time.monotonic() * 1000 + 5000)
        child = mock.Mock()
        child.poll.return_value = None
        with mock.patch.object(launcher, "snapshot", side_effect=[{"instance":"fixture"}, {"instance":"newer"}]), mock.patch.object(subprocess, "Popen", return_value=child), mock.patch.object(launcher, "manager") as manager:
            with self.assertRaisesRegex(ValueError, "instance changed"):
                launcher.run_ui()
        child.terminate.assert_called_once()
        child.wait.assert_called_once_with(timeout=3)
        manager.assert_not_called()

    def test_expired_ui_handoff_starts_nothing(self):
        with launcher.control_lock():
            launcher.write_owner("fixture", time.monotonic() * 1000 - 1)
        with mock.patch.object(subprocess, "Popen") as process:
            with self.assertRaisesRegex(ValueError, "expired"):
                launcher.run_ui()
        process.assert_not_called()

    def test_unmanaged_backend_or_protocol_failure_never_starts_ui(self):
        for error in [None, ValueError("protocol mismatch")]:
            with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager, mock.patch.object(launcher, "snapshot", side_effect=error, return_value={"protocol": 1, "instance": "fixture"}):
                with self.assertRaises(ValueError):
                    launcher.launch("show")
                manager.assert_not_called()

    def test_interrupted_install_blocks_start_but_quit_remains_available(self):
        root = self.runtime / "prefix/lib/qoda-player/releases/fixture"
        root.mkdir(parents=True)
        stage = self.runtime / "prefix/lib/.qoda-stage-fixture"
        stage.mkdir()
        (stage / "recovery-needed").touch()
        with mock.patch.object(launcher, "root", root), mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager:
            with self.assertRaisesRegex(ValueError, "interrupted installation"):
                launcher.launch("show")
            manager.assert_not_called()
            launcher.launch("quit")
            self.assertEqual(manager.call_args_list[0], mock.call("stop", "qoda-ui.service", "qoda-backend.service", check=False))

    def test_backend_start_failure_does_not_start_ui(self):
        states = iter([False, False])
        with mock.patch.object(launcher, "active", side_effect=lambda _: next(states)), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager, mock.patch.object(launcher, "snapshot", side_effect=ConnectionRefusedError):
            with self.assertRaisesRegex(ValueError, "Backend not ready"):
                launcher.launch("show")
        manager.assert_called_once_with("start", "qoda-backend.service")

    def test_quit_only_stops_owned_units(self):
        with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout="LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus=0\n")) as manager:
            launcher.launch("quit")
        self.assertEqual(manager.call_args_list[0], mock.call("stop", "qoda-ui.service", "qoda-backend.service", check=False))

    def test_quit_reports_persistence_failure_even_when_stop_job_succeeds(self):
        for status, message in [(74, "checkpoint failed"), (75, "session was saved"), (76, "cleanup failed"), (99, "unconfirmed")]:
            report = f"LoadState=loaded\nResult=success\nExecMainCode=1\nExecMainStatus={status}\n"
            with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout=report)) as manager:
                with self.assertRaisesRegex(ValueError, message):
                    launcher.launch("quit")
                self.assertEqual(manager.call_args_list[0], mock.call("stop", "qoda-ui.service", "qoda-backend.service", check=False))
                self.assertFalse(any(call.args[0] == "start" for call in manager.call_args_list))

    def test_signal_missing_and_malformed_shutdown_results_are_not_success(self):
        for report in ["", "LoadState=not-found\n", "LoadState=loaded\nResult=success\nExecMainCode=2\nExecMainStatus=2\n"]:
            with mock.patch.object(launcher, "manager", return_value=subprocess.CompletedProcess([], 0, stdout=report)):
                with self.assertRaisesRegex(ValueError, "unconfirmed"):
                    launcher.verify_shutdown()

    def test_failed_stop_job_still_surfaces_the_checkpoint_error(self):
        def manager(*args, **kwargs):
            report = "LoadState=loaded\nResult=exit-code\nExecMainCode=1\nExecMainStatus=74\n"
            return subprocess.CompletedProcess([], 1 if args[0] == "stop" else 0, stdout=report)
        with mock.patch.object(launcher, "active", return_value=False), mock.patch.object(launcher, "manager", side_effect=manager):
            with self.assertRaisesRegex(ValueError, "checkpoint failed"):
                launcher.launch("quit")

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
