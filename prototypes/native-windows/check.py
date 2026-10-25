#!/usr/bin/env python3
"""THROWAWAY native-surface checks through the prototype's application IPC.
No key/mouse injection, compositor configuration changes, or audio.
Start shell.qml first; run this script from any directory. It exits that test instance.
"""
import json
import os
from pathlib import Path
import subprocess
import time

HERE=Path(__file__).resolve().parent
CONFIG=str(HERE/'shell.qml')
REPORT=HERE/'evidence'
REPORT.mkdir(exist_ok=True)
observations=[]
checks=[]

def ipc(name,*args,required=True):
    argv=['quickshell','ipc','-p',CONFIG,'call','probe',name,*[str(a).lower() if isinstance(a,bool) else str(a) for a in args]]
    p=subprocess.run(argv,text=True,capture_output=True,timeout=8)
    if required and (p.returncode or 'Not ready' in p.stdout):
        raise RuntimeError(name+': '+p.stdout+p.stderr)
    return p

def state():
    return json.loads(ipc('snapshot').stdout)

def compositor():
    clients=json.loads(subprocess.check_output(['hyprctl','clients','-j'],text=True))
    monitors=json.loads(subprocess.check_output(['hyprctl','monitors','-j'],text=True))
    layers=json.loads(subprocess.check_output(['hyprctl','layers','-j'],text=True))
    active=json.loads(subprocess.check_output(['hyprctl','activewindow','-j'],text=True))
    ours=[{'workspace':w['workspace']['id'],'floating':w['floating'],'mapped':w['mapped'],
           'size':w['size'],'at':w['at'],'monitor':w['monitor'],'fullscreen':w['fullscreen']}
          for w in clients if w['title'].startswith('Qoda Native Prototype')]
    panels={m:[{k:v for k,v in layer.items() if k not in ['address','pid']} for group in data['levels'].values() for layer in group if layer.get('namespace')=='qoda-native-prototype'] for m,data in layers.items()}
    return {'clients':ours,'panels':panels,'prototype_toplevel_active':active.get('title','').startswith('Qoda Native Prototype'),
            'monitors':[{'name':m['name'],'width':m['width'],'height':m['height'],'scale':m['scale'],'reserved':m['reserved'],'workspace':m['activeWorkspace']['id']} for m in monitors]},active.get('address')

def snap(label,settle=.25):
    time.sleep(settle)
    c,active=compositor()
    s=state()
    observations.append({'label':label,'state':s,'compositor':c})
    print(label+': '+s['form']+' full='+str(s['full']['width'])+' compact-focus='+str(s['compact']['keyboardMode']),flush=True)
    return s,c,active

def check(name,result,detail=''):
    checks.append({'name':name,'passed':bool(result),'detail':detail})
    print(('PASS ' if result else 'NOT ESTABLISHED ')+name,flush=True)

ipc('hide')
initial,baseline,_=snap('initial-hidden')
reserved={m['name']:m['reserved'] for m in baseline['monitors']}
ipc('showFull')
s,c,_=snap('full-tiled',.5)
check('normal tiled workspace',len(c['clients'])==1 and not c['clients'][0]['floating'] and c['clients'][0]['workspace']>0)
check('full native map and 200-percent scale',s['full']['mapped'] and s['full']['dpr']==2)
check('full compositor focus',c['prototype_toplevel_active'])
check('narrow queue is not persistent',not s['full']['persistentQueue'])

ipc('seed','Artists','native study',35)
seed,_,_=snap('navigation-seeded')
check('scroll position established',seed['listY']>1000)
focus=[]
for _ in range(4):
    focus.append(ipc('focusNext').stdout.strip())
s,_,_=snap('programmatic-focus-chain')
check('programmatic focus-chain operation',len(set(focus))>1,json.dumps(focus))

ipc('maximize',True)
s,c,_=snap('full-maximized',.65)
if s['full']['width']<1050:
    observations[-1]['note']='Client maximize did not widen the window; installed Omarchy defaults suppress maximize. Test wide layout via a temporary prototype-only fullscreen request.'
    ipc('wideProbe',True)
    s,c,_=snap('full-width-probe',.65)
check('wide persistent queue',s['full']['width']>=1050 and s['full']['persistentQueue'])
ipc('wideProbe',False)
ipc('maximize',False)
snap('full-unmaximized',.5)
ipc('openQueue')
s,_,_=snap('queue-open')
check('queue drawer opened',s['queueDrawer'])
ipc('dismiss')
s,_,_=snap('queue-dismissed')
check('queue dismiss keeps full form',not s['queueDrawer'] and s['form']=='full')
ipc('openDialog')
s,_,_=snap('full-dialog-open')
check('full dialog opened',s['dialog'])
ipc('dismiss')
s,_,_=snap('full-dialog-dismissed')
check('dialog dismiss keeps full form',not s['dialog'] and s['form']=='full')
ipc('dismiss')
s,_,_=snap('full-dismiss-no-overlay')
check('full Escape handler leaves browser open',s['form']=='full')

