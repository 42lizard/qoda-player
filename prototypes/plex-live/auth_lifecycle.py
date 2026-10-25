#!/usr/bin/env python3
"""Negative JWT checks and retirement of this harness's own JWT test device only.
The separate working legacy test login and all pre-existing devices are preserved.
"""
import json,pathlib,subprocess,time,xml.etree.ElementTree as ET
import requests
from credential import load,load_legacy
from validate import Probe
HERE=pathlib.Path(__file__).resolve().parent
out=[]
def observe(name,**kw):v={'name':name,**kw};out.append(v);print(json.dumps(v),flush=True)
def main():
 c,_=load();legacy=load_legacy();assert c['client']!=legacy['client'];p=Probe();p.client=c['client'];p.kid=c['kid'];p.key=c['key'].encode();p.token=c['token'];p.headers['X-Plex-Client-Identifier']=p.client
 def nonce():
  r=p.request('GET','https://clients.plex.tv/api/v2/auth/nonce',headers=p.headers);assert r.status_code==200;return r.json()['nonce']
 now=int(time.time());expired=p.sign({'aud':'plex.tv','iss':p.client,'nonce':nonce(),'scope':'username,email,friendly_name','iat':now-600,'exp':now-300})
 r=p.request('POST','https://clients.plex.tv/api/v2/auth/token',headers=p.headers,json={'jwt':expired});observe('Expired signed device-JWT exchange',http_status=r.status_code,rejected=r.status_code not in (200,201),actual_seven_day_account_token_expiry_tested=False)
 valid=p.device_jwt(nonce=nonce(),scope='username,email,friendly_name');r=p.request('POST','https://clients.plex.tv/api/v2/auth/token',headers=p.headers,json={'jwt':valid});observe('Fresh signed device-JWT exchange',http_status=r.status_code)
 assert r.status_code==200
 r=p.request('POST','https://clients.plex.tv/api/v2/auth/token',headers=p.headers,json={'jwt':valid});observe('Immediate identical nonce/JWT replay',http_status=r.status_code,rejected=r.status_code not in (200,201))
 h={'Accept':'application/xml','X-Plex-Token':legacy['token'],'X-Plex-Client-Identifier':legacy['client'],'X-Plex-Product':'Qoda Plex Compatibility Validator'}
 r=p.request('GET','https://plex.tv/devices.xml',headers=h);assert r.status_code==200;devices=ET.fromstring(r.content).findall('Device');matches=[d for d in devices if d.get('clientIdentifier')==c['client']]
 observe('Exact own JWT test-device selection',matched_count=len(matches))
 if len(matches)!=1:return
 target=matches[0];assert target.get('product')=='Qoda Plex Validator' and 'server' not in target.get('provides','').split(',');ident=target.get('id');assert ident and ident.isdigit()
 r=p.request('DELETE','https://plex.tv/devices/'+ident+'.xml',headers=h);observe('Remove only own JWT test device',http_status=r.status_code)
 if r.status_code not in (200,204):return
 r=p.request('GET','https://plex.tv/devices.xml',headers=h);assert r.status_code==200;devices=ET.fromstring(r.content).findall('Device');observe('Device retirement verified',own_jwt_device_absent=not any(d.get('clientIdentifier')==c['client'] for d in devices),working_legacy_device_present=any(d.get('clientIdentifier')==legacy['client'] for d in devices))
 r=p.request('GET','https://plex.tv/api/resources',headers={**h,'X-Plex-Token':c['token']},params={'includeHttps':1});observe('Previously issued JWT after device removal',http_status=r.status_code,immediate_rejection=r.status_code in (401,403,498))
 r=p.request('POST','https://clients.plex.tv/api/v2/auth/token',headers=p.headers,json={'jwt':p.device_jwt(nonce=nonce(),scope='username,email,friendly_name')});observe('JWT renewal after own-device retirement',http_status=r.status_code,renewal_rejected=r.status_code not in (200,201))
 # Clear only this retired device's own local credential item, never legacy.
 attrs=['application','qoda-plex-validation','run',c['client']];subprocess.run(['/usr/bin/secret-tool','clear',*attrs],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
 q=subprocess.run(['/usr/bin/secret-tool','lookup',*attrs],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=5);observe('Retired own JWT credential local cleanup',no_secret_returned=not q.stdout)
 r=p.request('GET','https://plex.tv/api/resources',headers=h,params={'includeHttps':1});observe('Working legacy authorization retained',http_status=r.status_code)
if __name__=='__main__':
 try:main()
 except Exception as e:observe('Authentication lifecycle stopped',result='unverified',error_class=type(e).__name__)
 finally:(HERE/'evidence/auth-lifecycle.json').write_text(json.dumps({'observations':out,'scope':'Only the explicitly matched fresh JWT validation device; no server or pre-existing device targeted','limits':['A signed expired device JWT is not an observed seven-day account-token expiry','Immediate results are observations, not a universal provider revocation guarantee','Working compatibility credential is intentionally retained for further authorized validation']},indent=2)+'\n')
