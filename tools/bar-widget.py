#!/usr/bin/env python3
"""Install/remove Qoda's user-scoped Omarchy widget; activation is explicit."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
PLUGIN_ID = "qoda.player"
MARKER = ".qoda-install.json"
SHARED = ["CompactControls.qml", "WidgetClient.qml", "WidgetState.qml", "BackendState.qml", "BackendConnection.qml", "ThemedControl.qml", "Theme.qml", "OutputPresentation.qml"]


def fingerprint(directory):
    files = {}
    for path in directory.rglob("*"):
        if path.is_symlink():
            raise ValueError("Widget contains a symlink; preserved existing files")
        if path.is_file() and path != directory / MARKER:
            files[str(path.relative_to(directory))] = hashlib.sha256(path.read_bytes()).hexdigest()
    return files


def owned(directory):
    if directory.is_symlink():
        raise ValueError("Widget directory is a symlink; preserved existing files")
    try:
        metadata = json.loads((directory / MARKER).read_text())
    except (OSError, ValueError) as error:
        raise ValueError("Existing widget is not owned by this installer") from error
    if metadata.get("version") != 1 or metadata.get("id") != PLUGIN_ID or metadata.get("files") != fingerprint(directory):
        raise ValueError("Widget was modified or has an unsupported installation record; preserved existing files")
    return metadata


def config_home():
    return Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))).expanduser().absolute()


def backup_settings(config):
    settings = config / "omarchy/shell.json"
    if settings.is_symlink() or not settings.is_file():
        raise ValueError("Live activation requires a regular existing Omarchy shell.json")
    directory = config / "omarchy/qoda-widget-backups" / (time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:8])
    directory.mkdir(parents=True, mode=0o700)
    shutil.copy2(settings, directory / "shell.json")
    return directory / "shell.json"


def activate(config, root, launcher=None):
    if config != config_home():
        raise ValueError("Activation can only target the current desktop configuration")
    backup = backup_settings(config)
    print("Bar configuration backup:", backup)
    settings = json.loads(backup.read_text())
    right = settings.get("bar", {}).get("layout", {}).get("right", [])
    placement = ["--section", "right"]
    if any(isinstance(entry, dict) and entry.get("id") == "omarchy.audio" for entry in right):
        placement += ["--before", "omarchy.audio"]
    # First-time QML loading can outlast the shell wrapper's two-second default.
    shell_env = dict(os.environ, OMARCHY_SHELL_IPC_TIMEOUT="10s")
    subprocess.run(["omarchy-shell", "shell", "rescanPlugins"], check=True, timeout=15, env=shell_env)
    subprocess.run(["omarchy", "plugin", "enable", PLUGIN_ID, *placement], check=True, timeout=15, env=shell_env)
    subprocess.run(["omarchy", "bar", "set", PLUGIN_ID, "playerPath", str(root) if launcher is None else "", "--section", "right"], check=True, timeout=15, env=shell_env)
    subprocess.run(["omarchy", "bar", "set", PLUGIN_ID, "launcherPath", str(launcher) if launcher else "", "--section", "right"], check=True, timeout=15, env=shell_env)



def install(config, root=ROOT, enable=False, installed_prefix=None):
    config, root = Path(config).expanduser().absolute(), Path(root).resolve()
    if enable and config != config_home():
        raise ValueError("Activation can only target the current desktop configuration")
    launcher = None
    if installed_prefix is not None:
        launcher = Path(installed_prefix).expanduser().absolute() / "bin/qoda"
        if launcher.is_symlink() or not launcher.is_file() or not os.access(launcher, os.X_OK):
            raise ValueError("Installed prefix must contain a regular executable bin/qoda launcher")
    destination = config / "omarchy/plugins" / PLUGIN_ID
    previous = owned(destination) if destination.exists() or destination.is_symlink() else None
    staging_parent = config / "omarchy/qoda-widget-backups"
    staging_parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix="stage-", dir=staging_parent) as temporary:
        staged = Path(temporary) / PLUGIN_ID
        shutil.copytree(root / "integrations/omarchy" / PLUGIN_ID, staged)
        shared = staged / "qoda-ui"
        shared.mkdir()
        for name in SHARED:
            shutil.copyfile(root / "ui" / name, shared / name)
        shutil.copyfile(root / "LICENSE", staged / "LICENSE")
        (shared / "qmldir").write_text("singleton Theme 1.0 Theme.qml\n" + "".join(f"{Path(name).stem} 1.0 {name}\n" for name in SHARED if name != "Theme.qml"))
        # Quickshell can retain components at unchanged URLs across plugin rescans.
        # Version the entire QML bundle so shared imports also receive fresh URLs.
        digest = hashlib.sha256(json.dumps(fingerprint(staged), sort_keys=True).encode()).hexdigest()[:16]
        bundle = staged / ("payload-" + digest)
        bundle.mkdir()
        (staged / "BarWidget.qml").rename(bundle / "BarWidget.qml")
        shared.rename(bundle / "qoda-ui")
        manifest_path = staged / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["entryPoints"]["barWidget"] = str((bundle / "BarWidget.qml").relative_to(staged))
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        record = {"version": 1, "id": PLUGIN_ID, "files": fingerprint(staged), "activated": enable or bool(previous and previous.get("activated"))}
        (staged / MARKER).write_text(json.dumps(record, indent=2) + "\n")
        destination.parent.mkdir(parents=True, exist_ok=True)
        if previous:
            backup = staging_parent / ("plugin-" + uuid.uuid4().hex)
            destination.rename(backup)
        try:
            staged.rename(destination)
        except OSError:
            if previous:
                backup.rename(destination)
            raise
    if enable:
        activate(config, root, launcher)
    return destination


def remove(config):
    config = Path(config).expanduser().absolute()
    destination = config / "omarchy/plugins" / PLUGIN_ID
    metadata = owned(destination)
    if metadata.get("activated"):
        if config != config_home():
            raise ValueError("Disable the widget in its desktop before moving/removing an activated installation")
        print("Bar configuration backup:", backup_settings(config))
        subprocess.run(["omarchy", "plugin", "disable", PLUGIN_ID], check=True, timeout=15)
    shutil.rmtree(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["install", "remove"])
    parser.add_argument("--config-home", type=Path, default=config_home(), help="Alternate profile for isolated installation checks")
    parser.add_argument("--enable", action="store_true", help="Explicitly add the installed widget to the running desktop bar")
    parser.add_argument("--installed-prefix", type=Path, help="Use this installed Qoda prefix instead of the development checkout when enabling")
    args = parser.parse_args()
    try:
        if args.command == "install":
            print("Installed widget:", install(args.config_home, enable=args.enable, installed_prefix=args.installed_prefix))
        else:
            if args.enable:
                parser.error("--enable only applies to install")
            remove(args.config_home)
            print("Removed Qoda widget")
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
