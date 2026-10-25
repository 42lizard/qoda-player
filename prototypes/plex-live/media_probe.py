#!/usr/bin/env python3
"""Bounded original-media access experiment. Bytes remain in memory, null reporting.
No timeline, progress/scrobble call, audible output, or transcoded stream request.
"""
import json,pathlib,re,subprocess,time,uuid,urllib.parse,xml.etree.ElementTree as ET
import requests
from credential import load_legacy
HERE=pathlib.Path(__file__).resolve().parent
observations=[]
stage='starting'
def observe(name,**kw):v={'name':name,**kw};observations.append(v);print(json.dumps(v),flush=True)
def main():
 global stage
 stage='credential-load'
 c=load_legacy();h={'Accept':'application/xml','X-Plex-Token':c['token'],'X-Plex-Client-Identifier':c['client'],'X-Plex-Product':'Qoda Plex Compatibility Validator','X-Plex-Client-Profile-Name':'Generic'}
 stage='resource-discovery'
 r=requests.get('https://plex.tv/api/resources',headers=h,params={'includeHttps':1},timeout=(5,15),allow_redirects=False);assert r.status_code==200
 servers=[s for s in ET.fromstring(r.content).findall('Device') if s.get('owned')=='1' and 'server' in s.get('provides','').split(',')];assert len(servers)==1
 s=servers[0];h['X-Plex-Token']=s.get('accessToken') or c['token'];url=next(x.get('uri') for x in s.findall('Connection') if x.get('protocol')=='https').rstrip('/')
 def get(path,params=None,headers=None,stream=False):
  start=time.monotonic()
  for attempt in range(3):
   left=30-(time.monotonic()-start)
   if left<=1:raise TimeoutError('read-budget')
   try:return requests.get(url+path,headers={**h,**(headers or {})},params=params,timeout=(min(5,left/2),min(15,left/2)),allow_redirects=False,stream=stream)
   except (requests.exceptions.Timeout,requests.exceptions.ConnectionError):
    if attempt==2:raise
    delay=attempt+1
    if time.monotonic()-start+delay>=30:raise
    time.sleep(delay)
 stage='library-discovery'
 r=get('/library/sections');assert r.status_code==200;lib=next(x for x in ET.fromstring(r.content).findall('Directory') if x.get('type')=='artist')
 stage='track-page'
 r=get('/library/sections/'+urllib.parse.quote(lib.get('key'),safe='')+'/all',{'type':10,'X-Plex-Container-Start':0,'X-Plex-Container-Size':5});assert r.status_code==200;track=ET.fromstring(r.content).find('Track');assert track is not None
 stage='track-detail'
 ref=track.get('ratingKey');r=get('/library/metadata/'+urllib.parse.quote(ref,safe=''));assert r.status_code==200;detail=ET.fromstring(r.content);media=detail.find('.//Media');part=detail.find('.//Part');assert media is not None and part is not None
 path=part.get('key');assert path and path.startswith('/library/parts/') and not path.startswith('//')
 observe('Selected original representation',container=media.get('container'),audio_codec=media.get('audioCodec'),channels=media.get('audioChannels'),part_size_known=part.get('size') is not None,metadata_not_exported=True)
 stage='original-decision'
 session=str(uuid.uuid4());r=get('/music/:/transcode/universal/decision',{'path':'/library/metadata/'+ref,'mediaIndex':0,'partIndex':0,'directPlay':1,'directStream':0,'protocol':'http','hasMDE':1,'normalizeLoudness':0,'audioBoost':100,'transcodeSessionId':session})
 codes={};decisions=[]
 if r.status_code==200:
  tree=ET.fromstring(r.content);codes={k:tree.get(k) for k in ['generalDecisionCode','directPlayDecisionCode','transcodeDecisionCode','mdeDecisionCode'] if tree.get(k) is not None}
  decisions=[{k:x.get(k) for k in ['decision','audioDecision','videoDecision'] if x.get(k) is not None} for x in tree.iter() if any(x.get(k) is not None for k in ['decision','audioDecision','videoDecision'])]
 observe('Explicit original playback decision',http_status=r.status_code,decision_codes=codes,decisions=decisions,transcoded_stream_requested=False)
 saved=[]
 for offset,size in [(0,65536),(131072,128),(131072,128)]:
  stage='original-range-'+str(offset)
  r=get(path,headers={'Range':f'bytes={offset}-{offset+size-1}'},stream=True);data=next(r.iter_content(size),b'');r.close();cr=r.headers.get('Content-Range','');match=re.fullmatch(r'bytes (\d+)-(\d+)/(\d+|\*)',cr)
  observe('Original-media bounded Range request',requested_offset=offset,http_status=r.status_code,bytes_retained=len(data),range_start_matches=bool(match) and int(match[1])==offset,server_honors_range=r.status_code==206 and bool(match),original_content_type=r.headers.get('Content-Type'))
  saved.append(data)
 observe('Fresh-connection range reconnect',same_range_bytes_match=saved[1]==saved[2],bytes_compared=len(saved[1]),network_drop_simulated=False)
 tags='REPLAYGAIN_TRACK_GAIN,REPLAYGAIN_TRACK_PEAK,REPLAYGAIN_ALBUM_GAIN,REPLAYGAIN_ALBUM_PEAK,replaygain_track_gain,replaygain_track_peak,replaygain_album_gain,replaygain_album_peak'
 p=subprocess.run(['ffprobe','-v','quiet','-show_entries','format_tags='+tags+':stream_tags='+tags,'-of','json','-i','pipe:0'],input=saved[0],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10)
 try:decoded=json.loads(p.stdout)
 except ValueError:decoded={}
 filtered={}
 def extract(node):
  if isinstance(node,dict):
   for k,v in node.items():
    if k.lower().startswith('replaygain_') and isinstance(v,str):filtered[k.lower()]=v
    else:extract(v)
  elif isinstance(node,list):
   for x in node:extract(x)
 extract(decoded)
 observe('Embedded ReplayGain tags in bounded prefix',tag_names=sorted(filtered),gain_unit_suffix_db={k:v.strip().lower().endswith('db') for k,v in filtered.items() if k.endswith('_gain')},prefix_bound_bytes=65536,absence_not_proven=True,values_exported=False,plex_field_provenance_proven=False)
if __name__=='__main__':
 try:main()
 except Exception as e:observe('Media access probe stopped',result='unverified',error_class=type(e).__name__,stage=stage)
 finally:(HERE/'evidence/media.json').write_text(json.dumps({'scope':'one track; bounded in-memory bytes only; no timeline/scrobble, audible playback or raw media artifact','observations':observations,'limits':['No real-time gapless or DAC claim','Fresh-connection repeated Range is not a real network-drop recovery test','No server progress/history report sent','No universal media version/format claim','Gain field presence is not verified provenance or unit mapping']},indent=2)+'\n')
