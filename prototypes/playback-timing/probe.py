#!/usr/bin/env python3
"""Disposable libmpv clock observer. Synthetic quiet local PCM, no desktop edits."""
import ctypes as C
import ctypes.util
import json
import math
from pathlib import Path
import sys
import time

class Event(C.Structure):_fields_=[('id',C.c_int),('error',C.c_int),('reply',C.c_uint64),('data',C.c_void_p)]
class End(C.Structure):_fields_=[('reason',C.c_int),('error',C.c_int),('entry',C.c_int64),('insert',C.c_int64),('count',C.c_int)]
class Start(C.Structure):_fields_=[('entry',C.c_int64)]
class Log(C.Structure):_fields_=[('prefix',C.c_char_p),('level',C.c_char_p),('text',C.c_char_p),('number',C.c_int)]

def main(config):
 C.CDLL(ctypes.util.find_library('asound'),mode=C.RTLD_GLOBAL)
 lib=C.CDLL(ctypes.util.find_library('mpv'))
 declarations=[('mpv_create',C.c_void_p,[]),('mpv_set_option_string',C.c_int,[C.c_void_p,C.c_char_p,C.c_char_p]),('mpv_initialize',C.c_int,[C.c_void_p]),('mpv_command',C.c_int,[C.c_void_p,C.POINTER(C.c_char_p)]),('mpv_get_property',C.c_int,[C.c_void_p,C.c_char_p,C.c_int,C.c_void_p]),('mpv_wait_event',C.POINTER(Event),[C.c_void_p,C.c_double]),('mpv_request_log_messages',C.c_int,[C.c_void_p,C.c_char_p]),('mpv_terminate_destroy',None,[C.c_void_p])]
 for name,ret,args in declarations:fn=getattr(lib,name);fn.restype=ret;fn.argtypes=args
 h=lib.mpv_create();assert h
 result={'events':[],'clocks':[],'commands':[],'mode':config['mode']}
 def get(name,fmt=5):
  value={3:C.c_int,4:C.c_int64,5:C.c_double}[fmt]()
  rc=lib.mpv_get_property(h,name.encode(),fmt,C.byref(value))
  return value.value if rc>=0 and (fmt!=5 or math.isfinite(value.value)) else None
 def cmd(*args):
  argv=(C.c_char_p*(len(args)+1))(*(str(a).encode() for a in args),None)
  rc=lib.mpv_command(h,argv);result['commands'].append({'mono_ns':time.monotonic_ns(),'args':[Path(a).name if str(a).startswith('/') else a for a in args],'result':rc});assert rc>=0
 options={'config':'no','load-scripts':'no','terminal':'no','ytdl':'no','vo':'null','vid':'no','audio-display':'no','volume':'100','volume-gain':'0','speed':'1','replaygain':'no','audio-swresample-o':'dither_method=none','pause':'yes','idle':'yes','gapless-audio':config.get('gapless','weak'),'prefetch-playlist':'yes','audio-buffer':'0.1','audio-channels':'stereo','audio-format':'s16','ao':config['mode']}
 if config['mode']=='alsa':options.update({'audio-device':'alsa/hw:CARD=avsmax98927,DEV=1','alsa-resample':'no'})
 start=time.monotonic();actions=list(config.get('actions',[]));started=None;ended=0;last_confirmed=None
 try:
  for k,v in options.items():assert lib.mpv_set_option_string(h,k.encode(),str(v).encode())>=0,k
  assert lib.mpv_initialize(h)>=0;lib.mpv_request_log_messages(h,b'warn')
  for i,path in enumerate(config['files']):cmd('loadfile',path,'replace' if i==0 else 'append')
  cmd('set','pause','no')
  while time.monotonic()-start<config.get('timeout',6):
   ev=lib.mpv_wait_event(h,.003).contents
   if ev.id:
    item={'event':ev.id,'mono_ns':time.monotonic_ns()}
    if ev.id==6:item['entry']=C.cast(ev.data,C.POINTER(Start)).contents.entry
    if ev.id==7:
     e=C.cast(ev.data,C.POINTER(End)).contents;item.update(entry=e.entry,reason=e.reason,error=e.error);ended+=1
    if ev.id==2:
     log=C.cast(ev.data,C.POINTER(Log)).contents;item.update(prefix=log.prefix.decode(),text=log.text.decode().strip())
    result['events'].append(item)
    if ev.id==21 and started is None:started=time.monotonic()
   pos=get('playlist-playing-pos',4)
   entry=get(f'playlist/{pos}/id',4) if pos is not None and pos>=0 else None
   row={'mono_ns':time.monotonic_ns(),'entry':entry,'index':pos,'audio_pts':get('audio-pts'),'time_pos':get('time-pos'),'duration':get('duration'),'pause':get('pause',3),'seeking':get('seeking',3),'cache_pause':get('paused-for-cache',3),'idle':get('idle-active',3),'core_idle':get('core-idle',3),'eof':get('eof-reached',3)}
   after=get('playlist-playing-pos',4);after_entry=get(f'playlist/{after}/id',4) if after is not None and after>=0 else None
   row['coherent']=entry==after_entry;result['clocks'].append(row)
   if row['coherent'] and row['audio_pts'] is not None and row['audio_pts']>0 and not row['pause'] and not row['seeking']:
    last_confirmed={'entry':entry,'index':pos,'position':row['audio_pts'],'mono_ns':row['mono_ns']}
   for action in actions[:]:
    condition=(started is not None and time.monotonic()-started>=action.get('after',0))
    if 'engine_index' in action:condition=condition and pos==action['engine_index']
    if 'pts_min' in action:condition=condition and row['audio_pts'] is not None and row['audio_pts']>=action['pts_min']
    if 'pts_negative' in action:condition=condition and row['audio_pts'] is not None and row['audio_pts']<0
    if condition:
     if action.get('rebuild'):
      result['barrier']={'mono_ns':time.monotonic_ns(),'clock':row,'last_confirmed':last_confirmed}
      cmd('set','pause','yes');cmd('stop')
      if action['rebuild']=='upcoming':
       assert last_confirmed and last_confirmed['index']==0
       checkpoint=max(0,last_confirmed['position'])
       if pos==1 and row['audio_pts'] is not None and row['audio_pts']<0:
        checkpoint=max(0,config.get('exact_durations',[0])[0]+row['audio_pts'])
       result['barrier']['recovery_position']=checkpoint
       cmd('loadfile',config['files'][0],'replace','-1','start='+str(checkpoint)+',af=')
       cmd('loadfile',config['files'][2],'append','-1','af=')
      else:cmd('loadfile',config['files'][2],'replace','-1','af=')
      cmd('set','pause','no');result['rebuild_completed']=True
     else:cmd(*action['command'])
     actions.remove(action)
   if row['idle'] and ended>=config.get('expected_ends',len(config['files'])):break
  result.update(unexecuted_actions=actions,elapsed=time.monotonic()-start,timeout=time.monotonic()-start>=config.get('timeout',6))
 finally:lib.mpv_terminate_destroy(h)
 return result

if __name__=='__main__':
 config=json.loads(Path(sys.argv[1]).read_text())
 try:result=main(config)
 except Exception as e:result={'error':type(e).__name__,'detail':str(e)}
 Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
