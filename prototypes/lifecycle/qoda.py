#!/usr/bin/env python3
"""THROWAWAY isolated lifecycle controller. Requires an explicit study root."""
import argparse
import contextlib
import fcntl
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp=path.with_name(path.name+'.tmp-'+str(os.getpid()))
    tmp.write_text(json.dumps(value)+'\n');tmp.replace(path)


class Controller:
    def __init__(self, root):
        self.root=Path(root).resolve()
        self.cfg=json.loads((self.root/'study.json').read_text())
        self.runtime=self.root/'runtime'
        self.runtime.mkdir(mode=0o700,exist_ok=True)
        self.env=dict(os.environ, DBUS_SESSION_BUS_ADDRESS=self.cfg['managerBus'])
        self.start=time.time_ns()

    def systemctl(self, *args, check=True):
        return subprocess.run(['systemctl','--user',*args],env=self.env,capture_output=True,text=True,timeout=12,check=check)

    @contextlib.contextmanager
    def lock(self, name='activation'):
        with (self.runtime/(name+'.lock')).open('a') as f:
            deadline=time.monotonic()+12
            while True:
                try:
                    fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);break
                except BlockingIOError:
                    if time.monotonic()>deadline:raise RuntimeError('lifecycle-lock-timeout')
                    time.sleep(.02)
            yield

    def unit(self, kind):return self.cfg['unit']+'-'+kind+'.service'

    def active(self,kind):return self.systemctl('is-active',self.unit(kind),check=False).stdout.strip()=='active'

    def rpc(self, op, **kw):
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(2);s.connect(str(self.runtime/'backend.sock'))
            f=s.makefile('rb')
            s.sendall(b'{"op":"hello","protocol":1,"id":0}\n')
            boot=json.loads(f.readline())
            if boot.get('type')!='snapshot':raise RuntimeError('protocol-mismatch')
            s.sendall((json.dumps(dict(op=op,id=1,**kw))+'\n').encode())
            while True:
                b=f.readline()
                if not b:raise RuntimeError('backend-disconnected')
                v=json.loads(b)
                if v.get('id')==1 and v.get('type')=='ack':
                    result=v.get('result',v)
                    if result.get('error'):raise RuntimeError(result['error'])
                    return result

    def ui(self,method,*args):
        release=json.loads((self.runtime/'ui-release.json').read_text())
        return subprocess.run(['quickshell','ipc','-p',release['qml'],'call','--','lifecycle',method,*map(str,args)],capture_output=True,text=True,timeout=3,check=True).stdout.strip()

    def intent(self, action):
        with self.lock('intent'):
            p=self.runtime/'intent.json'
            previous=json.loads(p.read_text()) if p.exists() else {'time':0}
            if self.start>previous['time']:atomic(p,{'time':self.start,'action':action})

    def check_shutdown(self):
        if (self.runtime/'quitting').exists():raise RuntimeError('shutdown-in-progress')
        p=self.runtime/'last-quit.json'
        if p.exists() and self.start<=json.loads(p.read_text())['time']:raise RuntimeError('activation-superseded-by-quit')

    def wait_ready(self, ui=False):
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            self.check_shutdown()
            try:
                state=json.loads(self.ui('snapshot')) if ui else self.rpc('snapshot')['state']
                if state.get('connected') and state.get('compatible') if ui else state.get('loaded'):
                    return state
            except (OSError,RuntimeError,ValueError,subprocess.CalledProcessError):pass
            result=self.systemctl('show',self.unit('ui' if ui else 'backend'),'-p','Result','-p','ActiveState',check=False).stdout
            if 'ActiveState=failed' in result:raise RuntimeError('service-failed:'+result.replace('\n',' '))
            time.sleep(.05)
        raise RuntimeError('readiness-timeout')

    def status(self):
        state=None
        if self.active('backend'):
            try:state=self.rpc('snapshot')['state']
            except (OSError,ValueError,RuntimeError):pass
        errorfile=self.root/'state'/'lifecycle-error.json'
        return {'backend':self.active('backend'),'ui':self.active('ui'),'current':(self.root/'prefix/lib/qoda-player/current').resolve().name,'state':state,'lifecycleError':json.loads(errorfile.read_text()) if errorfile.exists() else None}

    def shutdown(self):
        (self.runtime/'quitting').touch()
        error=None
        try:
            with self.lock():
                if self.active('backend'):
                    try:self.rpc('pause');self.rpc('checkpoint')
                    except (OSError,RuntimeError,ValueError) as e:error=str(e)
                self.systemctl('stop',self.unit('ui'),self.unit('backend'))
                atomic(self.runtime/'last-quit.json',{'time':time.time_ns()})
                if error:atomic(self.root/'state/lifecycle-error.json',{'error':error,'action':'quit','lastCheckpointRetained':True})
        finally:
            (self.runtime/'quitting').unlink(missing_ok=True)
        if error:raise RuntimeError('stopped-with-persistence-error:'+error)
        return {'stopped':True}

    def run(self, action):
        if action=='status':return self.status()
        if action=='quit':return self.shutdown()
        if action in ('exec-backend','exec-ui'):
            release=(self.root/'prefix/lib/qoda-player/current').resolve()
            env=dict(os.environ,**self.cfg['serviceEnv'])
            env['QODA_STUDY_RELEASE']=release.name
            if action=='exec-backend':os.execve(str(release/'backend'),[str(release/'backend')],env)
            atomic(self.runtime/'ui-release.json',{'qml':str(release/'shell.qml'),'release':release.name})
            os.execve('/usr/bin/quickshell',['quickshell','--no-duplicate','--no-color','-p',str(release/'shell.qml')],env)
        if action in ('show','toggle','compact','hide','login-start'):self.intent(action)
        with self.lock():
            self.check_shutdown()
            if action=='stop-clear':return self.rpc('stop-clear')
            if action=='hide' and not self.active('backend'):return {'absent':True}
            self.systemctl('start',self.cfg['unit']+'-session.target')
            if not self.active('backend'):
                self.systemctl('start',self.unit('backend'))
            self.wait_ready()
            if action=='login-start':return {'hidden':True,'paused':not self.rpc('snapshot')['state']['playing']}
            if not self.active('ui'):
                self.systemctl('start',self.unit('ui'))
            self.wait_ready(ui=True)
            intent=json.loads((self.runtime/'intent.json').read_text())
            self.check_shutdown()
            if intent['time']!=self.start:return {'coalesced':True}
            reply=json.loads(self.ui('present',intent['action']))
            if not reply['compatible']:raise RuntimeError('ui-backend-version-mismatch')
            return {'applied':intent['action']}


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',required=True);p.add_argument('action',choices=['show','toggle','compact','hide','status','quit','stop-clear','login-start','exec-backend','exec-ui'])
    args=p.parse_args()
    try:
        print(json.dumps(Controller(args.root).run(args.action)))
    except Exception as e:
        print(json.dumps({'error':str(e)}),file=sys.stderr);sys.exit(1)

if __name__=='__main__':main()
