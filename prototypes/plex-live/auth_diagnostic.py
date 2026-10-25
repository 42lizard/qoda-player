#!/usr/bin/env python3
"""Differential auth probe. Outputs only shape, timing and HTTP codes."""
import json,pathlib,time,base64,email.utils,xml.etree.ElementTree as ET
import requests
from credential import load,store
from validate import Probe
HERE=pathlib.Path(__file__).resolve().parent
observations=[]
def observe(name,**kw):v={'name':name,**kw};observations.append(v);print(json.dumps(v),flush=True)
def payload(token):return json.loads(base64.urlsafe_b64decode(token.split('.')[1]+'==='))
def main():
 credential,_=load();p=Probe();p.client=credential['client'];p.kid=credential['kid'];p.key=credential['key'].encode();p.token=credential['token'];p.headers['X-Plex-Client-Identifier']=p.client
 r=p.request('GET','https://plex.tv/api/resources',headers={**p.headers,'Accept':'application/xml','X-Plex-Token':p.token},params={'includeHttps':1});assert r.status_code==200
 servers=[x for x in ET.fromstring(r.content).findall('Device') if x.get('owned')=='1' and 'server' in x.get('provides','').split(',')];assert len(servers)==1
 server=servers[0];conn=next(c for c in server.findall('Connection') if c.get('protocol')=='https');url=conn.get('uri').rstrip('/')
 identity=p.request('GET',url+'/identity',headers={'Accept':'application/xml'});tree=ET.fromstring(identity.content)
 observe('Selected endpoint identity',matches_authorized_server=tree.get('machineIdentifier')==server.get('clientIdentifier'),pms_version=tree.get('version'),http_status=identity.status_code)
 for label,token in [('stored-account',p.token),('resource',server.get('accessToken'))]:
  claims=payload(token);exp=claims.get('exp');iat=claims.get('iat');date=email.utils.parsedate_to_datetime(identity.headers['Date']).timestamp() if identity.headers.get('Date') else None
  observe('JWT shape/timing',source=label,claim_names=sorted(claims),expiry_in_seconds=round(exp-time.time()) if isinstance(exp,(int,float)) else None,issued_offset_seconds=round(iat-time.time()) if isinstance(iat,(int,float)) else None,pms_http_date_skew_seconds=round(date-time.time()) if date else None,same_token_as_account=token==p.token)
 n=p.request('GET','https://clients.plex.tv/api/v2/auth/nonce',headers=p.headers);assert n.status_code==200
 r=p.request('POST','https://clients.plex.tv/api/v2/auth/token',headers=p.headers,json={'jwt':p.device_jwt(nonce=n.json()['nonce'],scope='username,email,friendly_name')});data=r.json() if r.status_code==200 else {};token=data.get('auth_token') or data.get('authToken')
 observe('Renewal with documented user-claim scope',http_status=r.status_code,jwt_received=bool(token))
 if not token:return
 credential['token']=token;assert store(credential)
 observe('Restored private key signed successful renewal',credential_round_trip=True)
 for source,t in [('stored-minimal-scope',p.token),('documented-scope',token)]:
  for version in [None,'1.0.0','1.2.3']:
   h={**p.headers,'Accept':'application/xml','X-Plex-Token':t}
   if version:h['X-Plex-Pms-Api-Version']=version
   r=p.request('GET',url+'/library/sections',headers=h)
   observe('PMS auth/version comparison',source=source,requested_api_version=version or 'omitted',http_status=r.status_code)
 r=p.request('GET','https://plex.tv/api/resources',headers={**p.headers,'Accept':'application/xml','X-Plex-Token':token},params={'includeHttps':1})
 observe('New documented-scope token cloud access',http_status=r.status_code)
if __name__=='__main__':
 try:main()
 except Exception as e:observe('Auth diagnostic stopped',result='unverified',error_class=type(e).__name__)
 finally:(HERE/'evidence/auth-diagnostic.json').write_text(json.dumps({'observations':observations,'hypotheses':['user-claim scope','API header negotiation','server-side JWT support despite valid token and matching endpoint'],'scope':'read-only authorization differential; no legacy sign-in, server mutation or timeline'},indent=2)+'\n')
