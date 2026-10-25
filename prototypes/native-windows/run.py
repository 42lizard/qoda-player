#!/usr/bin/env python3
"""THROWAWAY launcher: one command opens the study, --check records IPC evidence."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check',action='store_true')
args=parser.parse_args()
here=Path(__file__).resolve().parent
config=str(here/'shell.qml')
logdir=here.parents[1]/'.scratch'
logdir.mkdir(exist_ok=True)
env=os.environ.copy();env['QT_QUICK_CONTROLS_STYLE']='Basic'
with (logdir/'native-prototype.log').open('w') as log:
    child=subprocess.Popen(['quickshell','-p',config,'--no-duplicate','--no-color'],env=env,stdout=log,stderr=subprocess.STDOUT)
    try:
        ready=False
        for _ in range(100):
            if child.poll() is not None: break
            result=subprocess.run(['quickshell','ipc','-p',config,'call','probe','snapshot'],capture_output=True,text=True)
            try:
                json.loads(result.stdout);ready=True;break
            except ValueError:
                time.sleep(.05)
        if not ready:
            raise RuntimeError('Prototype did not start; inspect '+str(logdir/'native-prototype.log'))
        if args.check:
            subprocess.run(['python3',str(here/'check.py')],check=True)
            child.wait(timeout=5)
            warnings=[line.replace(config,'<prototype>') for line in (logdir/'native-prototype.log').read_text().splitlines() if 'WARN' in line or 'ERROR' in line]
            (here/'evidence'/'startup-warnings.txt').write_text('\n'.join(warnings)+'\n')
        else:
            subprocess.run(['quickshell','ipc','-p',config,'call','probe','showFull'],check=True)
            print('Qoda native study is open. Quit closes this prototype; Ctrl+C also stops it.',flush=True)
            child.wait()
    except KeyboardInterrupt:
        pass
    finally:
        if child.poll() is None:
            subprocess.run(['quickshell','ipc','-p',config,'call','probe','quit'],capture_output=True,timeout=3)
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired: child.terminate()
