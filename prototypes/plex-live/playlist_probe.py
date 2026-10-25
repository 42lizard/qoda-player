#!/usr/bin/env python3
"""Only creates/mutates/deletes its own fresh UUID-named disposable fixtures.
No existing playlist is a mutation target. No automatic mutation retries.
"""
import json,os,pathlib,uuid,urllib.parse,xml.etree.ElementTree as ET
import requests
from credential import load_legacy
HERE=pathlib.Path(__file__).resolve().parent
observations=[];created=[];context={}
run=str(uuid.uuid4());namespace='Qoda disposable validation '+run
manifest=pathlib.Path(os.environ.get('XDG_RUNTIME_DIR','/tmp'))/('qoda-plex-fixture-'+run+'.json')
def observe(name,**kw):v={'name':name,**kw};observations.append(v);print(json.dumps(v),flush=True)
def call(method,path,params=None):return requests.request(method,context['url']+path,headers=context['headers'],params=params,timeout=(5,15),allow_redirects=False)
def xml(r):return ET.fromstring(r.content) if r.status_code==200 else None
def save_manifest():
 fd=os.open(manifest,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
 with os.fdopen(fd,'w') as f:json.dump({'scope':'Only this fresh test run','created':created},f)
def current(f):
 assert f in created
 r=call('GET','/playlists/'+f['id']);assert r.status_code==200
 node=xml(r).find('Playlist');assert node is not None and node.get('title') in f['allowed_names'];return node

def items(f):
 current(f);r=call('GET','/playlists/'+f['id']+'/items',{'X-Plex-Container-Start':0,'X-Plex-Container-Size':10});assert r.status_code==200
 return xml(r).findall('Track')
def create(title,uri,smart=False):
 r=call('POST','/playlists',{'type':'audio','title':title,'smart':int(smart),'uri':uri})
 observe('Create disposable '+('smart' if smart else 'plain')+' playlist',http_status=r.status_code)
 if r.status_code!=200:return None
 node=xml(r).find('Playlist');assert node is not None and node.get('title')==title
 ident=node.get('ratingKey');assert ident and ident.isdigit()
 f={'id':ident,'allowed_names':[title],'smart':smart};created.append(f);save_manifest();return f

def main():
 c=load_legacy();h={'Accept':'application/xml','X-Plex-Token':c['token'],'X-Plex-Client-Identifier':c['client'],'X-Plex-Product':'Qoda Plex Compatibility Validator'}
 cloud=requests.get('https://plex.tv/api/resources',headers=h,params={'includeHttps':1},timeout=(5,15),allow_redirects=False);assert cloud.status_code==200
 servers=[s for s in ET.fromstring(cloud.content).findall('Device') if s.get('owned')=='1' and 'server' in s.get('provides','').split(',')];assert len(servers)==1
 s=servers[0];h['X-Plex-Token']=s.get('accessToken') or c['token'];url=next(v.get('uri') for v in s.findall('Connection') if v.get('protocol')=='https').rstrip('/');context.update(url=url,headers=h)
 ident=call('GET','/identity');assert ident.status_code==200 and xml(ident).get('machineIdentifier')==s.get('clientIdentifier')
 sections=call('GET','/library/sections');assert sections.status_code==200;library=next(x for x in xml(sections).findall('Directory') if x.get('type')=='artist');key=library.get('key')
 r=call('GET',f'/library/sections/{urllib.parse.quote(key,safe="")}/all',{'type':10,'X-Plex-Container-Start':0,'X-Plex-Container-Size':3});assert r.status_code==200;tracks=xml(r).findall('Track');assert len(tracks)==3
 a,b,c=[x.get('ratingKey') for x in tracks];base='server://'+s.get('clientIdentifier')+'/com.plexapp.plugins.library'
 uri=base+'/library/metadata/'+a+','+b
 fixture=create(namespace,uri)
 if fixture:
  before=items(fixture);observe('Read fresh fixture entries',rows=len(before),has_occurrence_ids=all(bool(x.get('playlistItemID')) for x in before))
  renamed=namespace+' renamed';fixture['allowed_names'].append(renamed);save_manifest();current(fixture);r=call('PUT','/playlists/'+fixture['id'],{'title':renamed});observe('Rename own fixture',http_status=r.status_code,title_verified=current(fixture).get('title')==renamed)
  current(fixture);r=call('PUT','/playlists/'+fixture['id']+'/items',{'uri':base+'/library/metadata/'+c});added=items(fixture);observe('Append distinct track to own fixture',http_status=r.status_code,order_preserved=[x.get('ratingKey') for x in added]==[a,b,c],rows=len(added))
  current(fixture);r=call('PUT','/playlists/'+fixture['id']+'/items',{'uri':base+'/library/metadata/'+a});rows=items(fixture);duplicates=[x for x in rows if x.get('ratingKey')==a]
  observe('Add duplicate occurrence to own fixture',http_status=r.status_code,rows=len(rows),duplicate_occurrences=len(duplicates),distinct_occurrence_ids=len({x.get('playlistItemID') for x in duplicates})==len(duplicates) and len(duplicates)>=2)
  if len(rows)>=2 and all(x.get('playlistItemID') for x in rows):
   order=[x.get('playlistItemID') for x in rows];moving=order[-1];current(fixture);r=call('PUT',f'/playlists/{fixture["id"]}/items/{moving}/move',{'after':0});new=items(fixture);neworder=[x.get('playlistItemID') for x in new]
   observe('Reorder occurrence in own fixture',http_status=r.status_code,moved_to_front=bool(neworder) and neworder[0]==moving,same_occurrence_membership=set(neworder)==set(order))
   target=next(x.get('playlistItemID') for x in new if x.get('ratingKey')==a);current(fixture);r=call('DELETE',f'/playlists/{fixture["id"]}/items/{target}');remaining=items(fixture)
   observe('Remove occurrence from own fixture',http_status=r.status_code,removed_only_requested_occurrence=[x.get('playlistItemID') for x in remaining]==[x.get('playlistItemID') for x in new if x.get('playlistItemID')!=target],remaining_same_track_occurrences=sum(x.get('ratingKey')==a for x in remaining))
  else:observe('Reorder/remove checks',result='unverified; occurrence IDs not established')
 duplicate_fixture=create(namespace+' duplicate source',base+'/library/metadata/'+a+','+b+','+a)
 if duplicate_fixture:
  dup_rows=items(duplicate_fixture);dup_occurrences=[x for x in dup_rows if x.get('ratingKey')==a];observe('Duplicate source URI at creation',rows=len(dup_rows),same_track_occurrences=len(dup_occurrences),distinct_occurrence_ids=len(dup_occurrences)>=2 and len({x.get('playlistItemID') for x in dup_occurrences})==len(dup_occurrences))
 # Smart fixture changes only this new playlist; the filter references existing metadata read-only.
 smart_uri=base+f'/library/sections/{key}/all?type=10&limit=2'
 smart=create(namespace+' smart',smart_uri,True)
 if smart:
  node=current(smart);rows=items(smart);observe('Own smart playlist metadata',smart_flag=node.get('smart'),bounded_rows=len(rows))
  r=call('PUT','/playlists/'+smart['id']+'/items',{'uri':base+'/library/metadata/'+a});observe('Direct edit response on own smart fixture',http_status=r.status_code,smart_flag_after=current(smart).get('smart'),permission_loss_tested=False)
if __name__=='__main__':
 try:main()
 except Exception as e:observe('Playlist fixture probe stopped',result='unverified',error_class=type(e).__name__,automatic_retry=False)
 finally:
  for f in list(created):
   try:
    current(f);r=call('DELETE','/playlists/'+f['id']);verify=call('GET','/playlists/'+f['id']);removed=r.status_code in (200,204) and verify.status_code in (404,410)
    observe('Delete and verify own '+('smart' if f['smart'] else 'plain')+' fixture',delete_http_status=r.status_code,verify_http_status=verify.status_code,removed=removed)
    if removed:created.remove(f);save_manifest()
   except Exception as e:observe('Own fixture cleanup incomplete',error_class=type(e).__name__)
  if not created:manifest.unlink(missing_ok=True)
  (HERE/'evidence/playlists.json').write_text(json.dumps({'scope':'fresh UUID namespace; mutation guard uses only IDs returned by this run; no pre-existing playlist targeted','observations':observations,'remaining_fixture_count':len(created),'limits':['No blind retries of mutation calls','No shared-account permission-loss test','A changed smart fixture response is not a blanket read-only permission guarantee']},indent=2)+'\n')
