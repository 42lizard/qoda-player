#!/usr/bin/env python3
import json,math,os,pathlib,subprocess,tempfile,wave,array
HERE=pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix='qoda-alsa-smoke-') as tmp:
 p=pathlib.Path(tmp);w=p/'quiet.wav';samples=[int(2048*math.sin(2*math.pi*997*i/48000)) for i in range(12000) for _ in range(2)];a=array.array('h',samples)
 with wave.open(str(w),'wb') as f:f.setparams((2,2,48000,0,'NONE','not compressed'));f.writeframes(a.tobytes())
 (p/'config.json').write_text(json.dumps({'files':[{'path':str(w),'bits':16,'rate':48000}]}));env=dict(os.environ,LD_PRELOAD=str(HERE/'tap/target/release/libqoda_alsa_tap_prototype.so'),QODA_TAP_DIR=tmp,QODA_TAP_DEVICE='hw:CARD=avsmax98927,DEV=1')
 r=subprocess.run(['python3',str(HERE/'worker.py'),str(p/'config.json'),str(p/'worker.json')],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=12)
 streams=[{'name':x.name,'bytes':x.stat().st_size} for x in p.glob('*.pcm')];data=b''.join(x.read_bytes() for x in sorted(p.glob('*.pcm')) if x.stat().st_size)
 report={'worker_exit':r.returncode,'worker':json.loads((p/'worker.json').read_text()) if (p/'worker.json').exists() else None,'stderr':r.stderr,'streams':streams,'expected_bytes':len(a.tobytes()),'actual_bytes':len(data),'exact':data==a.tobytes(),'logs':{x.name:x.read_text() for x in p.glob('*.jsonl')}}
 (HERE/'evidence/smoke.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report[k] for k in ['worker_exit','streams','expected_bytes','actual_bytes','exact','stderr']}))
