#!/usr/bin/env python3
"""Run the manual native study and persist sanitized observations, never input text."""
import json
import os
from pathlib import Path
import subprocess
import time

HERE = Path(__file__).resolve().parent
CONFIG = str(HERE / 'shell.qml')
EVIDENCE = HERE / 'evidence'
LOG = HERE.parents[1] / '.scratch' / 'native-input.log'
LOG.parent.mkdir(exist_ok=True)
EVIDENCE.mkdir(exist_ok=True)

def read(method):
    result = subprocess.run(['quickshell', 'ipc', '-p', CONFIG, 'call', 'inputStudy', method], capture_output=True, text=True, timeout=3)
    if result.returncode:
        raise ValueError('IPC not ready')
    return json.loads(result.stdout)

def save():
    state = read('snapshot')
    review = read('reviews')
    for name, value in [('observations.json', state), ('human-reviews.json', review)]:
        path = EVIDENCE / name
        tmp = path.with_suffix('.tmp')
        tmp.write_text(json.dumps(value, indent=2) + '\n')
        tmp.replace(path)

with LOG.open('w') as log:
    env = dict(os.environ, QT_QUICK_CONTROLS_STYLE='Basic')
    child = subprocess.Popen(['quickshell', '-p', CONFIG, '--no-duplicate', '--no-color'], env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        for _ in range(100):
            if child.poll() is not None:
                raise RuntimeError('Study startup failed; inspect ' + str(LOG))
            try:
                save()
                break
            except ValueError:
                time.sleep(.1)
        else:
            raise RuntimeError('IPC did not become ready')
        print('Qoda physical input checklist is open. Click Start / Prepare and follow the native instructions. No audio.', flush=True)
        while child.poll() is None:
            try:
                save()
            except (ValueError, subprocess.TimeoutExpired):
                pass
            time.sleep(.5)
    except KeyboardInterrupt:
        pass
    finally:
        if child.poll() is None:
            subprocess.run(['quickshell', 'ipc', '-p', CONFIG, 'call', 'inputStudy', 'quit'], capture_output=True, timeout=3)
            child.wait(timeout=5)
