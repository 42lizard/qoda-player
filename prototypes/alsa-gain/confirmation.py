#!/usr/bin/env python3
"""Resolve the timing ambiguity with a long current occurrence, plus mixer guard."""
import hashlib,json,os,pathlib,subprocess,tempfile
from run import fixture,run_case,HERE
root=pathlib.Path(tempfile.mkdtemp(prefix='qoda-alsa-confirmation-'));os.chmod(root,0o700)
a=fixture(root,'current-long',frames=48000)
b=fixture(root,'removed-upcoming',frames=14400,seed=70000,flac=True)
c=fixture(root,'surviving-upcoming',frames=12000,seed=93000,flac=True)
def mixer():return subprocess.check_output(['amixer','-c','avsmax98927','scontents'],stderr=subprocess.DEVNULL)
before=mixer()
case=run_case(root,'early-accepted-edit',[a,b,c],'off',actions=[{'at':.04,'command':['playlist-remove','1']}],expected_files=[a,c],expected_ends=2)
after=mixer()
events=[e for s in case['streams'] for e in s['events']];states=[e for e in events if e['event']=='pcm-state'];lastframe=next((e['accepted_total'] for e in reversed(events) if e['event']=='submit'),0)
report={'case':case,'mixer_unchanged':before==after,'mixer_values_exported':False,'driver_states':states,'xruns_before_all_frames_submitted':[e for e in states if e['state']==4 and e['accepted_total']<lastframe],'limits':['Only this targeted native case has an explicit before/after hardware mixer comparison','The earlier short fixture removed an item after the engine had advanced and does not establish an early-edit failure','Accepted PCM is distinct from heard/consumed PCM; presentation-boundary mapping needs its own decision'],'scratch':str(root)}
(HERE/'evidence/confirmation.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'exact':case['exact'],'mixer_unchanged':report['mixer_unchanged'],'states':states,'midstream_xruns':report['xruns_before_all_frames_submitted']}),flush=True)
