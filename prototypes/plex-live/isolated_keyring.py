#!/usr/bin/env python3
"""Own private bus + temporary XDG directories, fictional markers only.
Never locks, stops or reads an item from the desktop's real keyring.
"""
import json,os,pathlib,subprocess,sys,tempfile,time,uuid
HERE=pathlib.Path(__file__).resolve().parent

def worker():
 import dbus
 control=pathlib.Path(os.environ['XDG_RUNTIME_DIR'])/'keyring';control.mkdir(mode=0o700)
 daemon=subprocess.Popen(['gnome-keyring-daemon','--foreground','--components=secrets','--control-directory='+str(control),'--unlock'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 daemon.stdin.write(b'fictional-private-fixture-password');daemon.stdin.close();results=[]
 try:
  bus=dbus.SessionBus();end=time.monotonic()+6
  while time.monotonic()<end and not bus.name_has_owner('org.freedesktop.secrets'):time.sleep(.05)
  assert bus.name_has_owner('org.freedesktop.secrets')
  attrs=['application','qoda-isolated-secret-fixture','run',str(uuid.uuid4())];marker=b'fictional-persisted-marker'
  p=subprocess.run(['secret-tool','store','--label=Isolated Qoda fixture',*attrs],input=marker,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
  q=subprocess.run(['secret-tool','lookup',*attrs],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=5);results.append({'name':'Private service fixture store/read','pass':p.returncode==0 and q.stdout==marker})
  svc=dbus.Interface(bus.get_object('org.freedesktop.secrets','/org/freedesktop/secrets'),'org.freedesktop.Secret.Service');collection=svc.ReadAlias('default');unlocked,_=svc.SearchItems({'application':'qoda-isolated-secret-fixture','run':attrs[-1]});assert len(unlocked)==1
  _,session=svc.OpenSession('plain',dbus.String('',variant_level=1));svc.Lock([collection]);props=dbus.Interface(bus.get_object('org.freedesktop.secrets',collection),'org.freedesktop.DBus.Properties');locked=bool(props.Get('org.freedesktop.Secret.Collection','Locked'));error=None
  try:dbus.Interface(bus.get_object('org.freedesktop.secrets',unlocked[0]),'org.freedesktop.Secret.Item').GetSecret(session)
  except dbus.DBusException as e:error=e.get_dbus_name()
  results.append({'name':'Private locked collection rejects secret access','pass':locked and error=='org.freedesktop.Secret.Error.IsLocked','error_name':error})
  # Explicit fixture-only session choice: new value remains in memory, not a store call.
  session_only=b'fictional-session-only-marker';assert session_only!=marker
  p=subprocess.run(['gnome-keyring-daemon','--unlock','--control-directory='+str(control)],input=b'fictional-private-fixture-password',stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
  end=time.monotonic()+3
  while time.monotonic()<end and bool(props.Get('org.freedesktop.Secret.Collection','Locked')):time.sleep(.05)
  q=subprocess.run(['secret-tool','lookup',*attrs],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=5)
  results.append({'name':'Private unlock restores old stored value; session-only choice did not persist','pass':p.returncode==0 and not bool(props.Get('org.freedesktop.Secret.Collection','Locked')) and q.stdout==marker,'unlock_exit':p.returncode,'collection_locked_after':bool(props.Get('org.freedesktop.Secret.Collection','Locked')),'old_marker_restored':q.stdout==marker,'lookup_exit':q.returncode})
  subprocess.run(['secret-tool','clear',*attrs],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
  dbus.Interface(bus.get_object('org.freedesktop.secrets',session),'org.freedesktop.Secret.Session').Close()
 except Exception as e:results.append({'name':'Isolated keyring fixture stopped','pass':False,'error_class':type(e).__name__})
 finally:
  daemon.terminate()
  try:daemon.wait(timeout=3)
  except subprocess.TimeoutExpired:daemon.kill();daemon.wait(timeout=3)
 print(json.dumps({'observations':results,'limits':['Synthetic private-bus mechanism fixture, not production UI/session-only integration','Actual desktop collection was never locked or stopped','No account credentials used']}))

def main():
 with tempfile.TemporaryDirectory(prefix='qoda-private-keyring-') as temp:
  env=dict(os.environ)
  for k in ['DISPLAY','WAYLAND_DISPLAY','GNOME_KEYRING_CONTROL','SSH_AUTH_SOCK']:env.pop(k,None)
  for k,name in [('XDG_DATA_HOME','data'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime')]:
   p=pathlib.Path(temp)/name;p.mkdir(mode=0o700);env[k]=str(p)
  p=subprocess.run(['dbus-run-session','--',sys.executable,str(pathlib.Path(__file__).resolve()),'--worker'],env=env,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,timeout=35)
  report=json.loads(p.stdout.strip().splitlines()[-1]);(HERE/'evidence/isolated-keyring.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':
 if '--worker' in sys.argv:worker()
 else:main()
