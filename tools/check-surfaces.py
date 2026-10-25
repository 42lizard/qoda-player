#!/usr/bin/env python3
"""Native UI/MPRIS checks in an empty private session; no credentials or audio.

Build qoda-backend first. Requires a running Wayland desktop, Quickshell,
busctl and an unused Qoda MPRIS name. Does not modify desktop configuration; stops its owned processes.
"""
import os,subprocess,tempfile,socket,json,time
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
 p=None;ui=None;f=None;s=None;log=open(temp+'/ui.log','w+')
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
  def until(predicate):
   global state
   end=time.monotonic()+8
   while not predicate(state):
    assert time.monotonic()<end,'Operation timed out'
    state=json.loads(f.readline(2**24));assert not state.get('error'),'Controlled error: '+str(state.get('error'))
  start();assert state['intent']=='paused' and not state['sign_in']['connected'];assert bus('get-property',name,path,player,'PlaybackStatus').stdout.strip()=='s "Stopped"'
  assert bus('get-property',name,path,player,'Metadata').stdout.strip()=='a{sv} 0';print('Native desktop bus registered with stopped, metadata-free, unsigned-in state',flush=True)
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
  ipc('dismiss');assert json.loads(ipc('status'))['form']=='full'
  for _ in range(4):
   ipc('showCompact');ipc('hide');ipc('showFull')
  assert json.loads(ipc('status'))['form']=='full'
  print('Compact passive/interactive focus, last-form restore, rapid switching and browser-state retention passed',flush=True)
  ipc('hide');bus('set-property',name,path,player,'Shuffle','b','false');until(lambda v:not v['queue']['shuffled']);assert not json.loads(ipc('status'))['visible'];ipc('show')
  bus('set-property',name,path,player,'Shuffle','b','true');until(lambda v:v['queue']['shuffled']);print('Desktop controls remained active while native QML was hidden',flush=True)
  bus('call',name,path,'org.mpris.MediaPlayer2','Quit');p.wait(timeout=15);assert p.returncode==0;ui.wait(timeout=5);assert ui.returncode==0;f.close();f=None;s.close();s=None
  assert bus('get-property',name,path,player,'PlaybackStatus',ok=False).returncode!=0
  start();assert state['artwork_cache']['budget_mib']==1;assert state['queue']['shuffled'] and state['queue']['repeat']=='all' and state['intent']=='paused';assert bus('get-property',name,path,player,'Metadata').stdout.strip()=='a{sv} 0';print('MPRIS Quit closed backend/UI and released the name; restart retained modes without sign-in or autoplay',flush=True)
  command('quit');p.wait(timeout=15);assert p.returncode==0;f.close();f=None;s.close();s=None
  start(unavailable=True);assert state['intent']=='paused';command('quit');p.wait(timeout=15);assert p.returncode==0;print('Unavailable session bus remained a visible, nonfatal feature limitation',flush=True)
  log.flush();log.seek(0);errors=[line for line in log.read().splitlines() if any(e in line for e in ['TypeError','ReferenceError','Cannot assign','Failed to load','SyntaxError'])];assert not errors,'QML runtime error: '+repr(errors)
  print('No real music, credentials or Plex requests were used; scoped processes stopped',flush=True)
 finally:
  if ui and ui.poll() is None:ui.terminate();ui.wait(timeout=5)
  if p and p.poll() is None:
   p.send_signal(2)
   try:p.wait(timeout=15)
   except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
  if f:f.close()
  if s:s.close()
  log.close()
