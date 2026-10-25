#!/usr/bin/env python3
"""Bounded native window on the existing session bus; no portal feature requests."""
import datetime
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from portal_guard import host

QML='''//@ pragma ShellId qoda-player-host-portal-smoke
//@ pragma AppId qoda-player
import QtQuick
import Quickshell
import Quickshell.Io
ShellRoot {
 IpcHandler {
  target:"portalSmoke"
  function snapshot():string{return JSON.stringify({appId:Quickshell.appId,visible:window.visible,mapped:window.backingWindowVisible});}
  function visible(value:bool):void{window.visible=value;}
  function quit():void{Qt.quit();}
 }
 FloatingWindow {
  id:window;visible:true;implicitWidth:420;implicitHeight:180;color:"#18261d"
  title:"Qoda lifecycle portal check · fictional data"
  Text {anchors.centerIn:parent;text:"Qoda · portal registration check\\nFictional data · no audio";color:"#d0e8bb"}
 }
}
'''

def main():
    before=host()
    if not all(s['ActiveState']=='active' for s in before.values()):raise RuntimeError('host-portals-not-already-running')
    since=datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%d %H:%M:%S UTC')
    root=Path(tempfile.mkdtemp(prefix='qoda-host-portal-'));root.chmod(0o700)
    config=root/'shell.qml';config.write_text(QML)
    env=dict(os.environ,QT_QUICK_CONTROLS_STYLE='Basic',NO_AT_BRIDGE='1',QT_ACCESSIBILITY='0')
    for key,folder in [('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_CACHE_HOME','cache')]:
        (root/folder).mkdir();env[key]=str(root/folder)
    def ipc(method,*args):
        r=subprocess.run(['quickshell','ipc','-p',str(config),'call','--','portalSmoke',method,*args],capture_output=True,text=True,timeout=2)
        if r.returncode:raise RuntimeError('smoke-ipc-unavailable')
        return r.stdout.strip()
    error=None;transitions=[]
    with (root/'ui.log').open('w') as log:
        p=subprocess.Popen(['quickshell','--no-duplicate','--no-color','-p',str(config)],env=env,stdout=log,stderr=subprocess.STDOUT)
        try:
            deadline=time.monotonic()+6
            while time.monotonic()<deadline:
                if p.poll() is not None:raise RuntimeError('smoke-ui-exited')
                try:
                    state=json.loads(ipc('snapshot'))
                    if state['mapped']:break
                except (RuntimeError,ValueError):pass
                time.sleep(.1)
            else:raise RuntimeError('smoke-readiness-timeout')
            transitions.append(state)
            ipc('visible','false');transitions.append(json.loads(ipc('snapshot')))
            ipc('visible','true');transitions.append(json.loads(ipc('snapshot')))
        except Exception as e:error=str(e)
        finally:
            if p.poll() is None:
                try:ipc('quit');p.wait(timeout=3)
                except Exception:p.terminate();p.wait(timeout=3)
    after=host()
    journal=subprocess.run(['journalctl','--no-pager','-o','json','--since',since,'COREDUMP_EXE=/usr/lib/xdg-desktop-portal-hyprland'],capture_output=True,text=True,check=True)
    crashes=sum(1 for line in journal.stdout.splitlines() if line.startswith('{'))
    warnings=[line.replace(str(root),'<study>') for line in (root/'ui.log').read_text().splitlines() if 'WARN' in line or 'ERROR' in line]
    passed=not error and before==after and crashes==0 and [s['visible'] for s in transitions]==[True,False,True] and all(s['appId']=='qoda-player' for s in transitions)
    result={'scope':'existing real session bus; window registration/render/hide/show/exit only','pass':passed,'error':error,'transitions':transitions,'hostPortalProcessesUnchanged':before==after,'newHyprlandPortalCoredumps':crashes,'warnings':warnings,'limits':['No file chooser, screencast, document export or other portal feature requested','Real credentials and unrelated windows not captured']}
    (Path(__file__).resolve().parent/'evidence/host-portal-smoke.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)
    if not passed:raise RuntimeError('host-portal-smoke-failed')

if __name__=='__main__':main()
