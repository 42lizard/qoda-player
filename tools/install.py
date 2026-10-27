#!/usr/bin/env python3
"""Install stopped code, explicitly checkpoint/update, roll back, or remove Qoda."""
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
    recovery_record(stage / "recovery-before.json", (json.dumps(previous,sort_keys=True)+"\n").encode())
    marker = stage / "recovery-needed"
    recovery_record(marker,b"Retained previous payload and external files for reviewed recovery. Do not delete until repaired.\n")
    return marker


def sync_directory(path):
    fd=os.open(path,os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:os.fsync(fd)
    finally:os.close(fd)


def recovery_record(path, content):
    fd=os.open(path,os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,"wb") as stream:
        stream.write(content);stream.flush();os.fsync(stream.fileno())
    sync_directory(path.parent)


def exchange_paths(first, second):
    # Linux rename exchange keeps the selected tree continuously present, even
    # for a wrapper that opens current before acquiring its activation lock.
    import ctypes
    library=ctypes.CDLL(None,use_errno=True)
    rename=library.renameat2
    rename.argtypes=[ctypes.c_int,ctypes.c_char_p,ctypes.c_int,ctypes.c_char_p,ctypes.c_uint]
    rename.restype=ctypes.c_int
    if rename(-100,os.fsencode(first),-100,os.fsencode(second),2)!=0:
        error=ctypes.get_errno()
        raise OSError(error,os.strerror(error))


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
        sync_directory(path.parent)
    finally:
        Path(temporary).unlink(missing_ok=True)


def integrations(prefix, *, packaged=False):
    files = launch_module().integration_files(prefix, packaged=packaged)
    files["prefix"]["share/icons/hicolor/scalable/apps/qoda-player.svg"] = ((ROOT / "packaging/qoda-player.svg").read_bytes(), 0o644)
    return files


def launch_module():
    spec = importlib.util.spec_from_file_location("qoda_launcher", ROOT / "tools/launch.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def launcher_module():
    return launch_module().Launch()


def release_state_module():
    spec = importlib.util.spec_from_file_location("qoda_release_state", Path(__file__).with_name("release_state.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def storage_evidence():
    return release_state_module().evidence(ROOT)


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
    if any(module.active(unit) for unit in module.units):
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
    contract=storage_evidence()
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
    if storage_evidence()!=contract:
        raise ValueError("Storage sources changed while building; retry from a stable checkout")
    (destination / "release.json").write_text(json.dumps({"version": version, "protocol": 1, "rustc": toolchain, "storage_compatibility": contract}, indent=2) + "\n")
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


def install(prefix, config, checkpointed=False, rollback_release=None):
    checkpointed = checkpointed or rollback_release is not None
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
            compatibility = release_state_module()
            if rollback_release is not None:
                if previous is None or rollback_release in {"", ".", ".."} or Path(rollback_release).name != rollback_release:
                    raise ValueError("Choose an exact retained release ID from this installation")
                target = root / "releases" / rollback_release
                if not target.is_dir():
                    raise ValueError("Retained release is unavailable; installation preserved")
                compatibility.rollback_compatible((root / "current").resolve(), target, check_data=False)
                shutil.copytree(target, release)
                release_id = rollback_release
            else:
                release_id = stage_release(release)
            contract = compatibility.release_evidence(release)
            service = launcher_module()
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
            with service.control_lock():
                if owned(root, anchors) != previous:
                    raise ValueError("Installation changed while building; retry")
                if previous is None:
                    for domain, files in payload.items():
                        for name in files:
                            if safe_path(anchors[domain] / name).exists():
                                raise ValueError("Integration appeared while building; existing file preserved")
                running = compatibility.stop_checkpointed(service, config) if checkpointed else (False, False)
                if not checkpointed:
                    require_stopped()
                compatibility.data_compatible(contract)
                backup = stage / "previous"
                undo = []
                installed = False
                activation_started = False
                marker = prepare_recovery(stage, anchors, previous)
                try:
                    if previous:
                        exchange_paths(root,new)
                        installed = True
                        sync_directory(root.parent)
                        sync_directory(stage)
                        new.rename(backup)
                        sync_directory(stage)
                    else:
                        new.rename(root)
                        installed = True
                        sync_directory(root.parent)
                    for domain, files in payload.items():
                        for name, (content, mode) in files.items():
                            path = safe_path(anchors[domain] / name)
                            old = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) if previous else None
                            undo.append((path, old))
                            atomic_file(path, content, mode)
                    service.manager("daemon-reload")
                    if checkpointed and running[0]:
                        activation_started = True
                        compatibility.restart_paused(service, root, running)
                    marker.unlink()
                except BaseException as activation_error:
                    safe_restart = True
                    if activation_started:
                        service.manager("stop", *reversed(service.units), check=False)
                        if any(service.active(unit) for unit in service.units):
                            raise ValueError("Activation failed and new services could not stop; code and recovery files retained at " + str(stage)) from activation_error
                        try:
                            compatibility.rollback_compatible((root / "current").resolve(), (backup / "current").resolve())
                        except (OSError, ValueError):
                            safe_restart = False
                    for path, old in reversed(undo):
                        if old is None:
                            path.unlink(missing_ok=True)
                        else:
                            atomic_file(path, *old)
                    if installed and previous:
                        before=backup if backup.exists() else new
                        exchange_paths(root,before)
                        if safe_restart:
                            shutil.rmtree(before)
                        else:
                            before.rename(stage / "failed-release")
                    elif installed:
                        shutil.rmtree(root)
                    # New code may already have migrated data. Keep both payloads
                    # and block launches if identical-reader evidence is absent.
                    if not safe_restart:
                        try:
                            service.manager("daemon-reload")
                        except (OSError, subprocess.SubprocessError):
                            pass
                        raise ValueError("Activation failed; previous code restored but restart refused because data compatibility is unproven. Newer data and failed release retained at " + str(stage)) from activation_error
                    try:
                        service.manager("daemon-reload")
                        if checkpointed:
                            compatibility.restart_paused(service, root, running)
                    except (OSError, subprocess.SubprocessError, ValueError):
                        if checkpointed:
                            raise ValueError("Previous code restored; restart/reload needs recovery at " + str(stage)) from activation_error
                    marker.unlink()
                    raise
    return root / "current"


def recover(prefix, config, directory):
    """Restore verified previous code only; never restore/downgrade user data."""
    anchors={"prefix":safe_path(prefix),"config":safe_path(config)}
    root=safe_path(anchors["prefix"] / "lib/qoda-player")
    stage=safe_path(directory)
    if stage.parent!=root.parent or not stage.name.startswith(".qoda-stage-") or not (stage / "recovery-needed").is_file():
        raise ValueError("Choose the exact marked recovery directory under this prefix lib directory")
    with transaction_lock(anchors["prefix"]), launcher_module().control_lock():
        require_stopped()
        saved_anchors={domain:stage / "external" / domain for domain in EXTERNAL}
        regular_hash(stage / "recovery-before.json")
        expected=json.loads((stage / "recovery-before.json").read_text())
        previous=None
        for location in [stage / "previous",stage / "qoda-player",root]:
            location=safe_path(location)
            if location.is_dir():
                regular_hash(location / RECORD)
                if json.loads((location / RECORD).read_text())==expected and owned(location,saved_anchors)==expected:
                    previous=location
                    break
        if previous is None or expected is None:
            raise ValueError("Verified previous payload is unavailable; retain recovery files for manual review")
        record=expected
        candidate=next((safe_path(path) for path in [stage / "failed-release",stage / "recovered-candidate",root] if path.is_dir()),None)
        if candidate is None:
            raise ValueError("Current candidate is unavailable; recovery files preserved")
        regular_hash(candidate / RECORD)
        selected=json.loads((candidate / RECORD).read_text())
        if selected.get("schema")!=1 or selected.get("files")!=inventory(candidate):
            raise ValueError("Current candidate changed; recovery files preserved")
        compatibility=release_state_module()
        compatibility.rollback_compatible((candidate / "current").resolve(),(previous / "current").resolve())
        for domain,names in EXTERNAL.items():
            for name in names:
                path=safe_path(anchors[domain] / name)
                if path.exists() and regular_hash(path) not in [record["external"][domain][name],selected["external"][domain][name]]:
                    raise ValueError("External file changed after interruption; preserved " + str(path))
        displaced=stage / "recovered-candidate"
        if previous!=root and displaced.exists():
            raise ValueError("A recovery candidate already exists; retain files for manual review")
        try:
            if previous!=root:
                exchange_paths(root,previous)
                sync_directory(root.parent)
                sync_directory(stage)
                previous.rename(displaced)
            for domain,names in EXTERNAL.items():
                for name in names:
                    source=saved_anchors[domain] / name
                    atomic_file(anchors[domain] / name,source.read_bytes(),stat.S_IMODE(source.stat().st_mode))
            launcher_module().manager("daemon-reload")
            owned(root,anchors)
            (stage / "recovery-needed").unlink()
            (stage / "recovery-complete").write_text("Previous code restored; all units remain stopped. User data was not restored. Retained candidate is available for review.\n")
        except BaseException:
            # Preserve every candidate and the marker. A partially repaired
            # external set is intentionally not treated as a completed recovery.
            raise
    print("Previous code restored; services remain stopped and data unchanged. Retained code copies:",stage)


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
    parser.add_argument("command", choices=["install", "update", "rollback", "recover", "remove"])
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
    parser.add_argument("--config-home", type=Path, default=Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))))
    parser.add_argument("--release", help="Exact retained release ID for explicit compatible code rollback")
    parser.add_argument("--recovery-dir", type=Path, help="Exact marked recovery directory reported by an interrupted switch")
    args = parser.parse_args()
    try:
        if args.recovery_dir is not None and args.command != "recover":
            raise ValueError("--recovery-dir is only accepted by recover")
        if args.release is not None and args.command != "rollback":
            raise ValueError("--release is only accepted by rollback")
        if args.command in {"install", "update", "rollback"}:
            if (args.command == "rollback") != (args.release is not None):
                raise ValueError("Only rollback requires --release with an exact retained release ID")
            print("Selected Qoda release:", install(args.prefix, args.config_home, checkpointed=args.command != "install", rollback_release=args.release))
        elif args.command == "recover":
            if args.recovery_dir is None or args.release is not None:
                raise ValueError("recover requires --recovery-dir and never restores database backups")
            recover(args.prefix,args.config_home,args.recovery_dir)
        else:
            remove(args.prefix, args.config_home)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
