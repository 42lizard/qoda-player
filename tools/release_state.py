"""Conservative release compatibility and owned, paused activation helpers."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import stat
import time

# Include orchestration and models as well as readers: identical version numbers
# alone do not prove compatible representations or migration behavior.
STORAGE_FILES = ("Cargo.toml", "Cargo.lock", "crates/qoda-backend/Cargo.toml",
                 *["crates/qoda-backend/src/" + name for name in (
                     "main.rs", "buffered_reset.rs", "account.rs", "maintenance.rs", "observation.rs", "session_commands.rs", "prepared_playback.rs", "catalog.rs", "storage.rs", "storage/session.rs", "queue.rs", "archive.rs", "archive/store.rs",
                     "cache_files.rs", "gain.rs", "output.rs", "offline.rs", "metadata_cache.rs", "artwork_store.rs")])


def evidence(source):
    paths = list(STORAGE_FILES)
    for crate in ["qoda-core", "qoda-provider"]:
        paths.append("crates/" + crate + "/Cargo.toml")
        paths.extend(str(path.relative_to(source)) for path in sorted((source / "crates" / crate / "src").rglob("*.rs")))
    files = {}
    for name in paths:
        path = source / name
        if path.is_symlink() or not path.is_file():
            raise ValueError("Storage compatibility source is missing or unsafe: " + name)
        files[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    fingerprint = hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()
    return {"schema": 1, "fingerprint": fingerprint, "files": files,
            "session_schemas": [1, 2, 3, 4, 5], "session_payloads": [1, 2, 3, 4, 5],
            "archive_schemas": [1], "catalog_schemas": [1, 2],
            "settings": {"audio.json": [1], "output.json": [1, 2],
                         "cache-settings/artwork.json": [1], "cache-settings/catalog.json": [1]}}


def release_evidence(release):
    path = release / "release.json"
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        raise ValueError("Release has no supported storage compatibility evidence; preserved installation")
    value = json.loads(path.read_text()).get("storage_compatibility")
    if not isinstance(value, dict) or value.get("schema") != 1 or not isinstance(value.get("files"), dict):
        raise ValueError("Release storage compatibility evidence is unsupported")
    fingerprint = hashlib.sha256(json.dumps(value["files"], sort_keys=True).encode()).hexdigest()
    if value.get("fingerprint") != fingerprint or not value["files"]:
        raise ValueError("Release storage compatibility evidence is inconsistent")
    for field in ["session_schemas", "session_payloads", "archive_schemas", "catalog_schemas"]:
        if not isinstance(value.get(field), list) or not value[field] or any(type(n) is not int or n <= 0 for n in value[field]):
            raise ValueError("Release storage format evidence is invalid")
    if not isinstance(value.get("settings"), dict) or not value["settings"] or any(not isinstance(versions,list) or not versions or any(type(n) is not int or n<=0 for n in versions) for versions in value["settings"].values()):
        raise ValueError("Release settings compatibility evidence is invalid")
    return value


def private(path, maximum):
    """Inspect existing data without creating, migrating, chmodding or repairing it."""
    if not path.exists() and not path.is_symlink():
        return False
    for parent in [path.parent, *path.parent.parents]:
        if parent.is_symlink():
            raise ValueError("Symlinked application data preserved: " + str(path))
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > maximum:
        raise ValueError("Unsafe or oversized application data preserved: " + str(path))
    return True


def data_compatible(contract):
    state = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "qoda"
    cache = Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "qoda"
    backups = list((state / "backups").glob("session-*.sqlite3")) + list(state.glob("session-preserved-*.sqlite3"))
    if len(backups)>64:
        raise ValueError("Too many retained recovery copies for bounded compatibility preflight; existing data preserved")
    for path, app, versions, payload in [
        (state / "session.sqlite3", 0x514f4441, contract["session_schemas"], True),
        (state / "archive/visits.sqlite3", 0x514f4841, contract["archive_schemas"], False),
        (cache / "catalog-v1/catalog.sqlite3", 0x514f4443, contract["catalog_schemas"], False),
        *[(path,0x514f4441,contract["session_schemas"],True) for path in backups],
    ]:
        sidecars = [Path(str(path) + suffix) for suffix in ["-journal", "-wal", "-shm"]]
        # Stopped/checkpointed packages use DELETE journals; don't invent recovery
        # of an interrupted or externally supplied WAL profile during a switch.
        if any(p.exists() or p.is_symlink() for p in sidecars):
            raise ValueError("Unresolved database recovery files preserved: " + str(path))
        maximum=(2*1024*1024*1024) if path==cache / "catalog-v1/catalog.sqlite3" else 128*1024*1024
        if not private(path, maximum):
            continue
        with path.open("rb") as stream:
            header=stream.read(100)
        if len(header)<100 or header[:16]!=b"SQLite format 3\0" or header[18:20]!=b"\x01\x01":
            raise ValueError("Unsupported or unresolved SQLite journal format preserved: "+str(path))
        connection = sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)
        try:
            connection.execute("PRAGMA trusted_schema=OFF")
            if connection.execute("PRAGMA application_id").fetchone()[0] != app or connection.execute("PRAGMA user_version").fetchone()[0] not in versions or connection.execute("PRAGMA quick_check(1)").fetchone()[0] != "ok":
                raise ValueError("Unsupported or damaged database preserved: " + str(path))
            if payload:
                size = connection.execute("SELECT length(CAST(payload AS BLOB)) FROM session WHERE slot=1").fetchone()
                if size:
                    if size[0] > 8 * 1024 * 1024:
                        raise ValueError("Oversized session payload preserved: " + str(path))
                    row = connection.execute("SELECT payload FROM session WHERE slot=1").fetchone()
                    version=json.loads(row[0]).get("format")
                    if type(version) is not int or version not in contract["session_payloads"]:
                        raise ValueError("Unsupported session payload preserved: " + str(path))
        except (sqlite3.Error, TypeError, AttributeError, json.JSONDecodeError) as error:
            raise ValueError("Unqualified database payload preserved: " + str(path)) from error
        finally:
            connection.close()
    directory = Path(os.environ.get("XDG_CONFIG_HOME",str(Path.home() / ".config"))) / "qoda"
    for name, versions in contract["settings"].items():
        if name not in {"audio.json", "output.json", "cache-settings/artwork.json", "cache-settings/catalog.json"}:
            raise ValueError("Unknown settings compatibility path; installation preserved")
        path = directory / name
        if private(path, 4096):
            value = json.loads(path.read_text())
            if not isinstance(value, dict) or type(value.get("version")) is not int or value["version"] not in versions:
                raise ValueError("Unsupported settings preserved: " + str(path))
    identities = directory / "source-identities.json"
    if private(identities, 4 * 1024 * 1024):
        import uuid
        value = json.loads(identities.read_text())
        if not isinstance(value, dict) or len(value) > 16384:
            raise ValueError("Unsupported source identities preserved")
        seen = set()
        for key, source in value.items():
            scope = json.loads(key)
            if not isinstance(scope, list) or len(scope) != 3 or any(not isinstance(s, str) or not 0 < len(s.encode()) <= 1024 for s in scope) or json.dumps(scope, separators=(",", ":"), ensure_ascii=False) != key:
                raise ValueError("Unsupported source identity scope preserved")
            if not isinstance(source,str):
                raise ValueError("Unsupported source identity preserved")
            source = uuid.UUID(source)
            if source.int == 0 or source in seen:
                raise ValueError("Ambiguous source identities preserved")
            seen.add(source)


def rollback_compatible(current, target, check_data=True):
    old, new = release_evidence(current), release_evidence(target)
    if old != new:
        raise ValueError("Code rollback refused: storage readers/models or format evidence differ. Newer data and both releases were preserved; no database downgrade was attempted")
    if check_data:
        data_compatible(new)


def ownership(service, config):
    service.verify_ownership(config / "systemd/user")


def stop_checkpointed(service, config):
    running = tuple(service.active(unit) for unit in service.units)
    if any(running):
        ownership(service, config)
    if running[0]:
        service.verify_session()
    elif (Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/backend.sock").exists():
        raise ValueError("An unmanaged backend remains; stop it separately before switching releases")
    if running[1]:
        service.manager("stop", service.units[1])
    if running[0]:
        service.pause_checkpoint()
        result = service.manager("stop", service.units[0], check=False)
        if any(service.active(unit) for unit in service.units):
            raise ValueError("Qoda is still stopping; release switch cancelled")
        service.verify_shutdown()
        if result.returncode != 0:
            raise ValueError("Owned backend stop failed; release switch cancelled")
    if (Path(os.environ["XDG_RUNTIME_DIR"]) / "qoda/backend.sock").exists():
        raise ValueError("Backend socket remains; release switch cancelled")
    return running


def restart_paused(service, root, running):
    if not running[0]:
        return
    service = service.for_release((root / "current").resolve())
    service.manager("start", service.units[0])
    deadline = time.monotonic() + 10
    while True:
        try:
            ready = service.snapshot()
            if ready["intent"] != "paused" or ready.get("storage", {}).get("state") != "saved":
                raise ValueError("Activated backend did not restore a saved paused session")
            archive=ready.get("archive",{})
            if archive.get("available") is not True:
                if archive.get("message","").startswith("Opening") and time.monotonic()<deadline:
                    time.sleep(.05)
                    continue
                raise ValueError("Activated backend archive is unavailable; activation refused")
            audio=ready.get("audio")
            if isinstance(audio,dict) and audio.get("entry_ready") is True and audio.get("paused") is not True:
                raise ValueError("Activated native playback was not paused")
            break
        except OSError:
            if time.monotonic() >= deadline or not service.active(service.units[0]):
                raise ValueError("Activated backend did not become ready") from None
            time.sleep(.05)
    if running[1]:
        service.write_owner(ready["instance"], deadline * 1000)
        service.manager("start", service.units[1])
        while not service.ipc("show"):
            if time.monotonic() >= deadline or not service.active(service.units[1]):
                raise ValueError("Activated UI did not become ready")
            time.sleep(.05)
