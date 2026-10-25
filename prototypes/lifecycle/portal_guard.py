#!/usr/bin/env python3
"""Private bus activation boundary regression: never launch host desktop services."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

SERVICES=['xdg-desktop-portal.service','xdg-desktop-portal-hyprland.service','xdg-desktop-portal-gtk.service','xdg-document-portal.service']

def host():
    result={}
    for name in SERVICES:
        text=subprocess.check_output(['systemctl','--user','show',name,'-p','MainPID','-p','ActiveState'],text=True)
        result[name]=dict(line.split('=',1) for line in text.splitlines() if '=' in line)
    return result

def check_bus(address):
    def bus(*args):return subprocess.run(['busctl','--address='+address,*args],capture_output=True,text=True,timeout=4)
    names=bus('call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','ListActivatableNames')
    if names.returncode:raise RuntimeError('activation-list-unavailable')
    # A session bus may advertise itself. No service directories or standard session configuration may be loaded.
    if names.stdout.strip() not in ('as 0','as 1 "org.freedesktop.DBus"'):
        raise RuntimeError('private-bus-can-activate-external-services:'+names.stdout.strip())
    denied={}
    for name in ('org.freedesktop.portal.Desktop','org.freedesktop.impl.portal.desktop.hyprland','org.freedesktop.portal.Documents','org.freedesktop.secrets'):
        r=bus('call','org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus','StartServiceByName','su',name,'0')
        denied[name]=r.returncode!=0 and 'not provided by any .service files' in r.stderr
    if not all(denied.values()):raise RuntimeError('unexpected-service-activation:'+json.dumps(denied))
    return {'activatable':names.stdout.strip(),'activationDenied':denied}

def main():
    before=host()
    with tempfile.TemporaryDirectory(prefix='qoda-portal-guard-') as d:
        root=Path(d);root.chmod(0o700);path=root/'bus'
        config=root/'bus.conf'
        config.write_text('<busconfig><type>session</type><listen>unix:path='+str(path)+'</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        p=subprocess.Popen(['dbus-daemon','--nofork','--config-file='+str(config)],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        try:
            for _ in range(100):
                if path.exists():break
                time.sleep(.02)
            result=check_bus('unix:path='+str(path))
        finally:p.terminate();p.wait(timeout=4)
    after=host()
    result.update({'hostPortalProcessesUnchanged':before==after,'hostServicesRunning':all(s['ActiveState']=='active' for s in after.values()),'status':'pass' if before==after else 'fail'})
    target=Path(__file__).resolve().parent/'evidence/portal-isolation.json'
    target.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
    if before!=after:raise RuntimeError('host-portal-state-changed')

if __name__=='__main__':main()
