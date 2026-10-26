#!/usr/bin/env python3
"""Build/install or remove Qoda in a user prefix. Never starts services."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import shlex
import stat
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
RECORD = "installation.json"
EXTERNAL = {"prefix": ("bin/qoda", "share/applications/qoda-player.desktop", "share/icons/hicolor/scalable/apps/qoda-player.svg"),
            "config": ("systemd/user/qoda-backend.service", "systemd/user/qoda-ui.service")}


def regular_hash(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError(f"Expected a regular non-linked file: {path}")
    return {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "mode": stat.S_IMODE(info.st_mode)}


def inventory(root):
    result = {}
    for path in sorted(root.rglob("*")):
        relative = str(path.relative_to(root))
        if relative == RECORD:
            continue
        if relative == "current" and path.is_symlink():
            result[relative] = {"target": os.readlink(path)}
        elif path.is_symlink():
            raise ValueError(f"Unexpected symlink: {path}")
        elif path.is_file():
            result[relative] = regular_hash(path)
        elif path.is_dir():
            result[relative] = {"directory": stat.S_IMODE(path.stat().st_mode)}
        else:
            raise ValueError(f"Unexpected artifact: {path}")
    return result


def safe_path(path):
    path = Path(os.path.abspath(Path(path).expanduser()))
    for parent in [*reversed(path.parents), path]:
        if parent.is_symlink():
            raise ValueError(f"Symlinked installation path preserved: {parent}")
    if any(ord(c) < 32 or ord(c) == 127 for c in str(path)):
        raise ValueError("Installation paths must not contain control characters")
    return path


def owned(root, anchors):
    if not root.exists():
        return None
    regular_hash(root / RECORD)
    metadata = json.loads((root / RECORD).read_text())
    if not isinstance(metadata, dict) or metadata.get("schema") != 1 or metadata.get("files") != inventory(root):
        raise ValueError("Installation is modified, unowned or unsupported; existing files preserved")
    expected = {domain: {name: regular_hash(safe_path(anchor / name)) for name in EXTERNAL[domain]}
                for domain, anchor in anchors.items()}
    if metadata.get("external") != expected:
        raise ValueError("Installed launcher/desktop/units changed; existing files preserved")
    target = metadata["files"].get("current", {}).get("target", "")
    if not target.startswith("releases/") or Path(target).name != target.removeprefix("releases/") or not (root / target).is_dir():
        raise ValueError("Invalid current release; installation preserved")
    return metadata


@contextmanager
def staging(parent, prefix):
    directory = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
    try:
        yield directory
    finally:
        if (directory / "recovery-needed").exists():
            print("Incomplete transaction: recovery files retained at", directory)
        else:
            shutil.rmtree(directory)


def prepare_recovery(stage, anchors, previous):
    if previous:
        for domain, names in EXTERNAL.items():
            for name in names:
                saved = stage / "external" / domain / name
                saved.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(anchors[domain] / name, saved)
    marker = stage / "recovery-needed"
    marker.write_text("Retained previous payload and external files for manual recovery. Do not delete until repaired.\n")
    return marker


def atomic_file(path, content, mode):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".qoda-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
            os.fchmod(stream.fileno(), mode)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def quote_exec(value, desktop=False):
    value = str(value).replace("%", "%%").replace("\\", "\\\\").replace('"', '\\"')
    if desktop:
        # Desktop Entry string escaping happens before Exec argument unquoting.
        value = value.replace("$", "\\$").replace("`", "\\`").replace("\\", "\\\\")
    else:
        value = value.replace("$", "$$")
    return '"' + value + '"'


def integrations(prefix):
    launcher = prefix / "bin/qoda"
    current = prefix / "lib/qoda-player/current"
    wrapper = ("#!/usr/bin/python3\nimport runpy\nrunpy.run_path(" + repr(str(current / "launch.py")) + ", run_name='__main__')\n").encode()
    desktop = ("[Desktop Entry]\nType=Application\nName=Qoda Player\nComment=Browse and play your music\n"
               f"Exec=/usr/bin/python3 {quote_exec(launcher, True)} show\nIcon=qoda-player\nTerminal=false\nCategories=AudioVideo;Audio;Player;\n"
               "StartupWMClass=qoda-player\nActions=Toggle;Compact;Quit;\n" + "".join(
                   f"\n[Desktop Action {name}]\nName={name}\nExec=/usr/bin/python3 {quote_exec(launcher, True)} {action}\n"
                   for name, action in [("Toggle", "toggle"), ("Compact", "compact"), ("Quit", "quit")])).encode()
    common = ("[Unit]\nDescription=Qoda {name}\nPartOf=graphical-session.target\nRequisite=graphical-session.target\nAfter=graphical-session.target\n"
              "StartLimitIntervalSec=60\nStartLimitBurst=3\n\n[Service]\nType=exec\n")
    backend = (common.format(name="backend") + f"ExecStart=/usr/bin/env -- {quote_exec(current / 'qoda-backend')}\n"
               "Restart=on-failure\nRestartPreventExitStatus=74 75 76\nRestartSec=2\nKillSignal=SIGINT\nTimeoutStopSec=10\nUMask=0077\n").encode()
    ui = (common.format(name="interface") + f"ExecStart=quickshell --path {quote_exec(current / 'ui')} --no-duplicate\n"
          "Restart=no\nTimeoutStopSec=5\nUMask=0077\n").encode()
    return {"prefix": {"bin/qoda": (wrapper, 0o755), "share/applications/qoda-player.desktop": (desktop, 0o644),
                       "share/icons/hicolor/scalable/apps/qoda-player.svg": ((ROOT / "packaging/qoda-player.svg").read_bytes(), 0o644)},
            "config": {"systemd/user/qoda-backend.service": (backend, 0o644), "systemd/user/qoda-ui.service": (ui, 0o644)}}


def launcher_module():
    spec = importlib.util.spec_from_file_location("qoda_launcher", ROOT / "tools/launch.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def transaction_lock(prefix):
    prefix.mkdir(parents=True, exist_ok=True)
    fd = os.open(prefix, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        os.close(fd)


def require_stopped():
    module = launcher_module()
    if any(module.active(unit) for unit in module.UNITS):
        raise ValueError("Quit Qoda before switching/removing releases; no playback was stopped")
    if (Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/backend.sock").exists():
        raise ValueError("A Qoda backend socket remains; quit the development player before changing releases")


def preflight():
    required = ["cargo", "rustc", "quickshell", "pkg-config", "systemctl"]
    missing = [name for name in required if shutil.which(name) is None]
    if missing:
        raise ValueError("Missing build/runtime commands: " + ", ".join(missing))
    expected = (ROOT / "packaging/toolchain.txt").read_text().strip()
    actual = subprocess.run(["rustc", "--version"], check=True, text=True, capture_output=True).stdout.split()
    if actual[:2] != expected.split():
        raise ValueError(f"Release builds require {expected}; install that toolchain explicitly")
    subprocess.run(["pkg-config", "--exists", "mpv", "alsa", "sqlite3"], check=True)
    if not shutil.which("secret-tool"):
        print("Secret Service tooling unavailable: sign-in is session-only until libsecret is installed.")


def stage_release(destination):
    preflight()
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
    if not target.is_absolute():
        target = ROOT / target
    target = target.resolve()
    environment = dict(os.environ)
    flags = environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if environment.get("CARGO_ENCODED_RUSTFLAGS") else shlex.split(environment.get("RUSTFLAGS", ""))
    flags += [f"--remap-path-prefix={Path.home()}=/build/user", f"--remap-path-prefix={ROOT}=/build/qoda-player", f"--remap-path-prefix={target}=/build/target"]
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "qoda-backend", "--target-dir", str(target)], cwd=ROOT, check=True, env=environment)
    destination.mkdir()
    shutil.copyfile(target / "release/qoda-backend", destination / "qoda-backend")
    (destination / "qoda-backend").chmod(0o755)
    for path in (ROOT / "ui").iterdir():
        if path.is_symlink() or not path.is_file() or (path.suffix != ".qml" and path.name != "qmldir"):
            raise ValueError("UI source contains unexpected files; review before packaging")
    shutil.copytree(ROOT / "ui", destination / "ui")
    shutil.copyfile(ROOT / "tools/launch.py", destination / "launch.py")
    shutil.copyfile(ROOT / "LICENSE", destination / "LICENSE")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    toolchain = subprocess.run(["rustc", "--version"], check=True, text=True, capture_output=True).stdout.strip()
    (destination / "release.json").write_text(json.dumps({"version": version, "protocol": 1, "rustc": toolchain}, indent=2) + "\n")
    binary = (destination / "qoda-backend").read_bytes()
    if any(str(path).encode() in binary for path in [Path.home(), ROOT, target]):
        raise ValueError("Release binary still contains personal build paths; installation preserved")
    destination.chmod(0o755)
    for path in destination.rglob("*"):
        path.chmod(0o755 if path.is_dir() or path == destination / "qoda-backend" else 0o644)
    digest = hashlib.sha256(json.dumps(inventory(destination), sort_keys=True).encode()).hexdigest()[:16]
    return version + "-" + digest


def require_complete_transactions(parent):
    for pattern in [".qoda-stage-*", ".qoda-remove-*"]:
        for directory in parent.glob(pattern):
            if (directory / "recovery-needed").exists():
                raise ValueError(f"Unfinished Qoda transaction; recovery files preserved at {directory}. Review before retrying")


def install(prefix, config):
    anchors = {"prefix": safe_path(prefix), "config": safe_path(config)}
    prefix, config = anchors.values()
    root = safe_path(prefix / "lib/qoda-player")
    payload = integrations(prefix)
    with transaction_lock(prefix):
        require_complete_transactions(root.parent)
        previous = owned(root, anchors)
        # Refuse conflicts before the build or any integration changes.
        if previous is None:
            for domain, files in payload.items():
                for name in files:
                    path = safe_path(anchors[domain] / name)
                    if path.exists():
                        raise ValueError(f"Unowned integration preserved: {path}")
        root.parent.mkdir(parents=True, exist_ok=True)
        with staging(root.parent, ".qoda-stage-") as temporary:
            stage = Path(temporary)
            release = stage / "release"
            release_id = stage_release(release)
            with launcher_module().control_lock():
                if owned(root, anchors) != previous:
                    raise ValueError("Installation changed while building; retry")
                if previous is None:
                    for domain, files in payload.items():
                        for name in files:
                            if safe_path(anchors[domain] / name).exists():
                                raise ValueError("Integration appeared while building; existing file preserved")
                require_stopped()
                new = stage / "qoda-player"
                if previous:
                    shutil.copytree(root, new, symlinks=True)
                    (new / "current").unlink()
                else:
                    (new / "releases").mkdir(parents=True)
                destination = new / "releases" / release_id
                if destination.exists():
                    if inventory(destination) != inventory(release):
                        raise ValueError("Release ID collision; installation preserved")
                else:
                    release.rename(destination)
                (new / "current").symlink_to("releases/" + release_id, target_is_directory=True)
                external = {domain: {name: {"sha256": hashlib.sha256(content).hexdigest(), "mode": mode}
                                     for name, (content, mode) in files.items()} for domain, files in payload.items()}
                (new / RECORD).write_text(json.dumps({"schema": 1, "files": inventory(new), "external": external}, sort_keys=True, indent=2) + "\n")
                backup = stage / "previous"
                undo = []
                installed = False
                marker = prepare_recovery(stage, anchors, previous)
                try:
                    if previous:
                        root.rename(backup)
                    new.rename(root)
                    installed = True
                    for domain, files in payload.items():
                        for name, (content, mode) in files.items():
                            path = safe_path(anchors[domain] / name)
                            old = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) if previous else None
                            undo.append((path, old))
                            atomic_file(path, content, mode)
                    launcher_module().manager("daemon-reload")
                    marker.unlink()
                except BaseException:
                    for path, old in reversed(undo):
                        if old is None:
                            path.unlink(missing_ok=True)
                        else:
                            atomic_file(path, *old)
                    if installed:
                        shutil.rmtree(root)
                    if backup.exists():
                        backup.rename(root)
                    marker.unlink()
                    # Reload restored unit files; preserve the original failure.
                    try:
                        launcher_module().manager("daemon-reload")
                    except (OSError, subprocess.SubprocessError):
                        pass
                    raise
    return root / "current"


def remove(prefix, config):
    anchors = {"prefix": safe_path(prefix), "config": safe_path(config)}
    root = safe_path(anchors["prefix"] / "lib/qoda-player")
    with transaction_lock(anchors["prefix"]), launcher_module().control_lock():
        require_complete_transactions(root.parent)
        metadata = owned(root, anchors)
        if metadata is None:
            raise ValueError("No owned Qoda installation at this prefix")
        require_stopped()
        # Removal is reversible through staging until daemon-reload succeeds.
        with staging(root.parent, ".qoda-remove-") as temporary:
            backup = Path(temporary) / "qoda-player"
            originals = [(safe_path(anchors[domain] / name), None) for domain, files in EXTERNAL.items() for name in files]
            originals = [(path, (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))) for path, _ in originals]
            marker = prepare_recovery(Path(temporary), anchors, metadata)
            root.rename(backup)
            try:
                for path, _ in originals:
                    path.unlink()
                launcher_module().manager("daemon-reload")
                marker.unlink()
            except BaseException:
                backup.rename(root)
                for path, content in originals:
                    atomic_file(path, *content)
                marker.unlink()
                raise
    print("Removed owned code and desktop units. Preferences, session data, cache and keyring sign-in retained.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["install", "remove"])
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    parser.add_argument("--config-home", type=Path, default=Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))))
    args = parser.parse_args()
    try:
        if args.command == "install":
            print("Installed Qoda:", install(args.prefix, args.config_home))
        else:
            remove(args.prefix, args.config_home)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
