#!/usr/bin/env python3
"""One bounded clock/payload series. No services, browser server or persistence."""
import json
import argparse
import time
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import wave
HERE=Path(__file__).resolve().parent

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--modes',nargs='+',default=['alsa','null','pipewire']);parser.add_argument('--output',default='clock-traces.json');parser.add_argument('--edits',action='store_true');parser.add_argument('--names',nargs='*');args=parser.parse_args()
 subprocess.run(['cargo','build','--release','--manifest-path',str(HERE/'tap/Cargo.toml')],check=True)
 root=Path(tempfile.mkdtemp(prefix='qoda-clock-'));root.chmod(0o700)
 results=[]
 for mode in args.modes:
  if mode=='alsa':
   from preflight import probe
   until=time.monotonic()+6
   while True:
    supported=probe(48000,'S16_LE',True)
    if supported['supported'] or time.monotonic()>=until:break
    time.sleep(.2)
   if not supported['supported']:raise RuntimeError('Native device unavailable; no fallback: '+json.dumps(supported))
  cases=[('short',[.25,.30,.20],'weak',[]),('tiny',[.025,.030,.020],'weak',[]),('long',[1.5,.3,.2],'weak',[]),('drained',[.25,.3,.2],'no',[]),('pause-tail',[.25,.3,.2],'weak',[{'engine_index':1,'command':['set','pause','yes']},{'after':.5,'command':['set','pause','no']}])]
  if args.edits:cases=[('early',[1.5,.8,.3],'weak',[{'after':.08,'engine_index':0,'command':['playlist-remove','1']}]),('buffered-remove',[1.5,.8,.3],'weak',[{'engine_index':1,'pts_negative':True,'rebuild':'upcoming'}]),('accepted-remove',[1.5,.8,.3],'weak',[{'engine_index':1,'pts_min':-.06,'pts_negative':True,'rebuild':'upcoming'}]),('late-remove',[1.5,.8,.3],'weak',[{'engine_index':1,'pts_min':.04,'rebuild':'current'}]),('seek',[2,.8,.3],'weak',[{'after':.1,'engine_index':0,'command':['seek','1.0','absolute+exact']}])]
  for name,lengths,gapless,actions in cases:
   if args.names and name not in args.names:continue
   folder=root/(mode+'-'+name);folder.mkdir();files=[]
   for index,seconds in enumerate(lengths):
    file=folder/(chr(65+index)+'.wav')
    with wave.open(str(file),'wb') as w:
     w.setparams((2,2,48000,0,'NONE','not compressed'))
     w.writeframes(b''.join(struct.pack('<hh',v,-v) for i in range(round(seconds*48000)) for v in [((i*17+index*43)%61)-30]))
    files.append(str(file))
   config={'mode':mode,'files':files,'exact_durations':lengths,'gapless':gapless,'actions':actions,'expected_ends':2 if name=='early' else 4 if name in ['buffered-remove','accepted-remove'] else 3,'timeout':6}
   (folder/'config.json').write_text(json.dumps(config))
   env=dict(os.environ,LD_PRELOAD=str(HERE/'tap/target/release/libqoda_timing_tap_prototype.so'),QODA_TAP_DEVICE='hw:CARD=avsmax98927,DEV=1',QODA_TAP_DIR=str(folder))
   p=subprocess.run(['python3',str(HERE/'probe.py'),str(folder/'config.json'),str(folder/'probe.json')],env=env,capture_output=True,text=True,timeout=9)
   result=json.loads((folder/'probe.json').read_text());result.update(name=name,lengths=lengths,gapless=gapless,exit=p.returncode,stderr=p.stderr)
   result['tap']=[json.loads(line) for file in sorted(folder.glob('stream-*.jsonl')) for line in file.read_text().splitlines()]
   results.append(result)
   first={}
   for r in result.get('clocks',[]):
    if r['coherent'] and r['audio_pts'] is not None:first.setdefault(str(r['index']),{'pts':round(r['audio_pts'],5),'mono_ns':r['mono_ns']})
   print(json.dumps({'mode':mode,'name':name,'first_audio_pts':first,'error':result.get('error'),'elapsed':result.get('elapsed')}),flush=True)
 summary={'boundary':'audio-pts is nominal driver-delay-adjusted position, not downstream audibility','cases':results}
 text=json.dumps(summary,indent=2).replace(str(root),'<scratch>')
 (HERE/'evidence'/args.output).write_text(text+'\n')
 (HERE/'evidence'/(args.output+'.gz')).unlink(missing_ok=True)
 local=HERE.parents[1]/'.scratch/timing-roots.json';local.parent.mkdir(exist_ok=True)
 roots=json.loads(local.read_text()) if local.exists() else {};roots[args.output]=str(root)
 local.write_text(json.dumps(roots))
 print('Private raw fixtures: '+str(root),flush=True)
if __name__=='__main__':main()
