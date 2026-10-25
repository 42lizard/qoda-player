#!/usr/bin/env python3
"""Diagnostic decision requests only; no stream or timeline starts."""
import json,pathlib,uuid,xml.etree.ElementTree as ET
import requests
from credential import load_legacy
HERE=pathlib.Path(__file__).resolve().parent
out=[]
def main():
 c=load_legacy();h={'Accept':'application/xml','X-Plex-Token':c['token'],'X-Plex-Client-Identifier':c['client'],'X-Plex-Product':'Qoda Plex Compatibility Validator'}
 r=requests.get('https://plex.tv/api/resources',headers=h,params={'includeHttps':1},timeout=(5,15));r.raise_for_status();s=next(x for x in ET.fromstring(r.content).findall('Device') if x.get('owned')=='1' and 'server' in x.get('provides','').split(','));h['X-Plex-Token']=s.get('accessToken') or c['token'];url=next(x.get('uri') for x in s.findall('Connection') if x.get('protocol')=='https').rstrip('/')
 def get(path,params=None,headers=None):return requests.get(url+path,headers={**h,**(headers or {})},params=params,timeout=(5,15),allow_redirects=False)
 r=get('/library/sections');r.raise_for_status();library=next(x for x in ET.fromstring(r.content).findall('Directory') if x.get('type')=='artist')
 r=get('/library/sections/'+library.get('key')+'/all',{'type':10,'X-Plex-Container-Start':0,'X-Plex-Container-Size':5});r.raise_for_status();track=ET.fromstring(r.content).find('Track');assert track is not None
 params={'path':'/library/metadata/'+track.get('ratingKey'),'mediaIndex':0,'partIndex':0,'directPlay':1,'directStream':0,'protocol':'http','hasMDE':1,'normalizeLoudness':0,'audioBoost':100,'transcodeSessionId':str(uuid.uuid4())}
 for label,headers,extra in [('baseline',{},{}),('api-1.2.3',{'X-Plex-Pms-Api-Version':'1.2.3'},{}),('platform-Linux',{'X-Plex-Platform':'Linux'},{}),('profile-Generic',{'X-Plex-Client-Profile-Name':'Generic'},{}),('legacy-session-query',{}, {'session':str(uuid.uuid4())})]:
  r=get('/music/:/transcode/universal/decision',{**params,**extra},headers);v={'name':label,'http_status':r.status_code,'error_hints':{k:k in r.text.lower() for k in ['profile','session','protocol','parameter','path']}}
  try:
   root=ET.fromstring(r.content);v['root_tag']=root.tag;v['attribute_names']=sorted(root.attrib);v['decision_codes']={k:root.get(k) for k in ['generalDecisionCode','directPlayDecisionCode','transcodeDecisionCode'] if root.get(k) is not None}
   v['decisions']=[{k:x.get(k) for k in ['decision','audioDecision','videoDecision'] if x.get(k) is not None} for x in root.iter() if any(x.get(k) is not None for k in ['decision','audioDecision','videoDecision'])]
  except ET.ParseError:v['xml']=False
  out.append(v);print(json.dumps(v),flush=True)
  if r.status_code==200:break
if __name__=='__main__':
 try:main()
 except Exception as e:out.append({'error_class':type(e).__name__})
 finally:(HERE/'evidence/decision-diagnostic.json').write_text(json.dumps({'observations':out,'limits':['Diagnostic error categories only; no private response text exported','Each variation changes one header or query parameter','No transcoded stream or timeline requested']},indent=2)+'\n')
