#!/usr/bin/env python3
"""Fresh disposable server queue as a duplicate-playlist source; never plays/reports.
No existing queue or playlist is modified. Only returned fixture IDs are targets.
"""
import json,os,pathlib,urllib.parse,xml.etree.ElementTree as ET
import requests
from credential import load_legacy
import playlist_probe as fixture
out=[];queue=None;ctx={}
queue_manifest=pathlib.Path(os.environ.get('XDG_RUNTIME_DIR','/tmp'))/('qoda-private-source-queue-'+fixture.run+'.json')
def observe(name,**kw):v={'name':name,**kw};out.append(v);print(json.dumps(v),flush=True)
def call(method,path,params=None):return fixture.call(method,path,params)
def root(r):assert r.status_code==200;return ET.fromstring(r.content)
def main():
 global queue
 c=load_legacy();h={'Accept':'application/xml','X-Plex-Token':c['token'],'X-Plex-Client-Identifier':c['client'],'X-Plex-Product':'Qoda Plex Compatibility Validator'}
 r=requests.get('https://plex.tv/api/resources',headers=h,params={'includeHttps':1},timeout=(5,15),allow_redirects=False);s=next(x for x in root(r).findall('Device') if x.get('owned')=='1' and 'server' in x.get('provides','').split(','));h['X-Plex-Token']=s.get('accessToken') or c['token'];url=next(x.get('uri') for x in s.findall('Connection') if x.get('protocol')=='https').rstrip('/');fixture.context.update(url=url,headers=h)
 lib=next(x for x in root(call('GET','/library/sections')).findall('Directory') if x.get('type')=='artist');tracks=root(call('GET','/library/sections/'+lib.get('key')+'/all',{'type':10,'X-Plex-Container-Size':2,'X-Plex-Container-Start':0})).findall('Track');assert len(tracks)==2;a,b=[x.get('ratingKey') for x in tracks];base='server://'+s.get('clientIdentifier')+'/com.plexapp.plugins.library/library/metadata/'
 r=call('POST','/playQueues',{'type':'audio','uri':base+a+','+b,'shuffle':0,'repeat':0,'continuous':0});tree=root(r);queue=tree.get('playQueueID');assert queue and queue.isdigit();fd=os.open(queue_manifest,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
 with os.fdopen(fd,'w') as f:json.dump({'scope':'Only this fresh queue','queue_id':queue},f)
 observe('Create own temporary source queue',http_status=r.status_code,rows=len(tree.findall('Track')))
 r=call('PUT','/playQueues/'+queue,{'uri':base+a,'next':0});rows=root(r).findall('Track');duplicates=[x for x in rows if x.get('ratingKey')==a];observe('Append duplicate to own source queue',http_status=r.status_code,same_track_occurrences=len(duplicates),distinct_queue_occurrences=len({x.get('playQueueItemID') for x in duplicates})==len(duplicates) and len(duplicates)>1)
 title=fixture.namespace+' queue import';r=call('POST','/playlists',{'type':'audio','title':title,'smart':0,'playQueueID':queue});tree=root(r);node=tree.find('Playlist');assert node is not None and node.get('title')==title;ident=node.get('ratingKey');assert ident and ident.isdigit();f={'id':ident,'allowed_names':[title],'smart':False};fixture.created.append(f);fixture.save_manifest();rows=fixture.items(f);duplicates=[x for x in rows if x.get('ratingKey')==a]
 observe('Create disposable playlist from own repeated queue',http_status=r.status_code,rows=len(rows),same_track_occurrences=len(duplicates),distinct_playlist_occurrences=len({x.get('playlistItemID') for x in duplicates})==len(duplicates) and len(duplicates)>1)
 if len(duplicates)>1:
  target=duplicates[-1].get('playlistItemID');assert target;before=[x.get('playlistItemID') for x in rows];fixture.current(f);r=call('PUT',f'/playlists/{ident}/items/{target}/move',{'after':0});moved=fixture.items(f);observe('Move one actual duplicate occurrence',http_status=r.status_code,moved_to_front=moved[0].get('playlistItemID')==target,preserved_other_occurrence=duplicates[0].get('playlistItemID') in [x.get('playlistItemID') for x in moved])
  fixture.current(f);r=call('DELETE',f'/playlists/{ident}/items/{target}');remaining=fixture.items(f);observe('Remove one actual duplicate occurrence',http_status=r.status_code,one_same_track_occurrence_remains=sum(x.get('ratingKey')==a for x in remaining)==len(duplicates)-1,requested_occurrence_removed=target not in [x.get('playlistItemID') for x in remaining])
if __name__=='__main__':
 try:main()
 except Exception as e:observe('Duplicate source probe stopped',result='unverified',error_class=type(e).__name__)
 finally:
  for f in list(fixture.created):
   try:
    fixture.current(f);r=call('DELETE','/playlists/'+f['id']);v=call('GET','/playlists/'+f['id']);removed=r.status_code in (200,204) and v.status_code in (404,410);observe('Remove only own queue-import playlist',removed=removed)
    if removed:fixture.created.remove(f);fixture.save_manifest()
   except Exception as e:observe('Own imported playlist cleanup incomplete',error_class=type(e).__name__)
  if not fixture.created:fixture.manifest.unlink(missing_ok=True)
  if queue:
   try:
    r=call('DELETE','/playQueues/'+queue+'/items');v=call('GET','/playQueues/'+queue,{'window':10});empty=v.status_code==404 or (v.status_code==200 and not root(v).findall('Track'));observe('Clear only own temporary source queue',http_status=r.status_code,empty=empty)
    if empty:queue_manifest.unlink(missing_ok=True)
   except Exception as e:observe('Own queue cleanup incomplete',error_class=type(e).__name__)
  (fixture.HERE/'evidence/duplicate-queue.json').write_text(json.dumps({'scope':'Only fresh returned queue/playlist IDs; no playback/timeline call; no existing object targeted','observations':out,'remaining_fixture_playlist_count':len(fixture.created)},indent=2)+'\n')
