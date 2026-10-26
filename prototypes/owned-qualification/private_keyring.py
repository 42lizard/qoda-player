#!/usr/bin/env python3
"""Synthetic Secret Service test on a private bus and filesystem namespace."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


def worker(root):
    import dbus
    bus = dbus.SessionBus()
    driver = dbus.Interface(bus.get_object('org.freedesktop.DBus', '/org/freedesktop/DBus'), 'org.freedesktop.DBus')
    attrs = ['application', 'qoda-private-secret-qualification', 'run', str(uuid.uuid4())]
    marker = b'fictional-qoda-credential-marker'
    facts = {'desktop_bus_unavailable_in_namespace': not Path('/run/user/1000/bus').exists()}
    def tool(op, data=None, env=None):
        return subprocess.run(['/usr/bin/secret-tool', op, *attrs], input=data,
                              capture_output=True, timeout=3, env=env)
    absent = tool('lookup')
    facts['private_bus_without_service_refuses_lookup'] = absent.returncode != 0
    bad_env = dict(os.environ, DBUS_SESSION_BUS_ADDRESS='unix:path=' + str(root / 'missing-bus'))
    facts['invalid_bus_refuses_lookup'] = tool('lookup', env=bad_env).returncode != 0
    daemon = None
    password = b'fictional-private-fixture-password'
    def start():
        process = subprocess.Popen(['/usr/bin/gnome-keyring-daemon', '--foreground', '--components=secrets',
                                    '--control-directory=' + str(root / 'control'), '--unlock'],
                                   stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        process.stdin.write(password)
        process.stdin.close()
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('owned-daemon-exited')
            try:
                if int(driver.GetConnectionUnixProcessID('org.freedesktop.secrets')) == process.pid:
                    return process
            except dbus.DBusException:
                pass
            time.sleep(.05)
        process.kill()
        process.wait(timeout=3)
        raise RuntimeError('owned-service-not-ready')
    def stop(process):
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
        deadline = time.monotonic() + 3
        while driver.NameHasOwner('org.freedesktop.secrets') and time.monotonic() < deadline:
            time.sleep(.02)
        assert not driver.NameHasOwner('org.freedesktop.secrets')
    def service():
        return dbus.Interface(bus.get_object('org.freedesktop.secrets', '/org/freedesktop/secrets'), 'org.freedesktop.Secret.Service')
    try:
        daemon = start()
        facts['owned_service_pid_verified'] = int(driver.GetConnectionUnixProcessID('org.freedesktop.secrets')) == daemon.pid
        stored = subprocess.run(['/usr/bin/secret-tool', 'store', '--label=Qoda synthetic fixture', *attrs],
                                input=marker, capture_output=True, timeout=3)
        assert stored.returncode == 0
        facts['initial_roundtrip'] = tool('lookup').stdout.strip() == marker
        svc = service()
        collection = svc.ReadAlias('default')
        assert str(collection).startswith('/org/freedesktop/secrets/collection/')
        unlocked, locked = svc.SearchItems(dict(zip(attrs[::2], attrs[1::2])))
        assert len(unlocked) == 1 and not locked
        _, session = svc.OpenSession('plain', dbus.String('', variant_level=1))
        targets, prompt = svc.Lock([collection])
        assert str(prompt) == '/' and list(targets) == [collection]
        properties = dbus.Interface(bus.get_object('org.freedesktop.secrets', collection), 'org.freedesktop.DBus.Properties')
        facts['private_collection_locked'] = bool(properties.Get('org.freedesktop.Secret.Collection', 'Locked'))
        try:
            dbus.Interface(bus.get_object('org.freedesktop.secrets', unlocked[0]), 'org.freedesktop.Secret.Item').GetSecret(session)
            facts['locked_read_refused'] = False
        except dbus.DBusException as error:
            facts['locked_read_refused'] = error.get_dbus_name() == 'org.freedesktop.Secret.Error.IsLocked'
        dbus.Interface(bus.get_object('org.freedesktop.secrets', session), 'org.freedesktop.Secret.Session').Close()
        stop(daemon)
        daemon = None
        # Restart only our foreground daemon; --unlock against the running daemon
        # did not actually unlock in the older retained experiment.
        daemon = start()
        collection = service().ReadAlias('default')
        properties = dbus.Interface(bus.get_object('org.freedesktop.secrets', collection), 'org.freedesktop.DBus.Properties')
        facts['restart_unlock_confirmed'] = not bool(properties.Get('org.freedesktop.Secret.Collection', 'Locked'))
        facts['persistent_marker_restored'] = tool('lookup').stdout.strip() == marker
        replacement = marker + b'-replacement'
        result = subprocess.run(['/usr/bin/secret-tool', 'store', '--label=Qoda synthetic fixture', *attrs],
                                input=replacement, capture_output=True, timeout=3)
        facts['replace_roundtrip'] = result.returncode == 0 and tool('lookup').stdout.strip() == replacement
        facts['scoped_delete_succeeded'] = tool('clear').returncode == 0
        remaining = service().SearchItems(dict(zip(attrs[::2], attrs[1::2])))
        facts['scoped_item_absent'] = not remaining[0] and not remaining[1]
        assert all(facts.values())
        print(json.dumps({'completed': True, 'facts': facts,
                          'limits': ['Synthetic credential only', 'Private daemon restart unlock; not desktop PAM or user prompt',
                                     'No desktop keyring operation']}))
    finally:
        if daemon:
            stop(daemon)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--worker':
        worker(Path(sys.argv[2]))
        return
    with tempfile.TemporaryDirectory(prefix='qoda-private-secret-') as temporary:
        root = Path(temporary)
        for name in ['data/keyrings', 'config', 'state', 'cache', 'runtime', 'control']:
            (root / name).mkdir(parents=True, mode=0o700, exist_ok=True)
        shutil.copyfile(__file__, root / 'worker.py')
        (root / 'bus.xml').write_text('''<busconfig><type>session</type><listen>unix:tmpdir=''' + str(root / 'runtime') + '''</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
        command = ['/usr/bin/bwrap', '--unshare-all', '--die-with-parent', '--new-session', '--ro-bind', '/', '/',
                   '--proc', '/proc', '--dev', '/dev', '--tmpfs', str(Path.home()), '--tmpfs', '/run',
                   '--bind', str(root), str(root), '--chdir', str(root), '--clearenv', '--setenv', 'PATH', '/usr/bin',
                   '--setenv', 'LANG', 'C.UTF-8']
        for key, value in [('XDG_DATA_HOME', 'data'), ('XDG_CONFIG_HOME', 'config'), ('XDG_STATE_HOME', 'state'),
                           ('XDG_CACHE_HOME', 'cache'), ('XDG_RUNTIME_DIR', 'runtime')]:
            command += ['--setenv', key, str(root / value)]
        command += ['/usr/bin/dbus-run-session', '--config-file=' + str(root / 'bus.xml'), '--',
                    '/usr/bin/python3', str(root / 'worker.py'), '--worker', str(root)]
        result = subprocess.run(command, capture_output=True, timeout=45)
        if result.returncode:
            # Error classes/known fixture frames only; never print environment.
            print(json.dumps({'completed': False, 'exit_code': result.returncode,
                              'error_tail': result.stderr.decode(errors='replace')[-1400:]}))
            raise SystemExit(1)
        evidence = json.loads(result.stdout)
        path = Path(__file__).resolve().parent / 'private-keyring-evidence.json'
        path.write_text(json.dumps(evidence, indent=2) + '\n')
        print(json.dumps(evidence))


if __name__ == '__main__':
    main()