before=s['ticks']
ipc('hide')
s,c,_=snap('hidden-ticker',1.4)
check('hidden maps neither form',not s['full']['mapped'] and not s['compact']['mapped'] and not c['clients'])
check('fictional root ticker survives hiding',s['playing'] and s['ticks']>before)
ipc('toggle')
s,_,_=snap('restore-full',.5)
check('restores full and navigation',s['form']=='full' and s['page']==seed['page'] and s['query']==seed['query'] and s['selected']==seed['selected'] and abs(s['listY']-seed['listY'])<1)
ipc('closeFull')
s,c,_=snap('native-close',.4)
check('native close maps to hide',s['form']=='hidden' and s['closeEvents']>0 and s['playing'])
ipc('toggle')
s,_,_=snap('restore-after-close',.5)
check('restore after actual close',s['form']=='full' and s['full']['mapped'] and s['query']==seed['query'])

ipc('hide')
_,_,prior=snap('before-passive')
ipc('showCompact',False)
s,c,after=snap('compact-passive')
check('compact passive no keyboard request',s['compact']['mapped'] and s['compact']['keyboardMode']==0)
check('passive panel preserves previous toplevel',prior==after)
check('panel reserves no desktop space',{m['name']:m['reserved'] for m in c['monitors']}==reserved)
panels=[p for group in c['panels'].values() for p in group]
check('compact is native overlay surface',len(panels)==1)
check('compact selects invoking monitor',s['compact']['screen']==baseline['monitors'][0]['name'])
ipc('hide')
_,_,prior=snap('before-interactive')
ipc('showCompact',True)
s,c,_=snap('compact-interactive',.4)
check('compact settles to on-demand focus',s['compact']['keyboardMode']==2)
check('compact native active focus',s['compact']['active'])
ipc('openDialog')
s,_,_=snap('compact-dialog-open')
check('compact dialog opened',s['dialog'])
ipc('dismiss')
s,_,_=snap('compact-dialog-dismissed')
check('first dismiss leaves compact',not s['dialog'] and s['form']=='compact')
ipc('dismiss')
s,c,after=snap('compact-dismissed',.4)
check('second dismiss hides and releases keyboard',s['form']=='hidden' and s['compact']['keyboardMode']==0 and not s['compact']['mapped'])
check('compositor restores previous toplevel',prior==after)
ipc('toggle')
s,_,_=snap('restore-compact',.3)
check('toggle restores compact',s['form']=='compact')
for _ in range(3):
    ipc('toggle'); time.sleep(.03); ipc('toggle');time.sleep(.03)
ipc('hide')
s,c,_=snap('rapid-toggle-finished',.4)
check('rapid toggle leaves no keyboard grab',s['compact']['keyboardMode']==0 and not any(c['panels'].values()))
check('all visibility transitions retain selection',s['selected']==seed['selected'] and s['page']==seed['page'] and s['query']==seed['query'])

ipc('showFull')
snap('before-quit',.4)
ipc('quit',required=False)
time.sleep(.3)
probe=ipc('snapshot',required=False)
c,_=compositor()
check('explicit quit ends prototype instance',probe.returncode!=0 and not c['clients'] and not any(c['panels'].values()))

versions={'quickshell':subprocess.check_output(['quickshell','--version'],text=True).strip(),
          'hyprland':subprocess.check_output(['hyprctl','version'],text=True).splitlines()[0],
          'qt':subprocess.check_output(['pacman','-Q','qt6-base','qt6-declarative','qt6-wayland'],text=True).splitlines()}
report={'scope':'Native Quickshell windows through application IPC and read-only Hyprland diagnostics; no physical key or mouse injection.',
        'versions':versions,'checks':checks,'observations':observations,
        'unverified':['Physical Tab/Shift+Tab/Enter/Space/Ctrl+K/Escape dispatch and actual pointer interaction: no UI automation surface is available.',
                      'Multiple physical outputs, hotplug and scale changes: only one monitor at scale 2 is connected.',
                      'Real playback/backend survival, large catalog performance and production activation/shortcut binding are separate work.']}
(REPORT/'results.json').write_text(json.dumps(report,indent=2))
lines=['# Native surface observations','',report['scope'],'','| Check | Outcome |','| --- | --- |']
lines += ['| '+c['name']+' | '+('Observed' if c['passed'] else 'Not established')+' |' for c in checks]
lines += ['','## Unverified','']+['- '+s for s in report['unverified']]
(REPORT/'summary.md').write_text('\n'.join(lines)+'\n')
print('Evidence: '+str(REPORT),flush=True)
