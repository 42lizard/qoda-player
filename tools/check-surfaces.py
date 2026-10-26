#!/usr/bin/env python3
"""Native UI/MPRIS checks in an empty private session; no credentials or audio.

Build qoda-backend first. Requires a running Wayland desktop, Quickshell,
busctl and an unused Qoda MPRIS name. Does not modify desktop configuration; stops its owned processes.
"""
import os,subprocess,tempfile,socket,json,time,sqlite3,uuid
from pathlib import Path
root=Path(__file__).resolve().parent.parent
name='org.mpris.MediaPlayer2.qoda';path='/org/mpris/MediaPlayer2';player='org.mpris.MediaPlayer2.Player'
def bus(*args,ok=True):
 r=subprocess.run(['/usr/bin/busctl','--user','--timeout=4',*args],capture_output=True,text=True,timeout=6)
 if ok:assert r.returncode==0,'D-Bus command failed: '+r.stderr
 return r
assert bus('get-property',name,path,player,'PlaybackStatus',ok=False).returncode!=0,'A Qoda instance already owns the desktop bus; it was not touched'
with tempfile.TemporaryDirectory(prefix='qoda-mpris-check-') as temp:
 env=dict(os.environ);old=env['XDG_RUNTIME_DIR'];env.update(XDG_RUNTIME_DIR=temp+'/runtime',XDG_CONFIG_HOME=temp+'/config',XDG_STATE_HOME=temp+'/state',XDG_CACHE_HOME=temp+'/cache');Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
 env['DBUS_SESSION_BUS_ADDRESS']=os.environ.get('DBUS_SESSION_BUS_ADDRESS','unix:path='+old+'/bus')
 if not env.get('WAYLAND_DISPLAY','').startswith('/'):env['WAYLAND_DISPLAY']=old+'/'+env.get('WAYLAND_DISPLAY','wayland-1')
 cache=Path(temp+'/cache/qoda/artwork-v1');cache.mkdir(parents=True,mode=0o700)
 cached=cache/('0'*64+'.jpg');cached.write_bytes((root/'crates/qoda-provider/tests/fixtures/cover.jpg').read_bytes());cached.chmod(0o600)
 metadata=Path(temp+'/cache/qoda/catalog-v1');metadata.mkdir(parents=True,mode=0o700)
 database=metadata/'catalog.sqlite3';source=str(uuid.uuid4())
 payload=json.dumps({'kind':'tracks','page':{'source':source,'offset':'0','total':None,'query':'','tracks':[],'has_more':False}}).encode()
 with sqlite3.connect(database) as db:
  db.executescript('PRAGMA auto_vacuum=INCREMENTAL;CREATE TABLE pages(key TEXT PRIMARY KEY,source TEXT NOT NULL,payload BLOB NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL);CREATE TABLE tracks(source TEXT NOT NULL,key TEXT NOT NULL,payload BLOB NOT NULL,title TEXT NOT NULL,artist TEXT NOT NULL,album TEXT NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL,PRIMARY KEY(source,key));PRAGMA application_id=1364149315;PRAGMA user_version=1;')
  db.execute('INSERT INTO pages VALUES(?,?,?,?,?,?)',('fixture',source,payload,int(time.time()),1,len(payload)+128))
 database.chmod(0o600)

 # Reproduce the pre-audio-settings config directory created under umask 022.
 legacy_config=Path(temp+'/config/qoda');legacy_config.mkdir(parents=True);legacy_config.chmod(0o755)
 p=None;ui=None;f=None;s=None;hung=None;log=open(temp+'/ui.log','w+')
 try:
  def start(unavailable=False):
   global p,s,f,state,instance
   backend_env=dict(env)
   if unavailable:backend_env['DBUS_SESSION_BUS_ADDRESS']='unix:path='+temp+'/missing-session-bus'
   p=subprocess.Popen([str(root/'target/debug/qoda-backend')],env=backend_env,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
   for _ in range(150):
    assert p.poll() is None,'Backend exited'
    try:
     s=socket.socket(socket.AF_UNIX);s.settimeout(8);s.connect(env['XDG_RUNTIME_DIR']+'/qoda/backend.sock');f=s.makefile('rwb');break
    except OSError:time.sleep(.1)
   else:raise AssertionError('Backend socket did not start')
   f.write(b'{"op":"hello","protocol":1}\n');f.flush();state=json.loads(f.readline(2**24));instance=state['instance'];until(lambda v:v.get('mpris',{}).get('state')==('unavailable' if unavailable else 'available'))
  def command(op,**fields):f.write((json.dumps({'op':op,'instance':instance,**fields})+'\n').encode());f.flush()
  def until(predicate,allow_error=False):
   global state
   end=time.monotonic()+8
   while not predicate(state):
    assert time.monotonic()<end,'Operation timed out'
    state=json.loads(f.readline(2**24));assert allow_error or not state.get('error'),'Controlled error: '+str(state.get('error'))
  start();assert state['output']['available'] and state['replaygain']['available'];assert legacy_config.stat().st_mode & 0o777 == 0o700
  assert state['intent']=='paused' and not state['sign_in']['connected'];assert bus('get-property',name,path,player,'PlaybackStatus').stdout.strip()=='s "Stopped"'
  assert bus('get-property',name,path,player,'Metadata').stdout.strip()=='a{sv} 0';print('Native desktop bus registered with stopped, metadata-free, unsigned-in state',flush=True)

  assert bus('get-property',name,path,'org.mpris.MediaPlayer2','CanRaise').stdout.strip()=='b false'
  bus('call',name,path,'org.mpris.MediaPlayer2','Raise')
  assert state['metadata_cache']['pages']==1
  queue_before=state['queue']
  command('set_metadata_budget',mib=1);until(lambda v:v.get('metadata_cache',{}).get('budget_mib')==1 and not v.get('metadata_cache_action'))
  command('clear_metadata_cache');until(lambda v:v.get('metadata_cache',{}).get('pages')==0 and not v.get('metadata_cache_action') and not v.get('metadata_cache',{}).get('cleanup_pending'))
  assert state['queue']==queue_before and state['intent']=='paused' and 'clearing' not in state['status'].lower()
  print('Metadata target persisted and synthetic cached page cleared without queue or playback changes',flush=True)
  assert state['artwork_cache']['entries']==1
  command('set_artwork_budget',mib=1);until(lambda v:v.get('artwork_cache',{}).get('budget_mib')==1 and not v.get('cache_action'))
  command('clear_artwork_cache');until(lambda v:v.get('artwork_cache',{}).get('entries')==0 and not v.get('cache_action') and not v.get('artwork_cache',{}).get('cleanup_pending'))
  assert state['intent']=='paused' and not state['queue']['entries'] and not cached.exists()
  print('Cache budget persisted and synthetic artwork cleared without queue changes or playback',flush=True)
  assert state['gapless']['enabled']
  command('set_gapless',enabled=False);until(lambda v:not v['gapless']['enabled'])
  command('set_gapless',enabled=True);until(lambda v:v['gapless']['enabled']);assert state['intent']=='paused'
  bus('set-property',name,path,player,'Shuffle','b','true');until(lambda v:v['queue']['shuffled'])
  bus('set-property',name,path,player,'LoopStatus','s','Playlist');until(lambda v:v['queue']['repeat']=='all')
  before_gain_queue=state['queue']
  for mode in ['off','track','album','auto']:
   command('set_replaygain',mode=mode);until(lambda v:v['replaygain']['mode']==mode)
   assert state['queue']==before_gain_queue and state['intent']=='paused'
  command('set_replaygain',mode='invalid');until(lambda v:bool(v.get('error')),allow_error=True)
  assert state['error']=='Choose Off, Track, Album or Auto ReplayGain.'
  assert state['replaygain']['mode']=='auto' and state['queue']==before_gain_queue
  command('set_replaygain',mode='track');until(lambda v:v['replaygain']['mode']=='track' and not v.get('error'),allow_error=True)
  print('ReplayGain mode changes persisted without queue/playback changes; invalid mode rejected',flush=True)
  before_search=state['queue'];before_gapless=state['gapless']
  command('search_all',query='fictional');until(lambda v:bool(v.get('error')),allow_error=True)
  assert state['error']=='Connect Plex to search music libraries.' and state['search']['results']==[]
  command('search_play',source=str(uuid.uuid4()),key='42',search_revision=state['search']['revision'],queue_session=state['queue']['session'],queue_revision=state['queue']['revision'])
  until(lambda v:v.get('error')=='Connect Plex before using search results.',allow_error=True)
  assert state['queue']==before_search and state['gapless']==before_gapless and state['intent']=='paused'
  command('set_replaygain',mode='track');until(lambda v:not v.get('error'),allow_error=True)
  print('Offline cross-library search and result actions refused without changing queue, gapless plan or playback',flush=True)
  for method in ['Play','Pause','Stop','Next','Previous']:bus('call',name,path,player,method)
  bus('call',name,path,player,'Seek','x','1000000');bus('call',name,path,player,'SetPosition','ox','/org/mpris/MediaPlayer2/TrackList/NoTrack','1000000')
  assert bus('set-property',name,path,player,'Volume','d','0.5',ok=False).returncode!=0
  assert bus('set-property',name,path,player,'Rate','d','2.0',ok=False).returncode!=0
  assert bus('call',name,path,player,'OpenUri','s','file:///not-opened',ok=False).returncode!=0
  print('Desktop modes reached the coordinator; empty transport/seek controls stayed silent; unsupported controls rejected',flush=True)
  ui=subprocess.Popen(['quickshell','--path',str(root/'ui'),'--no-duplicate'],env=env,stdout=log,stderr=log)
  def ipc(method):
   r=subprocess.run(['quickshell','ipc','--path',str(root/'ui'),'call','--','control',method],env=env,capture_output=True,text=True,timeout=5)
   if r.returncode:raise ValueError('UI not ready')
   return r.stdout.strip()
  for _ in range(60):
   if ui.poll() is not None:log.flush();log.seek(0);raise AssertionError('QML failed: '+log.read())
   try:
    info=json.loads(ipc('status'))
    if info['connected'] and info['instance'] and info.get('presentation') is not None:break
   except ValueError:pass
   time.sleep(.1)
  else:raise AssertionError('UI never connected')
  until(lambda v:v.get('ui',{}).get('can_raise'))
  assert bus('get-property',name,path,'org.mpris.MediaPlayer2','CanRaise').stdout.strip()=='b true'
  ipc('showSearch');search_info=json.loads(ipc('status'));assert search_info['tab']==4 and search_info['search']['results']==[]
  ipc('showAudio');audio_settings=json.loads(ipc('status'));assert audio_settings['audioExpanded'] and audio_settings['replaygain']['mode']=='track'
  ipc('showCache');initial=json.loads(ipc('status'));assert initial['form']=='full' and initial['cacheExpanded'] and initial['artworkCache']['budget_mib']==1
  assert not initial['presentation']['known'] and initial['presentation']['track'] is None
  ipc('previewCompact');time.sleep(.3);compact=json.loads(ipc('status'))
  assert compact['form']=='compact' and compact['compact']['mapped'] and compact['compact']['keyboardMode']==0
  assert compact['compact']['width']<=440 and compact['compact']['height']<=340
  assert compact['instance']==initial['instance'] and compact['queue']==initial['queue']
  ipc('hide');assert not json.loads(ipc('status'))['visible']
  ipc('show');time.sleep(.3);compact=json.loads(ipc('status'))
  assert compact['form']=='compact' and compact['compact']['keyboardMode']!=0
  ipc('dismiss');assert not json.loads(ipc('status'))['visible']
  ipc('toggle');assert json.loads(ipc('status'))['form']=='compact'
  ipc('showFull');full=json.loads(ipc('status'));assert full['form']=='full' and not full['compact']['visible']
  assert full['query']==initial['query'] and full['tab']==initial['tab'] and full['instance']==initial['instance']
  assert full['settings']['section']=='cache' and full['settings']['visible']
  ipc('showSearch')
  for _ in range(60):
   focused=json.loads(ipc('status'))
   if focused['searchFocused'] and not focused['settings']['visible']:break
   time.sleep(.05)
  else:raise AssertionError('Search did not focus after settings closed')
  assert focused['tab']==4 and focused['form']=='full'
  ipc('dismiss');assert json.loads(ipc('status'))['form']=='full'
  for _ in range(4):
   ipc('showCompact');ipc('hide');ipc('showFull')
  assert json.loads(ipc('status'))['form']=='full'
  print('Compact passive/interactive focus, last-form restore, rapid switching and browser-state retention passed',flush=True)
  before=state['queue'];ipc('showCompact');ipc('hide');bus('call',name,path,'org.mpris.MediaPlayer2','Raise')
  for _ in range(60):
   raised=json.loads(ipc('status'))
   if raised['visible'] and raised['form']=='compact' and raised['compact']['mapped']:break
   time.sleep(.05)
  else:raise AssertionError('Desktop Raise did not restore the compact surface')
  until(lambda v:bool(v.get('ui',{}).get('raise_token')))
  assert state['queue']==before and state['intent']=='paused'
  ui.terminate();ui.wait(timeout=5);until(lambda v:not v['ui']['can_raise'])
  assert bus('get-property',name,path,'org.mpris.MediaPlayer2','CanRaise').stdout.strip()=='b false'
  ui=subprocess.Popen(['quickshell','--path',str(root/'ui'),'--no-duplicate'],env=env,stdout=log,stderr=log)
  for _ in range(60):
   assert ui.poll() is None,'Replacement UI exited'
   try:
    info=json.loads(ipc('status'))
    if info['instance']==instance:break
   except ValueError:pass
   time.sleep(.1)
  else:raise AssertionError('Replacement UI did not connect')
  until(lambda v:v['ui']['can_raise'])
  assert json.loads(ipc('status'))['form']=='full'
  print('Desktop Raise restored an attached hidden surface without playback changes; CanRaise followed UI disconnect/replacement',flush=True)

  ipc('hide');bus('set-property',name,path,player,'Shuffle','b','false');until(lambda v:not v['queue']['shuffled']);assert not json.loads(ipc('status'))['visible'];ipc('show')
  bus('set-property',name,path,player,'Shuffle','b','true');until(lambda v:v['queue']['shuffled']);print('Desktop controls remained active while native QML was hidden',flush=True)
  bus('call',name,path,'org.mpris.MediaPlayer2','Quit');p.wait(timeout=15);assert p.returncode==0;ui.wait(timeout=5);assert ui.returncode==0;f.close();f=None;s.close();s=None
  assert bus('get-property',name,path,player,'PlaybackStatus',ok=False).returncode!=0
  start();assert state['replaygain']['mode']=='track';assert state['metadata_cache']['budget_mib']==1;assert state['artwork_cache']['budget_mib']==1;assert state['queue']['shuffled'] and state['queue']['repeat']=='all' and state['intent']=='paused';assert bus('get-property',name,path,player,'Metadata').stdout.strip()=='a{sv} 0';print('MPRIS Quit closed backend/UI and released the name; restart retained modes without sign-in or autoplay',flush=True)
  command('quit');p.wait(timeout=15);assert p.returncode==0;f.close();f=None;s.close();s=None
  start(unavailable=True);assert state['intent']=='paused'
  ui=subprocess.Popen(['quickshell','--path',str(root/'ui'),'--no-duplicate'],env=env,stdout=log,stderr=log)
  for _ in range(60):
   assert ui.poll() is None,'Recovery UI exited'
   try:
    info=json.loads(ipc('status'))
    if info['instance']==instance and info['desktopControls']['state']=='unavailable':break
   except (ValueError,KeyError):pass
   time.sleep(.1)
  else:raise AssertionError('Recovery controls never loaded')
  hung=socket.socket(socket.AF_UNIX);hung.bind(temp+'/missing-session-bus');hung.listen()
  before=state['queue'];ipc('retryDesktopControls');until(lambda v:v['mpris']['state']=='connecting')
  command('set_gapless',enabled=False);until(lambda v:not v['gapless']['enabled']);assert state['mpris']['state']=='connecting'
  until(lambda v:v['mpris']['state']=='unavailable');assert state['queue']==before and state['intent']=='paused'
  hung.close();hung=None
  command('quit');p.wait(timeout=15);assert p.returncode==0;ui.wait(timeout=5);assert ui.returncode==0
  print('Native recovery controls started a bounded hung-bus retry while other controls stayed responsive; timeout remained nonfatal',flush=True)
  log.flush();log.seek(0);errors=[line for line in log.read().splitlines() if any(e in line for e in ['TypeError','ReferenceError','Cannot assign','Unable to assign','Failed to load','SyntaxError','Binding loop'])];assert not errors,'QML runtime error: '+repr(errors)
  print('No real music, credentials or Plex requests were used; scoped processes stopped',flush=True)
 finally:
  if hung:hung.close()
  if ui and ui.poll() is None:ui.terminate();ui.wait(timeout=5)
  if p and p.poll() is None:
   p.send_signal(2)
   try:p.wait(timeout=15)
   except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
  if f:f.close()
  if s:s.close()
  log.close()
