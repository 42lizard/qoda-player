import importlib.util
import json
import shutil
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("bar_widget", ROOT / "tools/bar-widget.py")
widget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(widget)


class Installer(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="qoda-widget-install-")
        self.config = Path(self.temporary.name) / "profile with spaces"

    def tearDown(self):
        self.temporary.cleanup()

    def test_install_update_remove_only_owned_payload_and_leave_other_configuration(self):
        shell = self.config / "omarchy/shell.json"
        shell.parent.mkdir(parents=True)
        original = b'{"version":1,"plugins":[],"unrelated":"preserved"}\n'
        shell.write_bytes(original)
        path = widget.install(self.config)
        entry = json.loads((path / "manifest.json").read_text())["entryPoints"]["barWidget"]
        self.assertTrue((path / entry).parent.joinpath("qoda-ui/CompactControls.qml").is_file())
        self.assertEqual(widget.owned(path)["id"], "qoda.player")
        self.assertEqual(shell.read_bytes(), original)
        widget.install(self.config)
        widget.remove(self.config)
        self.assertFalse(path.exists())
        self.assertEqual(shell.read_bytes(), original)

    def test_changed_payload_gets_fresh_qml_urls(self):
        source = Path(self.temporary.name) / "source"
        shutil.copytree(ROOT / "integrations", source / "integrations")
        shutil.copytree(ROOT / "ui", source / "ui")
        shutil.copyfile(ROOT / "LICENSE", source / "LICENSE")
        def entry():
            path = widget.install(self.config, root=source)
            return path, json.loads((path / "manifest.json").read_text())["entryPoints"]["barWidget"]
        path, initial = entry()
        self.assertEqual(entry()[1], initial)
        for relative in ["ui/CompactControls.qml", "integrations/omarchy/qoda.player/BarWidget.qml"]:
            changed = source / relative
            changed.write_text(changed.read_text() + "\n// changed payload\n")
            path, updated = entry()
            self.assertNotEqual(updated, initial)
            self.assertTrue((path / updated).is_file())
            self.assertEqual((path / updated).parent.joinpath("qoda-ui/CompactControls.qml").read_bytes(), (source / "ui/CompactControls.qml").read_bytes())
            initial = updated

    def test_user_edits_and_additional_files_are_preserved(self):
        for name in [None, "custom.txt", "notes/.qoda-install.json"]:
            path = widget.install(self.config)
            if name is None:
                name = json.loads((path / "manifest.json").read_text())["entryPoints"]["barWidget"]
            (path / name).parent.mkdir(parents=True, exist_ok=True)
            (path / name).write_text("user-owned edit")
            with self.assertRaises(ValueError):
                widget.install(self.config)
            with self.assertRaises(ValueError):
                widget.remove(self.config)
            self.assertEqual((path / name).read_text(), "user-owned edit")
            # Remove only this temporary test profile before the second case.
            shutil.rmtree(path)

    def test_unowned_newer_and_symlink_installations_are_never_replaced(self):
        path = self.config / "omarchy/plugins/qoda.player"
        path.mkdir(parents=True)
        with self.assertRaises(ValueError):
            widget.install(self.config)
        (path / widget.MARKER).write_text(json.dumps({"version": 2, "id": "qoda.player", "files": {}}))
        with self.assertRaises(ValueError):
            widget.remove(self.config)
        (path / widget.MARKER).unlink()
        path.rmdir()
        outside = self.config / "outside"
        outside.mkdir()
        (outside / "sentinel").write_text("preserved")
        path.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            widget.install(self.config)
        self.assertEqual((outside / "sentinel").read_text(), "preserved")

    def test_activation_uses_supported_cli_arguments_and_preserves_a_config_backup(self):
        shell = self.config / "omarchy/shell.json"
        shell.parent.mkdir(parents=True)
        original = b'{"bar":{"layout":{"right":[{"id":"omarchy.audio"}]}},"version":1}\n'
        shell.write_bytes(original)
        enabled = False
        def cli(command, **kwargs):
            nonlocal enabled
            if command[:3] == ["omarchy", "plugin", "enable"]:
                enabled = True
            if command[:3] == ["omarchy", "bar", "set"] and not enabled:
                raise RuntimeError("Omarchy cannot configure a widget before it has a bar entry")
        with mock.patch.object(widget, "config_home", return_value=self.config), mock.patch.object(widget.subprocess, "run", side_effect=cli) as run:
            widget.activate(self.config, ROOT)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[2], ["omarchy", "bar", "set", "qoda.player", "playerPath", str(ROOT), "--section", "right"])
        self.assertEqual(commands[1], ["omarchy", "plugin", "enable", "qoda.player", "--section", "right", "--before", "omarchy.audio"])
        backups = list((self.config / "omarchy/qoda-widget-backups").glob("*/shell.json"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), original)

    def test_alternate_profile_cannot_activate_the_real_desktop(self):
        with self.assertRaises(ValueError):
            widget.install(self.config, enable=True)
        self.assertFalse((self.config / "omarchy/plugins/qoda.player").exists())


if __name__ == "__main__":
    unittest.main()
