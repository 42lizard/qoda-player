#!/usr/bin/env python3
"""Account-authorized read-only comparison. No private response is printed/exported."""
import json,pathlib,subprocess,urllib.parse,xml.etree.ElementTree as ET
import argparse
import requests
from credential import load,store,load_legacy
HERE=pathlib.Path(__file__).resolve().parent
report={'scope':'single owned server, read-only music metadata and bounded artwork prefix; no mutations, timeline or music bytes','observations':[],'untested':['playlist CRUD/reorder/duplicate occurrences','smart/read-only permission fixture','shared-server permission loss','original media decision/Range/reconnect','playback reporting','actual expiration/revocation','locked-keyring UX and cleanup races','gain units and provenance']}
def observe(name,**kw):report['observations'].append(dict(name=name,**kw));print(json.dumps(report['observations'][-1]),flush=True)
def req(url,headers,params=None,stream=False):return requests.get(url,headers=headers,params=params,timeout=(5,15),allow_redirects=False,stream=stream)
def main():
 if LEGACY:
  credential=load_legacy();observe('Explicit legacy credential selection',credential_shape='jwt' if credential['token'].count('.')==2 else 'opaque')
 else:
  credential,repaired=load();assert credential['client'].startswith('qoda-validation-') and credential.get('token')
  pub=subprocess.check_output(['openssl','pkey','-pubout','-outform','DER'],input=credential['key'].encode(),stderr=subprocess.DEVNULL,timeout=5)
  saved=store(credential);assert saved
  observe('Recovered own credential and versioned envelope round trip',legacy_control_repair_needed=repaired,restored_private_key_valid=len(pub)==44,verified_round_trip=saved)
 common={'X-Plex-Product':'Qoda Plex Validator','X-Plex-Version':'0.0.0-validation','X-Plex-Client-Identifier':credential['client'],'Accept':'application/xml'}
 cloud=req('https://plex.tv/api/resources',{**common,'X-Plex-Token':credential['token']},{'includeHttps':1});assert cloud.status_code==200
 servers=[x for x in ET.fromstring(cloud.content).findall('Device') if 'server' in x.get('provides','').split(',') and x.get('owned')=='1'];assert len(servers)==1
 server=servers[0];resource_token=server.get('accessToken');assert resource_token
 endpoint=None;auth=None
 for c in sorted(server.findall('Connection'),key=lambda c:c.get('relay')=='1'):
  if c.get('protocol')!='https':continue
  url=c.get('uri','').rstrip('/');assert urllib.parse.urlsplit(url).scheme=='https'
  try:
   identity=req(url+'/identity',common)
   version=ET.fromstring(identity.content).get('version','unknown') if identity.status_code==200 else 'unknown'
   attempts=[]
   for kind,token in [('account-legacy' if LEGACY else 'account-jwt',credential['token']),('resource-issued',resource_token)]:
    h={**common,'X-Plex-Token':token};r=req(url+'/',h);attempts.append({'credential_kind':kind,'http_status':r.status_code,'credential_shape':'jwt' if token.count('.')==2 else 'opaque'})
    if kind=='resource-issued' and r.status_code==200:endpoint=url;auth=h
   observe('Secure PMS credential comparison',pms_version=version,relay=c.get('relay')=='1',tls_certificate_verified=True,attempts=attempts)
   if endpoint:break
  except requests.exceptions.SSLError:observe('Secure PMS connection',result='certificate-verification-failed')
  except requests.exceptions.RequestException:observe('Secure PMS connection',result='connection-failed')
 if not endpoint:observe('PMS access',result='unverified');return
 sections=None
 for route,version_header in [('/library/sections','0.0'),('/library/sections/all','1.0.0'),('/library/sections/all','1.2.3')]:
  r=req(endpoint+route,{**auth,'X-Plex-Pms-Api-Version':version_header})
  count=None
  if r.status_code==200:
   page=ET.fromstring(r.content);music=[x for x in page.findall('Directory') if x.get('type')=='artist'];count=len(music)
   if sections is None and music:sections=music
  observe('Library route/version-header response',route=route,requested_api_version=version_header,http_status=r.status_code,music_libraries=count,negotiated_api_version_proven=False)
 if not sections:observe('Music library access',result='no readable music library');return
 library=sections[0];key=urllib.parse.quote(library.get('key',''),safe='');pages=[];track=None
 for offset in (0,5):
  h={**auth,'X-Plex-Container-Start':str(offset),'X-Plex-Container-Size':'5'}
  r=req(endpoint+f'/library/sections/{key}/all',h,{'type':10});rows=ET.fromstring(r.content).findall('Track') if r.status_code==200 else [];container=ET.fromstring(r.content) if r.status_code==200 else None
  pages.append([x.get('ratingKey') for x in rows]);track=track if track is not None else (rows[0] if rows else None)
  observe('Bounded track page',requested_offset=offset,requested_size=5,http_status=r.status_code,rows=len(rows),returned_offset=container.get('offset') if container is not None else None,total=container.get('totalSize') if container is not None else None,track_attributes=sorted(set(k for row in rows for k in row.attrib)),nested_elements=sorted(set(e.tag for row in rows for e in row.iter())))
 observe('Adjacent track-page identities',non_overlapping=not set(pages[0]).intersection(pages[1]),bounded=all(len(p)<=5 for p in pages),counts=[len(p) for p in pages])
 if track is not None:
  ref=track.get('ratingKey','');r=req(endpoint+'/library/metadata/'+urllib.parse.quote(ref,safe=''),auth,{'includeMedia':1});detail=ET.fromstring(r.content) if r.status_code==200 else track
  names=sorted({k for e in detail.iter() for k in e.attrib if any(t in k.lower() for t in ('gain','peak','loudness'))})
  observe('Track detail and optional gain fields',http_status=r.status_code,gain_related_fields=names,units_proven=False,provenance_proven=False,has_media=bool(detail.findall('.//Media')),has_part=bool(detail.findall('.//Part')))
  r=req(endpoint+'/hubs/search',auth,{'query':track.get('title',''),'sectionId':library.get('key'),'type':10,'limit':3})
  hubs=ET.fromstring(r.content).findall('Hub') if r.status_code==200 else []
  observe('Music search',http_status=r.status_code,hub_types=sorted(set(x.get('type','unknown') for x in hubs)),track_rows=sum(len(x.findall('Track')) for x in hubs),query_exported=False)
  art=track.get('thumb') or track.get('parentThumb')
  if art and art.startswith('/') and not art.startswith('//'):
   r=req(endpoint+art,auth,stream=True);prefix=next(r.iter_content(128),b'');r.close();observe('Authenticated artwork prefix',http_status=r.status_code,bytes_retained=len(prefix),content_type=r.headers.get('Content-Type'),etag_present=bool(r.headers.get('ETag')),last_modified_present=bool(r.headers.get('Last-Modified')))
   condition={}
   if r.headers.get('ETag'):condition['If-None-Match']=r.headers['ETag']
   elif r.headers.get('Last-Modified'):condition['If-Modified-Since']=r.headers['Last-Modified']
   if condition:
    c=req(endpoint+art,{**auth,**condition},stream=True);observe('Artwork conditional revalidation',http_status=c.status_code,actual_artwork_replacement_tested=False);c.close()
 r=req(endpoint+'/playlists',auth,{'playlistType':'audio','X-Plex-Container-Start':0,'X-Plex-Container-Size':5})
 playlists=ET.fromstring(r.content).findall('Playlist') if r.status_code==200 else []
 observe('Audio playlist read-only discovery',http_status=r.status_code,rows=len(playlists),smart_flags_present=all('smart' in x.attrib for x in playlists),smart_rows=sum(x.get('smart')=='1' for x in playlists),mutations_performed=False)
if __name__=='__main__':
 parser=argparse.ArgumentParser();parser.add_argument('--legacy',action='store_true');LEGACY=parser.parse_args().legacy
 try:main()
 except Exception as e:observe('Read-only probe stopped',result='unverified',error_class=type(e).__name__)
 finally:(HERE/('evidence/legacy-read-only.json' if LEGACY else 'evidence/read-only.json')).write_text(json.dumps(report,indent=2)+'\n')
