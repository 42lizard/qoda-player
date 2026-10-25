#!/usr/bin/env python3
"""Private D-Bus Locked property fixture. No credentials and no host keyring access."""
import argparse
from gi.repository import Gio,GLib

p=argparse.ArgumentParser();p.add_argument('--address',required=True);args=p.parse_args()
connection=Gio.DBusConnection.new_for_address_sync(args.address,Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT|Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,None,None)
xml='<node><interface name="org.freedesktop.Secret.Collection"><property name="Locked" type="b" access="read"/></interface></node>'
locked=True
interface=Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0]
connection.register_object('/org/freedesktop/secrets/aliases/default',interface,None,lambda *a:GLib.Variant('b',locked),None)
connection.call_sync('org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','RequestName',GLib.Variant('(su)',('org.freedesktop.secrets',0)),GLib.VariantType.new('(u)'),Gio.DBusCallFlags.NONE,3000,None)
# Minimal standard lookup/unlock/delete boundary, with fake metadata and no secrets.
items={'/org/freedesktop/secrets/collection/fixture/qoda':{'application':'qoda-lifecycle-fixture','account':'fictional'},'/org/freedesktop/secrets/collection/fixture/other':{'application':'other-fixture','account':'unrelated'}}
registrations={}
service_xml='<node><interface name="org.freedesktop.Secret.Service"><method name="SearchItems"><arg type="a{ss}" direction="in"/><arg type="ao" direction="out"/><arg type="ao" direction="out"/></method><method name="Unlock"><arg type="ao" direction="in"/><arg type="ao" direction="out"/><arg type="o" direction="out"/></method></interface></node>'
item_xml='<node><interface name="org.freedesktop.Secret.Item"><method name="Delete"><arg type="o" direction="out"/></method><property name="Label" type="s" access="read"/><property name="Locked" type="b" access="read"/><property name="Attributes" type="a{ss}" access="read"/></interface></node>'
def service_call(connection,sender,path,interface,method,parameters,invocation):
 global locked
 if method=='SearchItems':
  attributes=parameters.unpack()[0];found=[p for p,a in items.items() if all(a.get(k)==v for k,v in attributes.items())]
  invocation.return_value(GLib.Variant('(aoao)',([],found) if locked else (found,[])))
 elif method=='Unlock':
  requested=parameters.unpack()[0];locked=False
  invocation.return_value(GLib.Variant('(aoo)',([p for p in requested if p in items],'/')))
def item_call(connection,sender,path,interface,method,parameters,invocation):
 if locked:
  invocation.return_dbus_error('org.freedesktop.Secret.Error.IsLocked','Fixture is locked');return
 del items[path];connection.unregister_object(registrations.pop(path))
 invocation.return_value(GLib.Variant('(o)',('/',)))
def item_property(connection,sender,path,interface,name):
 if name=='Locked':return GLib.Variant('b',locked)
 if name=='Attributes':return GLib.Variant('a{ss}',items[path])
 return GLib.Variant('s','Qoda fixture' if items[path]['application']=='qoda-lifecycle-fixture' else 'Other fixture')
connection.register_object('/org/freedesktop/secrets',Gio.DBusNodeInfo.new_for_xml(service_xml).interfaces[0],service_call,None,None)
for path in items:
 registrations[path]=connection.register_object(path,Gio.DBusNodeInfo.new_for_xml(item_xml).interfaces[0],item_call,item_property,None)
# A second player on the same private bus tests client-side player selection.
player_interface='org.mpris.MediaPlayer2.Player'
root_interface='org.mpris.MediaPlayer2'
playing=False
root_values={'Identity':('s','Qoda selection fixture'),'DesktopEntry':('s','fixture-other-player'),'CanQuit':('b',False),'CanRaise':('b',False),'HasTrackList':('b',False),'SupportedUriSchemes':('as',[]),'SupportedMimeTypes':('as',[])}
player_values={'PlaybackStatus':('s','Paused'),'Metadata':('a{sv}',{}),'Position':('x',0),'Rate':('d',1.0),'Volume':('d',1.0),'MinimumRate':('d',1.0),'MaximumRate':('d',1.0),'CanControl':('b',True),'CanPlay':('b',True),'CanPause':('b',True),'CanSeek':('b',False),'CanGoNext':('b',False),'CanGoPrevious':('b',False)}
def xml_for(name,values,methods):
 return '<node><interface name="'+name+'">'+''.join('<property name="'+key+'" type="'+value[0]+'" access="read"/>' for key,value in values.items())+''.join('<method name="'+method+'"/>' for method in methods)+'</interface></node>'
def property_get(connection,sender,path,interface,name):
 values=player_values if interface==player_interface else root_values
 kind,value=values[name]
 if name=='PlaybackStatus':value='Playing' if playing else 'Paused'
 return GLib.Variant(kind,value)
def method_call(connection,sender,path,interface,method,parameters,invocation):
 global playing
 if interface==player_interface:
  if method=='Play':playing=True
  elif method in ('Pause','Stop'):playing=False
  elif method=='PlayPause':playing=not playing
  connection.emit_signal(None,path,'org.freedesktop.DBus.Properties','PropertiesChanged',GLib.Variant('(sa{sv}as)',(player_interface,{'PlaybackStatus':GLib.Variant('s','Playing' if playing else 'Paused')},[])))
 invocation.return_value(None)
for name,values,methods in [(root_interface,root_values,['Raise','Quit']),(player_interface,player_values,['Play','Pause','PlayPause','Stop','Next','Previous'])]:
 info=Gio.DBusNodeInfo.new_for_xml(xml_for(name,values,methods)).interfaces[0]
 connection.register_object('/org/mpris/MediaPlayer2',info,method_call,property_get,None)
connection.call_sync('org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','RequestName',GLib.Variant('(su)',('org.mpris.MediaPlayer2.fixture_peer',0)),GLib.VariantType.new('(u)'),Gio.DBusCallFlags.NONE,3000,None)
print('locked-fixture-ready',flush=True)
GLib.MainLoop().run()
