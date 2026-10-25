#!/usr/bin/env python3
"""Native seek/repeat/failure/control observations after the initial sample matrix."""
import array,ctypes as C,json,os,pathlib,socket,subprocess,sys,tempfile,time
from run import fixture,gain,run_case,HERE
from preflight import A,DEVICE
BUS='org.mpris.MediaPlayer2.qoda_alsa_qualification'

def actual(directory):
 values=[]
 for f in sorted(directory.glob('stream-*.pcm')):
  a=array.array('h');a.frombytes(f.read_bytes());values.extend(a)
 return values

def compare(a,b):return {'same_count':len(a)==len(b),'max_error_lsb':max((abs(x-y) for x,y in zip(a,b)),default=0),'actual_samples':len(a),'expected_samples':len(b)}

def policy(root,name,file,operation):
 d=root/name;d.mkdir();sock=d/'control.sock';pol=gain(file,'track');config={'files':[{k:v for k,v in file.items() if k in ['path','bits','rate']}],'control_path':str(sock)};config['files'][0]['filter']=pol['filter'];(d/'config.json').write_text(json.dumps(config));env=dict(os.environ,LD_PRELOAD=str(HERE/'tap/target/release/libqoda_alsa_tap_prototype.so'),QODA_TAP_DIR=str(d),QODA_TAP_DEVICE=DEVICE)
 if operation=='device-loss':config['injected_device_loss']=True;(d/'config.json').write_text(json.dumps(config));env['QODA_TAP_LOSS_AFTER']='9600'
 p=subprocess.Popen(['python3',str(HERE/'worker.py'),str(d/'config.json'),str(d/'worker.json')],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 responses=[]
 def rpc(data):
  with socket.socket(socket.AF_UNIX) as s:s.settimeout(2);s.connect(str(sock));s.sendall(json.dumps(data).encode()+b'\n');return json.loads(s.recv(8192))
 try:
  end=time.monotonic()+4
  while time.monotonic()<end:
   if sock.exists():
    try:
     s=rpc({'op':'snapshot'})
     if (s.get('position') or 0)>.03:break
    except (OSError,ValueError):pass
   if p.poll() is not None:break
   time.sleep(.02)
  if operation!='device-loss':
   before=rpc({'op':'snapshot'});responses.append({'before':before})
   if operation=='volume':responses.append({'valid_prepare':rpc({'op':'complete_prepare','generation':before['generation']})})
   if operation=='volume':
    responses.append({'app_volume':rpc({'op':'set_volume','value':.5})})
    for value in ['0.5','0.0','2.0','1.0']:
     r=subprocess.run(['busctl','--user','set-property',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','Volume','d',value],text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=2)
     responses.append({'mpris_value':value,'exit':r.returncode,'unsupported_error':'locked at unity' in r.stderr})
    read=subprocess.check_output(['busctl','--user','get-property',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','Volume'],text=True).strip();responses.append({'mpris_reported':read,'native_after':rpc({'op':'snapshot'})})
    previous=rpc({'op':'snapshot'})['generation'];responses.append({'pause':rpc({'op':'pause'})});time.sleep(.08);responses.append({'stale_completion':rpc({'op':'complete_prepare','generation':previous}),'after_pause':rpc({'op':'snapshot'})});responses.append({'mode_change':rpc({'op':'set_mode','mode':'desktop'})})
   elif operation=='device-switch':responses.append({'device_change':rpc({'op':'set_device','device':'hw:CARD=qoda_missing,DEV=0'})})
   elif operation=='mode-switch':responses.append({'mode_change':rpc({'op':'set_mode','mode':'desktop'})})
  p.wait(timeout=5)
 finally:
  if p.poll() is None:p.terminate();p.wait(timeout=3)
 worker=json.loads((d/'worker.json').read_text());values=actual(d);ref=[round(x*pol['factor']) for x in file['samples']];events=[json.loads(x) for f in d.glob('stream-*.jsonl') for x in f.read_text().splitlines()];routes=[json.loads(x) for x in (d/'routes.jsonl').read_text().splitlines()] if (d/'routes.jsonl').exists() else []
 result={'name':name,'responses':responses,'worker':worker,'prefix_comparison':compare(values,ref[:len(values)]),'capture_events':events,'routes':routes,'process_exit':p.returncode};print(json.dumps({'name':name,'stop_reason':worker.get('stop_reason'),'prefix':result['prefix_comparison']}),flush=True);return result

def main():
 root=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path(tempfile.mkdtemp(prefix='qoda-alsa-followups-'));os.chmod(root,0o700)
 f=fixture(root,'long-gain',frames=48000,flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'-6 dB','REPLAYGAIN_TRACK_PEAK':'.0625'})
 b=fixture(root,'b',frames=14400,seed=12000,flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'-12 dB','REPLAYGAIN_TRACK_PEAK':'.0625'})
 a=fixture(root,'a',frames=12000,tags=None)
 c=fixture(root,'c',frames=12000,seed=32000,flac=True)
 boosted=fixture(root,'known-boost',frames=12000,flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'+6 dB','REPLAYGAIN_TRACK_PEAK':'.0625'})
 cases=[]
 cases.append(run_case(root,'positive-gain',[boosted],'track'))
 cases.append(run_case(root,'loop-file-gain',[f],'track',options={'loop-file':'1'},expected_files=[f,f],expected_ends=1))
 seek=run_case(root,'seek-gain',[f],'track',actions=[{'at':.08,'command':['seek','.4','absolute+exact']}]);values=actual(root/'seek-gain');events=[e for stream in seek['streams'] for e in stream['events']];cut=next((e['accepted_total'] for e in events if e['event']=='drop' and 0<e.get('accepted_total',0)<len(values)//2),None)
 if cut is not None:
  factor=gain(f,'track')['factor'];ref=[round(x*factor) for x in f['samples']];seek['seek_epochs']={'cut_frames':cut,'before':compare(values[:cut*2],ref[:cut*2]),'after':compare(values[cut*2:],ref[19200*2:]),'discarded_preseek_buffer_not_claimed_as_heard':True}
 cases.append(seek)
 cases.append(run_case(root,'accepted-remove-prefetch',[a,b,c],'off',actions=[{'at':.04,'command':['playlist-remove','1']}],expected_files=[a,c],expected_ends=2))
 handle=C.c_void_p();rc=A.snd_pcm_open(C.byref(handle),DEVICE.encode(),0,1);assert rc>=0
 try:busy=run_case(root,'busy-before-playback',[a],'off')
 finally:A.snd_pcm_close(handle)
 cases.append(busy)
 cases.append(policy(root,'volume-and-late-intent',f,'volume'))
 cases.append(policy(root,'mode-switch-pauses',f,'mode-switch'))
 cases.append(policy(root,'missing-new-device',f,'device-switch'))
 cases.append(policy(root,'injected-device-loss',f,'device-loss'))
 report={'positive_control_expected':True,'cases':cases,'scratch':str(root),'limits':['MPRIS/app guard is a disposable Python owner-thread fixture, not production Rust integration','Device loss is injected ENODEV, not an unplugged internal speaker','Mode switch verifies pause/release and retained intent; Desktop route playback is not exercised','Seek accepted buffers can be discarded before hardware consumption; no claim they were heard']}
 (HERE/'evidence/followups.json').write_text(json.dumps(report,indent=2)+'\n');print('Follow-up evidence written: '+str(root),flush=True)
if __name__=='__main__':main()
