"""Narrow disposable app/MPRIS control surface, same owner-thread callback."""
import json,os,socket
import dbus,dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib
PLAYER='org.mpris.MediaPlayer2.Player'
class Rejected(dbus.DBusException):_dbus_error_name='org.freedesktop.DBus.Error.NotSupported'
class Props(dbus.service.Object):
 def __init__(self,request,snapshot):
  self.request=request;self.snapshot=snapshot;bus=dbus.SessionBus();self.name=dbus.service.BusName('org.mpris.MediaPlayer2.qoda_alsa_qualification',bus);super().__init__(bus,'/org/mpris/MediaPlayer2')
 @dbus.service.method('org.freedesktop.DBus.Properties',in_signature='ss',out_signature='v')
 def Get(self,interface,name):
  s=self.snapshot()
  if interface==PLAYER and name=='Volume':return dbus.Double(s.get('volume',100)/100)
  if interface==PLAYER and name=='PlaybackStatus':return 'Paused' if s.get('pause',True) else 'Playing'
  if interface=='org.mpris.MediaPlayer2' and name=='Identity':return 'Qoda ALSA qualification prototype'
  raise Rejected('Property outside qualification scope')
 @dbus.service.method('org.freedesktop.DBus.Properties',in_signature='ssv',out_signature='')
 def Set(self,interface,name,value):
  if interface!=PLAYER or name!='Volume':raise Rejected('Property outside qualification scope')
  r=self.request({'op':'set_volume','value':float(value)})
  if not r.get('accepted'):raise Rejected(r.get('error','Rejected'))
 @dbus.service.method('org.freedesktop.DBus.Properties',in_signature='s',out_signature='a{sv}')
 def GetAll(self,interface):return {'Volume':self.Get(PLAYER,'Volume'),'PlaybackStatus':self.Get(PLAYER,'PlaybackStatus')} if interface==PLAYER else {}
class Control:
 def __init__(self,path,request,snapshot):
  DBusGMainLoop(set_as_default=True);self.props=Props(request,snapshot);self.request=request;self.snapshot=snapshot;self.socket=socket.socket(socket.AF_UNIX);self.socket.bind(path);os.chmod(path,0o600);self.socket.listen(4);self.socket.setblocking(False);self.context=GLib.MainContext.default()
 def pump(self):
  for _ in range(8):
   if not self.context.pending():break
   self.context.iteration(False)
  try:peer,_=self.socket.accept()
  except BlockingIOError:return
  with peer:
   peer.settimeout(.2);data=peer.recv(4096);q=json.loads(data);response=self.snapshot() if q.get('op')=='snapshot' else self.request(q);peer.sendall(json.dumps(response).encode()+b'\n')
 def close(self):self.props.remove_from_connection();self.socket.close()
