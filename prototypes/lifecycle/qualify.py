#!/usr/bin/env python3
"""Clean locked-build reproducibility, then both guarded lifecycle scopes."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

HERE=Path(__file__).resolve().parent
SCRATCH=HERE.parents[1]/'.scratch'
EVIDENCE=HERE/'evidence'
TARGET=SCRATCH/'lifecycle-clean-build'


def main():
    SCRATCH.mkdir(exist_ok=True);EVIDENCE.mkdir(exist_ok=True)
    hashes=[];seconds=[]
    env=dict(os.environ,CARGO_TARGET_DIR=str(TARGET),CARGO_INCREMENTAL='0',CARGO_BUILD_JOBS='2')
    for index in range(2):
        if TARGET.exists():shutil.rmtree(TARGET)
        started=time.monotonic()
        print('Clean locked offline build '+str(index+1)+' / 2',flush=True)
        with (SCRATCH/('lifecycle-build-'+str(index+1)+'.log')).open('w') as log:
            subprocess.run(['cargo','build','--release','--locked','--offline','--manifest-path',str(HERE/'Cargo.toml')],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        hashes.append(hashlib.sha256((TARGET/'release/qoda-lifecycle-prototype').read_bytes()).hexdigest())
        seconds.append(round(time.monotonic()-started,3))
    build={'scope':'two clean offline locked builds, same absolute build root and installed toolchain','sha256':hashes,'seconds':seconds,'identical':hashes[0]==hashes[1],
        'rustc':subprocess.check_output(['rustc','--version'],text=True).strip(),
        'cargo':subprocess.check_output(['cargo','--version'],text=True).strip()}
    (EVIDENCE/'reproducible-build.json').write_text(json.dumps(build,indent=2)+'\n')
    if not build['identical']:raise RuntimeError('clean build artifacts differ')
    print('PASS clean locked build hashes match',flush=True)
    env['QODA_BUILD_TARGET']=str(TARGET)
    subprocess.run([sys.executable,str(HERE/'study.py')],env=env,check=True)
    subprocess.run([sys.executable,str(HERE/'extended.py')],env=env,check=True)
    subprocess.run([sys.executable,str(HERE/'host_portal_smoke.py')],env=env,check=True)
    smoke=json.loads((EVIDENCE/'host-portal-smoke.json').read_text())
    core=json.loads((EVIDENCE/'results.json').read_text());extra=json.loads((EVIDENCE/'extended.json').read_text())
    scopes=[{'scope':s['scope'],'passed':sum(c['pass'] for c in s['checks']),'total':len(s['checks']),'error':s['error'],'retainedOwnedFiles':s['retainedOwnedFiles']} for s in (core,extra)]
    (EVIDENCE/'qualification.json').write_text(json.dumps({'build':build,'scopes':scopes,'hostPortalSmoke':{'pass':smoke['pass'],'warningRetained':bool(smoke['warnings']),'newHyprlandPortalCoredumps':smoke['newHyprlandPortalCoredumps']},'status':'scoped-checks-passed'},indent=2)+'\n')
    print(json.dumps(scopes),flush=True)

if __name__=='__main__':main()
