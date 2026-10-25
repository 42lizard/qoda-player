#!/usr/bin/env python3
"""Check the decoded source descriptor, rather than trusting fixture/provider hints."""
import array,json,os,pathlib,tempfile,wave
from run import fixture,run_case,HERE,samples
root=pathlib.Path(tempfile.mkdtemp(prefix='qoda-source-guard-'));os.chmod(root,0o700)
valid=fixture(root,'verified-source',flac=True)
valid_wav=fixture(root,'verified-wav')
misleading=fixture(root,'actual24-hint16',bits=24);misleading['bits']=16
mono=fixture(root,'mono-hint',frames=12000);values=samples(12000)[::2]
with wave.open(mono['path'],'wb') as w:w.setparams((1,2,48000,0,'NONE','not compressed'));w.writeframes(array.array('h',values).tobytes())
mono['samples']=values
cases=[run_case(root,'descriptor-native-valid',[valid]),run_case(root,'descriptor-native-wav',[valid_wav]),run_case(root,'descriptor-rejects-precision-hint',[misleading]),run_case(root,'descriptor-rejects-mono',[mono])]
(HERE/'evidence/source-guard.json').write_text(json.dumps({'cases':cases,'scope':'Local controlled descriptor extraction before ALSA open; no trust in supplied format hints','scratch':str(root)},indent=2)+'\n')
