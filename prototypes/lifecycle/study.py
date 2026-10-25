#!/usr/bin/env python3
"""Bounded lifecycle qualification. All integration names and files are disposable."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
import wave
from qoda import Controller, atomic
from portal_guard import check_bus, host

HERE=Path(__file__).resolve().parent
EVIDENCE=HERE/'evidence'
TARGET=Path(os.environ.get('QODA_BUILD_TARGET',str(HERE.parents[1]/'.scratch/lifecycle-target')))
BINARY=TARGET/'release/qoda-lifecycle-prototype'
BUS='org.mpris.MediaPlayer2.qoda'


def wait(fn,timeout=12):
    deadline=time.monotonic()+timeout;last=None
    while time.monotonic()<deadline:
        try:
            last=fn()
            if last:return last
        except (OSError,ValueError,RuntimeError,subprocess.CalledProcessError,KeyError):pass
        time.sleep(.06)
    raise TimeoutError(str(last))


def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()


class Study:
    def __init__(self):
        self.root=Path(tempfile.mkdtemp(prefix='qoda-life-'));self.root.chmod(0o700)
        self.tag='qoda-study-'+uuid.uuid4().hex[:10]
        self.results=[];self.units=[];self.bus=None;self.failures=[]
        self.manager=dict(os.environ)
        self.host_before=host()
        self.config=None

    def check(self,name,ok,detail=None):
        self.results.append({'name':name,'pass':bool(ok),'detail':detail})
        print(('PASS ' if ok else 'FAIL ')+name,flush=True)

    def redact(self,v):
        t=json.dumps(v).replace(str(self.root),'<study>').replace(str(HERE),'<source>').replace(str(Path.home()),'<home>').replace(os.environ['XDG_RUNTIME_DIR'],'<user-runtime>')
        return json.loads(t)

    def ctl(self,*args,check=True):
        return subprocess.run(['systemctl','--user',*args],capture_output=True,text=True,timeout=15,check=check)

    def qoda(self,action,check=True):
        return subprocess.run([sys.executable,str(HERE/'qoda.py'),'--root',str(self.root),action],capture_output=True,text=True,timeout=24,check=check)

    def controller(self):return Controller(self.root)

    def rpc(self,op,**kw):return self.controller().rpc(op,**kw)

    def state(self):return self.rpc('snapshot')['state']

    def ui(self):return json.loads(self.controller().ui('snapshot'))

    def props(self,kind):
        s=self.ctl('show',self.tag+'-'+kind+'.service','-p','MainPID','-p','NRestarts','-p','Result','-p','ActiveState','-p','SubState').stdout
        return dict(line.split('=',1) for line in s.splitlines() if '=' in line)

    def buscall(self,*args,check=True):
        return subprocess.run(['busctl','--address='+self.config['serviceEnv']['DBUS_SESSION_BUS_ADDRESS'],*args],capture_output=True,text=True,timeout=8,check=check)

    def own(self,path,content,executable=False):
        if path.exists():raise RuntimeError('refusing-existing-file:'+str(path))
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(content)
        if executable:path.chmod(0o755)
        self.manifest[str(path)]=digest(path)

    def release(self,version):
        target=self.root/'prefix/lib/qoda-player/releases'/version
        if target.exists():raise RuntimeError('release-exists')
        target.mkdir(parents=True)
        shutil.copy2(BINARY,target/'backend')
        self.manifest[str(target/'backend')]=digest(target/'backend')
        self.own(target/'shell.qml',(HERE/'shell.qml').read_text().replace('@RELEASE@',version))
        self.own(target/'release.json',json.dumps({'version':version,'schema':1,'protocol':1,'binarySha256':digest(target/'backend')}))
        return target

    def switch(self,version):
        current=self.root/'prefix/lib/qoda-player/current'
        temp=current.with_name('current.next');temp.unlink(missing_ok=True)
        temp.symlink_to('releases/'+version);temp.replace(current)

    def setup(self):
        root=self.root;self.manifest={}
        for d in ('runtime','data/qoda','state','cache','config'):(root/d).mkdir(parents=True,exist_ok=True)
        (root/'runtime').chmod(0o700)
        buspath=root/'runtime/bus'
        busconf=root/'runtime/bus.conf'
        busconf.write_text('<busconfig><type>session</type><listen>unix:path='+str(buspath)+'</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        self.bus=subprocess.Popen(['dbus-daemon','--nofork','--config-file='+str(busconf)],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        wait(buspath.exists)
        self.check('private bus cannot activate host desktop services',all(check_bus('unix:path='+str(buspath))['activationDenied'].values()))
        self.config={'unit':self.tag,'managerBus':os.environ.get('DBUS_SESSION_BUS_ADDRESS','unix:path='+os.environ['XDG_RUNTIME_DIR']+'/bus'),
            'serviceEnv':{k:os.environ[k] for k in ('XDG_RUNTIME_DIR','WAYLAND_DISPLAY','DISPLAY','XDG_SESSION_TYPE','XDG_CURRENT_DESKTOP') if k in os.environ}}
        self.config['serviceEnv'].update({
            'DBUS_SESSION_BUS_ADDRESS':'unix:path='+str(buspath),'QT_QUICK_CONTROLS_STYLE':'Basic','NO_AT_BRIDGE':'1','QT_ACCESSIBILITY':'0',
            'QODA_MANAGER_BUS':self.config['managerBus'],'QODA_STUDY_HOME':str(root),'QODA_STUDY_LAUNCHER':str(root/'prefix/lib/qoda-player/controller.py'),
            'QODA_PROTO_RUNTIME':str(root/'runtime'),'QODA_PROTO_ROWS':'100',
            'QODA_STUDY_DB':str(root/'data/qoda/session.sqlite3'),
            'XDG_CONFIG_HOME':str(root/'config'),'XDG_DATA_HOME':str(root/'data'),'XDG_STATE_HOME':str(root/'state'),'XDG_CACHE_HOME':str(root/'cache')})
        atomic(root/'study.json',self.config)
        with wave.open(str(root/'runtime/fixture.wav'),'wb') as w:
            w.setparams((2,2,48000,0,'NONE','not compressed'));w.writeframes(b'\0'*4*48000*120)
        self.own(root/'prefix/lib/qoda-player/controller.py',(HERE/'qoda.py').read_text())
        self.release('0.1.0-study');self.switch('0.1.0-study')
        launcher=root/'prefix/bin/qoda'
        self.own(launcher,'#!/bin/sh\nqoda_prefix=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)\nexec /usr/bin/python3 "$qoda_prefix/lib/qoda-player/controller.py" --root "$qoda_prefix/.." "$@"\n',True)
        self.own(root/'prefix/share/applications/qoda-player.desktop','[Desktop Entry]\nType=Application\nName=Qoda Player\nExec='+str(launcher)+' show\nIcon=qoda-player\nTerminal=false\nActions=Toggle;Compact;Quit;\n\n[Desktop Action Toggle]\nName=Toggle\nExec='+str(launcher)+' toggle\n\n[Desktop Action Compact]\nName=Compact\nExec='+str(launcher)+' compact\n\n[Desktop Action Quit]\nName=Quit\nExec='+str(launcher)+' quit\n')
        self.own(root/'prefix/share/icons/hicolor/scalable/apps/qoda-player.svg','<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" rx="12" fill="#18261d"/><path d="M24 16v32l26-16z" fill="#c8e6ad"/></svg>')
        unitdir=Path(os.environ['XDG_RUNTIME_DIR'])/'systemd/user'
        for kind in ('backend','ui'):
            name=self.tag+'-'+kind+'.service'
            content='[Unit]\nDescription=Disposable Qoda lifecycle '+kind+'\nPartOf='+self.tag+'-session.target\nAfter='+self.tag+'-session.target\n'
            if kind=='backend':content+='StartLimitIntervalSec=60\nStartLimitBurst=3\n'
            content+='[Service]\nType=exec\nExecStart=/usr/bin/python3 "'+str(root/'prefix/lib/qoda-player/controller.py')+'" --root "'+str(root)+'" exec-'+kind+'\nRestart='+('on-failure' if kind=='backend' else 'no')+'\nTimeoutStopSec=10\n'
            if kind=='backend':content+='RestartSec=2\nRestartPreventExitStatus=73 78\n'
            content+='StandardOutput=append:'+str(root/'state'/ (kind+'.log'))+'\nStandardError=append:'+str(root/'state'/(kind+'.log'))+'\n'
            # Keep generated config artifacts inside the prefix; use temporary runtime copies for the real manager.
            self.own(root/'config/systemd/user'/name,content)
            self.own(unitdir/name,content);self.units.append(unitdir/name)
        target=self.tag+'-session.target'
        self.own(unitdir/target,'[Unit]\nDescription=Disposable Qoda graphical-session lifetime fixture\n')
        self.units.append(unitdir/target)
        self.ctl('daemon-reload')
        self.save_manifest()

    def save_manifest(self):atomic(self.root/'manifest.json',self.manifest)

    def qualified_update(self,version,fail=False):
        c=self.controller()
        with c.lock():
            old=(self.root/'prefix/lib/qoda-player/current').resolve().name
            c.rpc('pause');c.rpc('checkpoint')
            self.ctl('stop',c.unit('ui'),c.unit('backend'))
            self.switch(version)
            if fail:
                raise RuntimeError('injected-activation-failure-old-release-retained:'+old)
        self.qoda('show')

    def run_checks(self):
        c=self.controller()
        self.check('status and absent hide do not start services',not json.loads(self.qoda('status').stdout)['backend'] and json.loads(self.qoda('hide').stdout)['absent'])
        validation=subprocess.run(['desktop-file-validate',str(self.root/'prefix/share/applications/qoda-player.desktop')],capture_output=True,text=True)
        self.check('desktop entry/actions and matching icon generated',validation.returncode==0 and (self.root/'prefix/share/icons/hicolor/scalable/apps/qoda-player.svg').exists(),validation.stderr)
        self.check('keyring missing on private bus is explicit',self.buscall('status','org.freedesktop.secrets',check=False).returncode!=0)
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            starts=list(pool.map(lambda _:self.qoda('show',check=False),range(8)))
        self.check('eight concurrent launches reuse one ready backend and UI',all(x.returncode==0 for x in starts) and self.ui()['connected'] and self.ui()['compatible'] and self.ui()['form']=='full',[x.stderr for x in starts if x.returncode])
        backend_pid=int(self.props('backend')['MainPID']);ui_pid=int(self.props('ui')['MainPID'])
        ui_state=self.ui()
        clients=json.loads(subprocess.check_output(['hyprctl','clients','-j']))
        own=[{k:client.get(k) for k in ('class','initialClass','pid')} for client in clients if client.get('pid')==ui_state['pid']]
        self.check('stock Quickshell AppId pragma reaches compositor product identity',ui_state['appId']=='qoda-player' and any(c['class']=='qoda-player' for c in own),{'shellId':ui_state['shellId'],'appId':ui_state['appId'],'ownClients':own})
        launched=subprocess.run([str(self.root/'prefix/bin/qoda'),'status'],capture_output=True,text=True,timeout=6)
        self.check('installed entrypoint runs from prefix without source launcher',launched.returncode==0 and json.loads(launched.stdout)['backend'])
        self.check('fresh launch is paused with version match',not self.state()['playing'] and self.ui()['release']==self.state()['release'])
        self.check('runtime owner-only and socket 0600',self.root.stat().st_mode&0o777==0o700 and (self.root/'runtime/backend.sock').stat().st_mode&0o777==0o600)
        # Exclusive runtime owner rejects a competing backend before touching its socket.
        env=dict(os.environ,**self.config['serviceEnv'],QODA_STUDY_RELEASE='0.1.0-study')
        duplicate=subprocess.run([str(BINARY)],env=env,capture_output=True,text=True,timeout=4)
        self.check('exclusive backend lock rejects a second owner',duplicate.returncode==73 and self.state()['instance'].startswith(str(backend_pid)+'-'))
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(2);s.connect(str(self.root/'runtime/backend.sock'));s.sendall(b'{"op":"hello","protocol":99}\n')
            response=json.loads(s.makefile('rb').readline())
        self.check('protocol mismatch rejected before session commands',response.get('error')=='protocol-mismatch')
        # Never export the private process environment. Compare only the expected fields.
        raw=(Path('/proc')/str(ui_pid)/'environ').read_bytes().split(b'\0')
        observed=dict(x.split(b'=',1) for x in raw if b'=' in x)
        self.check('graphical environment explicitly reaches service UI',observed.get(b'WAYLAND_DISPLAY')==os.environ['WAYLAND_DISPLAY'].encode() and observed.get(b'XDG_CONFIG_HOME')==str(self.root/'config').encode())
        self.rpc('append',trackId='fixture:track:1');self.rpc('play');wait(lambda:self.state()['playing'] and self.state()['position']>.1)
        before=self.state()['position'];self.qoda('hide');time.sleep(.3)
        self.check('hide preserves null playback and authority',self.ui()['form']=='hidden' and self.state()['position']>before)
        self.qoda('compact');self.qoda('hide');self.qoda('toggle')
        self.check('toggle restores last compact form',self.ui()['form']=='compact')
        # UI SIGKILL must not restart itself or affect the backend.
        instance=self.state()['instance'];before=self.state()['position'];os.kill(ui_pid,signal.SIGKILL)
        wait(lambda:self.props('ui')['ActiveState']=='failed');time.sleep(.3)
        self.check('UI crash leaves backend playing and does not auto-restart',self.state()['instance']==instance and self.state()['position']>before and int(self.props('ui')['MainPID'])==0)
        self.qoda('show');self.check('targeted launch recreates UI and resynchronizes',self.ui()['state']['instance']==instance and self.ui()['state']['queue']==self.state()['queue'])
        self.rpc('checkpoint');before=self.state()['position'];os.kill(backend_pid,signal.SIGKILL)
        recovered=wait(lambda:(s if s['instance']!=instance and s['loaded'] else None) if (s:=self.state()) else None)
        self.check('systemd backend crash restart restores queue and position paused',len(recovered['queue'])==1 and not recovered['playing'] and recovered['position']>=before-.5 and int(self.props('backend')['NRestarts'])==1,{'before':before,'after':recovered['position']})
        # MPRIS lives only on this study's private session bus.
        rootif='org.mpris.MediaPlayer2';playerif=rootif+'.Player';obj='/org/mpris/MediaPlayer2'
        identity=self.buscall('get-property',BUS,obj,rootif,'Identity').stdout
        desktop=self.buscall('get-property',BUS,obj,rootif,'DesktopEntry').stdout
        self.check('backend owns product MPRIS identity and desktop entry','Qoda Player' in identity and 'qoda-player' in desktop)
        self.buscall('call',BUS,obj,playerif,'Play');wait(lambda:self.state()['playing'])
        self.buscall('call',BUS,obj,playerif,'Pause');wait(lambda:not self.state()['playing'])
        self.check('MPRIS media commands share Rust session authority',not self.state()['playing'])
        self.rpc('mode-guard',enabled=True);wait(lambda:not self.state()['playing'])
        generation=self.state()['generation'];self.rpc('pause')
        stale=False
        try:self.rpc('completion',generation=generation)
        except RuntimeError as e:stale=str(e)=='stale-completion'
        self.check('Rust pause rejects stale completion generation',stale)
        denied=[]
        for value in (0,.5,2):denied.append(self.buscall('set-property',BUS,obj,playerif,'Volume','d',str(value),check=False).returncode!=0)
        unity=self.buscall('set-property',BUS,obj,playerif,'Volume','d','1',check=False)
        self.check('Rust MPRIS guard rejects non-unity volume in fidelity fixture',all(denied) and unity.returncode==0 and self.state()['volume']==1)
        self.rpc('mode-guard',enabled=False)
        self.buscall('set-property',BUS,obj,playerif,'Volume','d','.5');wait(lambda:self.state()['volume']==.5)
        self.check('Desktop-mode fixture volume reaches libmpv',self.state()['volume']==.5)
        self.qoda('hide');self.buscall('call',BUS,obj,rootif,'Raise');wait(lambda:self.ui()['form']=='full')
        self.check('MPRIS Raise targets canonical launcher',self.ui()['form']=='full')
        # Hold a real SQLite writer lock, observe error, then force bounded shutdown.
        db=sqlite3.connect(self.root/'data/qoda/session.sqlite3');db.execute('BEGIN IMMEDIATE')
        failed=False
        try:self.rpc('checkpoint')
        except RuntimeError:failed=True
        status=json.loads(self.qoda('status').stdout)
        self.check('checkpoint error is exposed in live status',failed and status['state']['persistenceError'] is not None)
        t=time.monotonic();q=self.qoda('quit',check=False);elapsed=time.monotonic()-t
        self.check('Quit stops both services and reports persistence failure',q.returncode!=0 and not c.active('backend') and not c.active('ui') and elapsed<10 and json.loads(self.qoda('status').stdout)['lifecycleError'] is not None,{'seconds':elapsed,'error':q.stderr})
        db.rollback();db.close();time.sleep(2.3)
        self.check('intentional Quit has no automatic resurrection',not c.active('backend') and not c.active('ui'))
        self.qoda('show');self.check('fresh invocation after Quit starts retained session paused',len(self.state()['queue'])==1 and not self.state()['playing'])
        self.qoda('stop-clear');wait(lambda:not self.state()['playing'])
        self.check('Stop and clear differs from Quit',not self.state()['queue'] and c.active('backend'))
        self.rpc('append',trackId='fixture:track:2');self.rpc('play');wait(lambda:self.state()['playing'])
        self.release('0.2.0-study');self.save_manifest()
        self.check('stage new release leaves current playback running',self.state()['playing'] and self.state()['release']=='0.1.0-study')
        self.qualified_update('0.2.0-study')
        self.check('atomic staged update resumes only paused with matched UI/backend',self.state()['release']==self.ui()['release']=='0.2.0-study' and not self.state()['playing'] and len(self.state()['queue'])==1)
        self.qualified_update('0.1.0-study')
        self.check('compatible-schema code rollback retains checkpoint',self.ui()['release']==self.state()['release']=='0.1.0-study' and len(self.state()['queue'])==1)
        try:self.qualified_update('0.2.0-study',fail=True)
        except RuntimeError:pass
        self.switch('0.1.0-study');self.qoda('show')
        self.check('failed activation retains recoverable old release',self.state()['release']=='0.1.0-study' and len(self.state()['queue'])==1)
        self.buscall('call',BUS,obj,rootif,'Quit');wait(lambda:not c.active('backend') and not c.active('ui'))
        self.check('MPRIS Quit routes shutdown outside backend cgroup',not c.active('backend'))
        # Login/logout uses only a disposable target, never the real desktop session.
        self.qoda('login-start');self.check('optional login fixture starts backend hidden and paused',c.active('backend') and not c.active('ui') and not self.state()['playing'])
        self.qoda('show');self.rpc('play');wait(lambda:self.state()['playing']);self.rpc('checkpoint')
        self.ctl('stop',self.tag+'-session.target');wait(lambda:not c.active('backend') and not c.active('ui'))
        self.check('disposable graphical lifetime stops both units',not c.active('backend') and not c.active('ui'))
        # Newer schemas must not be silently opened by older code.
        db=sqlite3.connect(self.root/'data/qoda/session.sqlite3');db.execute('PRAGMA user_version=2');db.commit();db.close()
        q=self.qoda('show',check=False)
        props=self.props('backend');time.sleep(2.2)
        self.check('incompatible schema fails without restart loop',q.returncode!=0 and props['Result']=='exit-code' and self.props('backend')['NRestarts']=='0')
        db=sqlite3.connect(self.root/'data/qoda/session.sqlite3');db.execute('PRAGMA user_version=1');db.commit();db.close()
        self.ctl('reset-failed',c.unit('backend'));self.qoda('show')
        # Exhaust the actual 3/60 start limit with crashes, then require explicit reset/retry.
        self.ctl('stop',c.unit('backend'));self.ctl('reset-failed',c.unit('backend'));self.ctl('start',c.unit('backend'));wait(lambda:self.state()['loaded'])
        crashes=0
        for _ in range(3):
            pid=int(self.props('backend')['MainPID']);os.kill(pid,signal.SIGKILL);crashes+=1
            time.sleep(2.25)
            if self.props('backend')['Result']=='start-limit-hit':break
            wait(lambda:self.state()['loaded'])
        self.check('backend failures hit actual three-starts-per-minute limit',self.props('backend')['Result']=='start-limit-hit',{'crashes':crashes,'properties':self.props('backend')})
        self.ctl('reset-failed',c.unit('backend'));self.qoda('show')
        self.check('explicit rate-limit reset recovers paused',not self.state()['playing'])
        # Quit during a deliberately slow startup: visibility requests cannot resurrect it.
        self.qoda('quit');self.ctl('reset-failed',c.unit('backend'))
        self.config['serviceEnv']['QODA_STUDY_DELAY']='1500';atomic(self.root/'study.json',self.config)
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            launch=pool.submit(self.qoda,'show',False);time.sleep(.15)
            quitjob=pool.submit(self.qoda,'quit',False);time.sleep(.05)
            racing=pool.submit(self.qoda,'compact',False)
            outcomes=[launch.result(),quitjob.result(),racing.result()]
        time.sleep(2.3)
        self.check('Quit during startup supersedes concurrent activation',outcomes[1].returncode==0 and not c.active('backend') and not c.active('ui'),[{'returncode':x.returncode,'stderr':x.stderr} for x in outcomes])
        self.config['serviceEnv'].pop('QODA_STUDY_DELAY');atomic(self.root/'study.json',self.config)
        self.save_manifest()

    def cleanup(self):
        if self.config:
            self.qoda('quit',check=False)
            self.ctl('stop',self.tag+'-session.target',check=False)
        # Preserve modified/unowned files; remove only exact manifest bytes within known roots.
        retained=[]
        for filename,sha in getattr(self,'manifest',{}).items():
            p=Path(filename)
            allowed=p.is_relative_to(self.root) or p in self.units
            if p.exists() and allowed and p.is_file() and digest(p)==sha:p.unlink()
            elif p.exists():retained.append(str(p))
        for p in self.units:
            if p.exists():retained.append(str(p))
        if self.units:self.ctl('daemon-reload');self.ctl('reset-failed',*[p.name for p in self.units],check=False)
        if self.bus:
            self.bus.terminate();self.bus.wait(timeout=5)
        self.check('host portal services unchanged after study',host()==self.host_before)
        return retained


def main():
    EVIDENCE.mkdir(exist_ok=True)
    env=dict(os.environ,CARGO_TARGET_DIR=str(TARGET))
    subprocess.run(['cargo','build','--release','--locked','--manifest-path',str(HERE/'Cargo.toml')],env=env,check=True)
    study=Study();error=None;retained=[]
    print('Study root: '+str(study.root),flush=True)
    try:
        study.setup();study.run_checks()
    except KeyboardInterrupt:
        error={'type':'Interrupted','message':'Study interrupted; acceptance incomplete'}
    except Exception as e:
        error={'type':type(e).__name__,'message':str(e)}
        print('ERROR '+str(error),flush=True)
    finally:
        # Keep synthetic local logs for diagnosis; publish redacted summaries only.
        try:retained=study.cleanup()
        except Exception as e:retained=['cleanup-error:'+str(e)]
        result=study.redact({'checks':study.results,'error':error,'retainedOwnedFiles':retained,'privateRun':str(study.root),'binarySha256':digest(BINARY)})
        atomic(EVIDENCE/('run-'+str(time.time_ns())+'.json'),result)
        atomic(EVIDENCE/'results.json',result)
        print(json.dumps({'passed':sum(x['pass'] for x in study.results),'total':len(study.results),'error':error,'retained':retained}),flush=True)
    return 1 if error or any(not r['pass'] for r in study.results) or retained else 0

if __name__=='__main__':sys.exit(main())
