#!/usr/bin/env python3
"""One disposable libmpv owner process; ALSA tap is loaded before libmpv.
Only synthetic local files. Rate/precision checks happen before audible output.
"""
import ctypes as C,ctypes.util,json,pathlib,subprocess,sys,time
from preflight import probe,DEVICE
class Event(C.Structure):_fields_=[('id',C.c_int),('error',C.c_int),('reply',C.c_uint64),('data',C.c_void_p)]
class Property(C.Structure):_fields_=[('name',C.c_char_p),('format',C.c_int),('data',C.c_void_p)]
class End(C.Structure):_fields_=[('reason',C.c_int),('error',C.c_int),('entry',C.c_int64),('insert',C.c_int64),('count',C.c_int)]
class Log(C.Structure):_fields_=[('prefix',C.c_char_p),('level',C.c_char_p),('text',C.c_char_p),('number',C.c_int)]
def main(config):
 start=time.monotonic();result={'intent':{'mode':'fidelity','device':DEVICE,'volume':100,'native_replaygain':'no','dither':'no'},'preflight':[],'events':[],'commands':[],'errors':[]};lib=None;handle=None
 for file in config['files']:
  probe_result=subprocess.run(['ffprobe','-v','quiet','-select_streams','a:0','-show_entries','stream=codec_name,channels,channel_layout,sample_rate,bits_per_sample,bits_per_raw_sample:format=format_name','-of','json',file['path']],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=5,check=True)
  media=json.loads(probe_result.stdout);stream=media['streams'][0];bits=int(stream.get('bits_per_raw_sample') or 0) or int(stream.get('bits_per_sample') or 0);rate=int(stream['sample_rate']);container=media.get('format',{}).get('format_name')
  result.setdefault('source_formats',[]).append({'codec':stream.get('codec_name'),'rate':rate,'bits':bits,'channels':stream.get('channels'),'container':container})
  if container not in ['wav','flac'] or stream.get('channels')!=2 or bits!=16 or file['bits']!=bits or file['rate']!=rate:
   result['preflight'].append({'supported':False,'reason':'source format/precision/layout is outside selected profile or mismatches preparation','source_bits':bits,'source_rate':rate,'source_channels':stream.get('channels')});result['blocked_before_playback']=True;return result
  if file['bits']!=16:

   result['preflight'].append({'supported':False,'reason':'selected native device has insufficient precision','source_bits':file['bits']});result['blocked_before_playback']=True;return result
  checked=probe(file['rate'],'S16_LE',False);result['preflight'].append(checked)
  if not checked['supported']:result['blocked_before_playback']=True;return result
 lib=C.CDLL(ctypes.util.find_library('mpv'))
 for n,ret,args in [('mpv_create',C.c_void_p,[]),('mpv_set_option_string',C.c_int,[C.c_void_p,C.c_char_p,C.c_char_p]),('mpv_initialize',C.c_int,[C.c_void_p]),('mpv_command_async',C.c_int,[C.c_void_p,C.c_uint64,C.POINTER(C.c_char_p)]),('mpv_observe_property',C.c_int,[C.c_void_p,C.c_uint64,C.c_char_p,C.c_int]),('mpv_wait_event',C.POINTER(Event),[C.c_void_p,C.c_double]),('mpv_terminate_destroy',None,[C.c_void_p]),('mpv_request_log_messages',C.c_int,[C.c_void_p,C.c_char_p])]:fn=getattr(lib,n);fn.restype=ret;fn.argtypes=args
 handle=lib.mpv_create();assert handle
 options={'config':'no','load-scripts':'no','load-auto-profiles':'no','ytdl':'no','osc':'no','terminal':'no','vo':'null','vid':'no','audio-display':'no','ao':'alsa','audio-device':'alsa/'+DEVICE,'alsa-resample':'no','audio-format':'s16','audio-channels':'stereo','volume':'100','volume-gain':'0','mute':'no','speed':'1','replaygain':'no','gapless-audio':'weak','prefetch-playlist':'yes','audio-swresample-o':'dither_method=none','pause':'yes','idle':'yes','audio-buffer':'0.1'}
 options.update(config.get('options',{}));result['options']=options
 seq=0;state={};control=None;stop_reason=None;intent_generation=1;desired_playing=True
 def command(args):
  nonlocal seq
  seq+=1;argv=(C.c_char_p*(len(args)+1))(*(str(x).encode() for x in args),None);rc=lib.mpv_command_async(handle,seq,argv);result['commands'].append({'id':seq,'args':[pathlib.Path(x).name if str(x).startswith('/') else x for x in args],'submit':rc,'t':round(time.monotonic()-start,6)});assert rc>=0;return seq
 def request(q):
  nonlocal stop_reason,intent_generation,desired_playing
  op=q.get('op')
  if op=='set_volume':
   accepted=q.get('value')==1
   result['events'].append({'event':'volume-policy','requested':q.get('value'),'accepted':accepted,'actual_native_volume':state.get('volume')})
   return {'accepted':accepted,'error':None if accepted else 'Fidelity volume locked at unity','volume':state.get('volume',100)/100}
  if op=='pause':intent_generation+=1;desired_playing=False;command(['set','pause','yes']);return {'accepted':True,'generation':intent_generation}
  if op=='complete_prepare':
   stale=q.get('generation')!=intent_generation or not desired_playing
   return {'accepted':not stale,'stale':stale,'generation':intent_generation}
  if op in ['set_mode','set_device']:
   intent_generation+=1;desired_playing=False;stop_reason=op;result['requested_mode']=q.get('mode');result['requested_device']=q.get('device');command(['set','pause','yes']);return {'accepted':True,'pending_release':True}
  return {'accepted':False,'error':'Unsupported qualification command'}
 def snapshot():return {'pause':bool(state.get('pause',1)),'volume':state.get('volume',100),'position':state.get('time-pos') or 0,'generation':intent_generation,'desired_playing':desired_playing}
 try:
  for k,v in options.items():assert lib.mpv_set_option_string(handle,k.encode(),str(v).encode())>=0,k
  assert lib.mpv_initialize(handle)>=0;lib.mpv_request_log_messages(handle,b'warn')
  if config.get('control_path'):
   from control import Control
   control=Control(config['control_path'],request,snapshot)
  for i,(k,fmt) in enumerate([('path',1),('playlist-pos',4),('pause',3),('volume',5),('time-pos',5),('audio-out-params/samplerate',4),('audio-out-params/format',1),('audio-out-params/channels',1),('idle-active',3)],1):assert lib.mpv_observe_property(handle,i,k.encode(),fmt)>=0
  for i,file in enumerate(config['files']):command(['loadfile',file['path'],'replace' if i==0 else 'append','-1','af='+file.get('filter','')])
  command(['set','pause','no']);ends=0;idle=False;state={};actions=list(config.get('actions',[]));first_started=None;last=time.monotonic()
  while time.monotonic()-start<8:
   if control:control.pump()
   if stop_reason:break
   ev=lib.mpv_wait_event(handle,.01).contents;t=round(time.monotonic()-start,6)
   if ev.id:
    item={'event':ev.id,'t':t}
    if ev.id==22:
     p=C.cast(ev.data,C.POINTER(Property)).contents;v=None
     if p.data:
      if p.format==1:
       x=C.cast(p.data,C.POINTER(C.c_char_p))[0];v=x.decode() if x else None
      elif p.format in (3,4,5):v=C.cast(p.data,C.POINTER({3:C.c_int,4:C.c_int64,5:C.c_double}[p.format]))[0]
     key=p.name.decode();state[key]=v;item.update(property=key,value=v)
     if key=='idle-active':idle=bool(v)
    elif ev.id==7:
     e=C.cast(ev.data,C.POINTER(End)).contents;ends+=1;item.update(reason=e.reason,error=e.error)
    elif ev.id==5:item.update(reply=ev.reply,error=ev.error)
    elif ev.id==2:
     log=C.cast(ev.data,C.POINTER(Log)).contents;item.update(prefix=log.prefix.decode(),text=log.text.decode().strip())
     if config.get('injected_device_loss') and 'No such device' in item['text']:
      stop_reason='injected-device-loss';desired_playing=False;command(['set','pause','yes'])
    elif ev.id==21 and first_started is None:first_started=t
    if ev.id==18:
     proc=pathlib.Path('/proc/asound/card1/pcm1p/sub0/hw_params');v=proc.read_text() if proc.exists() else None;result.setdefault('proc_hw_params_observations',[]).append(v)
     if v and v.strip()!='closed':result['proc_hw_params']=v
    result['events'].append(item);last=time.monotonic()
   for action in actions[:]:
    if first_started is not None and (state.get('time-pos') or 0)>=action['at']:
     command(action['command']);actions.remove(action)
   expected=config.get('expected_ends',len(config['files']))
   if idle and ends>=expected and time.monotonic()-last>.05:break
  result.update(state=state,end_events=ends,timeout=time.monotonic()-start>=8,unexecuted_actions=actions,stop_reason=stop_reason)
  if stop_reason:result['session_restore']={'paused':True,'position':state.get('time-pos') or 0,'queue_entries':len(config['files']),'reason':stop_reason}
 finally:
  lib.mpv_terminate_destroy(handle)
  if control:control.close()
  if result.get('requested_device'):
   result['new_device_preflight']=probe(config['files'][0]['rate'],'S16_LE',False,result['requested_device'])
 result['elapsed_seconds']=round(time.monotonic()-start,6);return result
if __name__=='__main__':
 config=json.loads(pathlib.Path(sys.argv[1]).read_text());output=pathlib.Path(sys.argv[2])
 try:r=main(config)
 except Exception as e:r={'harness_error':type(e).__name__,'message':str(e)}
 output.write_text(json.dumps(r,indent=2)+'\n')
