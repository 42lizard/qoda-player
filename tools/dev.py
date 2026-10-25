#!/usr/bin/env python3
"""Run the development app; never changes desktop configuration or autoplays music."""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
SOCKET = Path(os.environ['XDG_RUNTIME_DIR']) / 'qoda/backend.sock'

def request(op):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(3)
        s.connect(str(SOCKET))
        f = s.makefile('rwb')
        f.write(b'{"op":"hello","protocol":1}\n'); f.flush()
        state = json.loads(f.readline(2**20))
        if op != 'hello':
            f.write((json.dumps({'op': op, 'instance': state['instance']})+'\n').encode()); f.flush()
        return state

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--use-validation-login', action='store_true', help='Explicitly reuse the existing local validation keyring sign-in, session only')
    parser.add_argument('--quit', action='store_true')
    parser.add_argument('--restore-backup', action='store_true', help='With Qoda stopped, restore the latest session backup while preserving the original database')
    parser.add_argument('--backend-only', action='store_true')
    args = parser.parse_args()
    if args.quit:
        try: request('quit')
        except OSError: pass
        return
    subprocess.run(['cargo', 'build', '--locked', '-p', 'qoda-backend'], cwd=ROOT, check=True)
    if args.restore_backup:
        subprocess.run([str(ROOT / 'target/debug/qoda-backend'), '--restore-backup'], check=True)
        return
    backend = None
    ui = None
    try:
        try: request('hello')
        except OSError:
            credential = None
            if args.use_validation_login:
                result = subprocess.run(['/usr/bin/secret-tool', 'lookup', 'application', 'qoda-plex-legacy-validation'], capture_output=True, timeout=30, check=True)
                prefix = b'qoda-legacy-credential-v1:'
                value = result.stdout.strip()
                if not value.startswith(prefix): raise RuntimeError('Validation sign-in is unavailable')
                credential = json.dumps(json.loads(base64.b64decode(value[len(prefix):], validate=True))).encode()
                del value, result
            command = [str(ROOT / 'target/debug/qoda-backend')]
            if credential: command.append('--credential-stdin')
            backend = subprocess.Popen(command, stdin=subprocess.PIPE if credential else subprocess.DEVNULL)
            if credential:
                backend.stdin.write(credential); backend.stdin.close(); del credential
            for _ in range(100):
                if backend.poll() is not None: raise RuntimeError('Backend exited during startup')
                try: request('hello'); break
                except OSError: time.sleep(.1)
            else: raise RuntimeError('Backend did not become ready')
        if args.backend_only:
            if backend: backend.wait()
            return
        existing_ui = subprocess.run(['quickshell', 'ipc', '--path', str(ROOT / 'ui'), 'call', '--', 'control', 'show'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
        if existing_ui and backend is None:
            return
        if not existing_ui:
            ui = subprocess.Popen(['quickshell', '--path', str(ROOT / 'ui'), '--no-duplicate'])
        restarts = 0
        while True:
            if backend is not None and backend.poll() is not None:
                break
            try:
                request('hello')
            except OSError:
                break
            if ui is not None and ui.poll() is not None:
                # Keep the independently running backend alive during UI recovery.
                time.sleep(.3)
                try: request('hello')
                except OSError: break
                if restarts < 3:
                    restarts += 1
                    ui = subprocess.Popen(['quickshell', '--path', str(ROOT / 'ui'), '--no-duplicate'])
                else:
                    print('The UI exited repeatedly. Backend remains running; press Ctrl+C to quit.')
                    ui = None
            time.sleep(.5)
    except KeyboardInterrupt:
        pass
    finally:
        if backend and backend.poll() is None:
            try: request('quit'); backend.wait(timeout=8)
            except (OSError, subprocess.TimeoutExpired): backend.terminate(); backend.wait(timeout=8)
        if ui and ui.poll() is None:
            ui.terminate(); ui.wait(timeout=5)

if __name__ == '__main__':
    main()
