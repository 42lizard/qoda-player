#!/usr/bin/env python3
"""Exact ALSA hw preflight only; opens/closes own handle, submits no samples."""
import ctypes as C,ctypes.util,json,pathlib
DEVICE='hw:CARD=avsmax98927,DEV=1'
A=C.CDLL(ctypes.util.find_library('asound'),mode=C.RTLD_GLOBAL)
for n,res,args in [('snd_pcm_open',C.c_int,[C.POINTER(C.c_void_p),C.c_char_p,C.c_int,C.c_int]),('snd_pcm_close',C.c_int,[C.c_void_p]),('snd_pcm_hw_params_malloc',C.c_int,[C.POINTER(C.c_void_p)]),('snd_pcm_hw_params_free',None,[C.c_void_p]),('snd_pcm_hw_params_any',C.c_int,[C.c_void_p,C.c_void_p]),('snd_pcm_hw_params_set_access',C.c_int,[C.c_void_p,C.c_void_p,C.c_int]),('snd_pcm_hw_params_set_format',C.c_int,[C.c_void_p,C.c_void_p,C.c_int]),('snd_pcm_hw_params_set_channels',C.c_int,[C.c_void_p,C.c_void_p,C.c_uint]),('snd_pcm_hw_params_set_rate',C.c_int,[C.c_void_p,C.c_void_p,C.c_uint,C.c_int]),('snd_pcm_hw_params',C.c_int,[C.c_void_p,C.c_void_p]),('snd_pcm_format_value',C.c_int,[C.c_char_p]),('snd_strerror',C.c_char_p,[C.c_int])]:f=getattr(A,n);f.restype=res;f.argtypes=args

def probe(rate,fmt,install=False,device=DEVICE):
 h=C.c_void_p();p=C.c_void_p();code=A.snd_pcm_open(C.byref(h),device.encode(),0,1)
 if code<0:return {'supported':False,'stage':'open','error':code,'reason':A.snd_strerror(code).decode()}
 try:
  assert A.snd_pcm_hw_params_malloc(C.byref(p))>=0
  for stage,fn,args in [('any',A.snd_pcm_hw_params_any,()),('access',A.snd_pcm_hw_params_set_access,(3,)),('format',A.snd_pcm_hw_params_set_format,(A.snd_pcm_format_value(fmt.encode()),)),('channels',A.snd_pcm_hw_params_set_channels,(2,)),('rate',A.snd_pcm_hw_params_set_rate,(rate,0))]:
   code=fn(h,p,*args)
   if code<0:return {'supported':False,'stage':stage,'error':code,'reason':A.snd_strerror(code).decode()}
  if install:
   code=A.snd_pcm_hw_params(h,p)
   if code<0:return {'supported':False,'stage':'install','error':code,'reason':A.snd_strerror(code).decode()}
  return {'supported':True,'installed_without_samples':install}
 finally:
  if p:A.snd_pcm_hw_params_free(p)
  A.snd_pcm_close(h)
if __name__=='__main__':
 rows=[{'rate':r,'format':f,**probe(r,f,True)} for r in [44100,48000,96000] for f in ['S16_LE','S24_LE','S24_3LE','S32_LE']]
 report={'selected_device':DEVICE,'boundary':'Exact hw parameter installation; no PCM submitted','matrix':rows}
 pathlib.Path(__file__).with_name('evidence').joinpath('preflight.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
