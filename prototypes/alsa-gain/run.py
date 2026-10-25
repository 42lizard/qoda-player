#!/usr/bin/env python3
"""Disposable native ALSA payload qualification, synthetic fixtures only.
Short low-level sound on explicitly selected built-in speakers. No mixer changes.
"""
import array,json,math,os,pathlib,subprocess,tempfile,wave
from preflight import DEVICE,probe
HERE=pathlib.Path(__file__).resolve().parent

def samples(frames,seed=0,bits=16):
 values=[]
 for i in range(seed,seed+frames):
  for ch in range(2):
   x=((i*1664525+ch*1013904223+0x12345678)&0xffffffff);x^=x>>13
   values.append((x%4095)-2047)
 return values

def fixture(directory,name,frames=12000,seed=0,rate=48000,bits=16,tags=None,flac=False):
 values=samples(frames,seed,bits);path=directory/(name+'.wav')
 with wave.open(str(path),'wb') as w:w.setparams((2,bits//8,rate,0,'NONE','not compressed'));w.writeframes(b''.join(int(x).to_bytes(bits//8,'little',signed=True) for x in values))
 if flac:
  output=directory/(name+'.flac');cmd=['ffmpeg','-nostdin','-v','error','-y','-i',str(path),'-c:a','flac']
  for k,v in (tags or {}).items():cmd+=['-metadata',k+'='+str(v)]
  subprocess.run(cmd+[str(output)],check=True);path=output
 return {'path':str(path),'bits':bits,'rate':rate,'samples':values,'tags':tags or {}}

def read_tags(file):
 p=subprocess.run(['ffprobe','-v','quiet','-show_entries','format_tags','-of','json',file['path']],stdout=subprocess.PIPE,check=True)
 return {k.upper():v for k,v in json.loads(p.stdout).get('format',{}).get('tags',{}).items()}

def gain(file,mode='off',album_context=False):
 tags=read_tags(file);effective='album' if mode=='auto' and album_context else 'track' if mode=='auto' else mode
 chosen=None;db=0.;peak=None
 if effective!='off':
  candidates=['ALBUM','TRACK'] if effective=='album' else ['TRACK']
  for kind in candidates:
   try:
    text=tags['REPLAYGAIN_'+kind+'_GAIN'].strip();assert text.lower().endswith('db');db=float(text[:-2]);assert math.isfinite(db);chosen=kind.lower()
    try:peak=float(tags['REPLAYGAIN_'+kind+'_PEAK']);assert math.isfinite(peak) and peak>0
    except (KeyError,ValueError,AssertionError):peak=None
    break
   except (KeyError,ValueError,AssertionError):continue
 factor=10**(db/20) if chosen else 1.
 if chosen:factor=min(factor,1/peak if peak is not None else 1.)
 return {'factor':factor,'effective':effective,'provenance':'embedded-static-tags' if chosen else 'off' if effective=='off' else 'missing-data-unity','selected':chosen,'peak_known':peak is not None,'filter':'' if factor==1 else f'lavfi=[volume={factor:.17g}:precision=double]'}

def run_case(root,name,files,mode='off',album_context=False,actions=None,expected_ends=None,options=None,expected_files=None):
 directory=root/name;directory.mkdir(exist_ok=True);prepared=[];expected=[];policies=[]
 for f in files:
  policy=gain(f,mode,album_context);policies.append(policy);prepared.append({k:v for k,v in f.items() if k in ['path','bits','rate']});prepared[-1]['filter']=policy['filter'];expected.extend(max(-32768,min(32767,round(x*policy['factor']))) for x in f['samples'])
 if expected_files is not None:
  expected=[]
  for f in expected_files:
   pol=gain(f,mode,album_context);expected.extend(max(-32768,min(32767,round(x*pol['factor']))) for x in f['samples'])
 config={'files':prepared,'actions':actions or [],'options':options or {}}
 if expected_ends is not None:config['expected_ends']=expected_ends
 (directory/'config.json').write_text(json.dumps(config));env=dict(os.environ,LD_PRELOAD=str(HERE/'tap/target/release/libqoda_alsa_tap_prototype.so'),QODA_TAP_DIR=str(directory),QODA_TAP_DEVICE=DEVICE)
 if (directory/'worker.json').exists():
  from types import SimpleNamespace
  p=SimpleNamespace(returncode=None,stderr='');restored=True
 else:
  p=subprocess.run(['python3',str(HERE/'worker.py'),str(directory/'config.json'),str(directory/'worker.json')],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=12)
  restored=False
 worker=json.loads((directory/'worker.json').read_text()) if (directory/'worker.json').exists() else {};streams=[];actual=[]
 for file in sorted(directory.glob('stream-*.pcm')):
  raw=array.array('h');raw.frombytes(file.read_bytes());actual.extend(raw);events=[json.loads(l) for l in file.with_suffix('.jsonl').read_text().splitlines()];streams.append({'samples':len(raw),'events':events})
 diff=[abs(a-b) for a,b in zip(actual,expected)];routes=[json.loads(l) for l in (directory/'routes.jsonl').read_text().splitlines()] if (directory/'routes.jsonl').exists() else []
 native_reference=[]
 for f,pol in zip(files,policies):native_reference.extend(round(x*(round(pol['factor']*256)/256)) for x in f['samples'])
 native_error=max((abs(a-b) for a,b in zip(actual,native_reference)),default=0)
 result={'name':name,'restored_existing_capture':restored,'policies':policies,'expected_samples':len(expected),'actual_samples':len(actual),'same_count':len(expected)==len(actual),'max_error_lsb':max(diff,default=0) if actual else None,'exact':actual==expected,'native_quantized_error_lsb':native_error,'worker_exit':p.returncode,'stderr':p.stderr,'worker':worker,'streams':streams,'routes':routes}
 if worker.get('blocked_before_playback'):result['blocked_without_pcm']=not actual
 print(json.dumps({k:result.get(k) for k in ['name','actual_samples','expected_samples','same_count','max_error_lsb','exact','blocked_without_pcm']}),flush=True)
 return result

def main():
 subprocess.run(['cargo','build','--release','--locked','--manifest-path',str(HERE/'tap/Cargo.toml')],check=True)
 root=pathlib.Path(tempfile.mkdtemp(prefix='qoda-alsa-qualification-'));os.chmod(root,0o700)
 files={
 'wav-a':fixture(root,'wav-a'),
 'flac-b':fixture(root,'flac-b',frames=14400,seed=12000,flac=True),
 'wav-unaligned':fixture(root,'wav-unaligned',frames=12001),
 'flac-unaligned':fixture(root,'flac-unaligned',frames=14403,seed=12001,flac=True),
 'rg-a':fixture(root,'rg-a',flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'-6 dB','REPLAYGAIN_TRACK_PEAK':'0.0625','REPLAYGAIN_ALBUM_GAIN':'-12 dB','REPLAYGAIN_ALBUM_PEAK':'0.0625'}),
 'rg-b':fixture(root,'rg-b',frames=14400,seed=12000,flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'-12 dB','REPLAYGAIN_TRACK_PEAK':'0.0625','REPLAYGAIN_ALBUM_GAIN':'-12 dB','REPLAYGAIN_ALBUM_PEAK':'0.0625'}),
 'track-only':fixture(root,'track-only',flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'-6 dB'}),
 'boost-unknown':fixture(root,'boost-unknown',flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'+6 dB'}),
 'boost-capped':fixture(root,'boost-capped',flac=True,tags={'REPLAYGAIN_TRACK_GAIN':'+12 dB','REPLAYGAIN_TRACK_PEAK':'1.0'}),
 'rate44':fixture(root,'rate44',rate=44100),
 'rate96':fixture(root,'rate96',rate=96000),
 'bits24':fixture(root,'bits24',bits=24),
 }
 cases=[]
 for name,names,mode,context in [('off-wav',['wav-a'],'off',False),('off-flac',['flac-b'],'off',False),('ready-lossless-boundary',['wav-a','flac-b'],'off',False),('unaligned-boundary',['wav-unaligned','flac-unaligned'],'off',False),('track-boundary',['rg-a','rg-b'],'track',False),('gain-to-unity-and-back',['rg-a','flac-b','rg-b'],'track',False),('album-boundary',['rg-a','rg-b'],'album',False),('auto-album',['rg-a','rg-b'],'auto',True),('auto-mixed',['rg-a','rg-b'],'auto',False),('album-track-fallback',['track-only'],'album',False),('missing-unity',['flac-b'],'track',False),('boost-without-peak',['boost-unknown'],'track',False),('conservative-peak-cap',['boost-capped'],'track',False),('unsupported44',['rate44'],'off',False),('unsupported96',['rate96'],'off',False),('unsupported24',['bits24'],'off',False)]:
  cases.append(run_case(root,name,[files[n] for n in names],mode,context))
 report={'device':DEVICE,'boundary':'Only PCM frames accepted by real snd_pcm_writei/writen after successful hw negotiation; no kernel/DSP/DAC capture','cases':cases,'limits':['Only this built-in speaker device and 48k S16 stereo can qualify','No physical transport, DSP, DAC or analog fidelity claim','Low-level synthetic samples only; no Plex credentials or content','MMAP is not used by this pinned mpv ALSA implementation and is not qualified here'],'scratch':str(root)}
 (HERE/'evidence/results.json').write_text(json.dumps(report,indent=2)+'\n');print('Evidence written; scratch '+str(root),flush=True)
if __name__=='__main__':main()
