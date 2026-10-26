import importlib.util
import json
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
        self.assertTrue((path / "qoda-ui/CompactControls.qml").is_file())
        self.assertEqual(widget.owned(path)["id"], "qoda.player")
        self.assertEqual(shell.read_bytes(), original)
        widget.install(self.config)
        widget.remove(self.config)
        self.assertFalse(path.exists())
        self.assertEqual(shell.read_bytes(), original)

    def test_user_edits_and_additional_files_are_preserved(self):
        for name in ["BarWidget.qml", "custom.txt", "notes/.qoda-install.json"]:
            path = widget.install(self.config)
            (path / name).parent.mkdir(parents=True, exist_ok=True)
            (path / name).write_text("user-owned edit")
            with self.assertRaises(ValueError):
                widget.install(self.config)
            with self.assertRaises(ValueError):
                widget.remove(self.config)
            self.assertEqual((path / name).read_text(), "user-owned edit")
            # Remove only this temporary test profile before the second case.
            import shutil
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
        with mock.patch.object(widget, "config_home", return_value=self.config), mock.patch.object(widget.subprocess, "run") as run:
            widget.activate(self.config, ROOT)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[1], ["omarchy", "bar", "set", "qoda.player", "playerPath", str(ROOT), "--section", "right"])
        self.assertEqual(commands[2], ["omarchy", "plugin", "enable", "qoda.player", "--section", "right", "--before", "omarchy.audio"])
        backups = list((self.config / "omarchy/qoda-widget-backups").glob("*/shell.json"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), original)

    def test_alternate_profile_cannot_activate_the_real_desktop(self):
        with self.assertRaises(ValueError):
            widget.install(self.config, enable=True)
        self.assertFalse((self.config / "omarchy/plugins/qoda.player").exists())


if __name__ == "__main__":
    unittest.main()
