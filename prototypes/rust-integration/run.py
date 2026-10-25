#!/usr/bin/env python3
"""THROWAWAY experiment runner, not a production test suite. --check records evidence."""
import argparse,sys,hashlib,http.server,json,os,pathlib,shutil,socket,stat,struct,subprocess,tempfile,threading,time,wave,zlib
HERE=pathlib.Path(__file__).resolve().parent
ROOT=HERE.parent.parent
BUS='org.mpris.MediaPlayer2.qoda_integration_prototype'
class Peer:
    def __init__(self,path,protocol=1):
        self.s=socket.socket(socket.AF_UNIX);self.s.settimeout(3);self.s.connect(str(path));self.f=self.s.makefile('rb');self.seq=0
        self.send({'op':'hello','protocol':protocol});self.boot=self.read()
    def send(self,o):
        self.seq+=1;o.setdefault('id',self.seq);self.s.sendall((json.dumps(o)+'\n').encode());return o['id']
    def read(self):return json.loads(self.f.readline())
    def until(self,pred):
        end=time.monotonic()+4
        while time.monotonic()<end:
            v=self.read()
            if pred(v):return v
        raise TimeoutError('response')
    def command(self,op,**kw):
        i=self.send(dict(op=op,**kw));return self.until(lambda v:v.get('type')=='ack' and v.get('id')==i)
    def snapshot(self):return self.command('snapshot')['result']['state']
    def close(self):self.f.close();self.s.close()
def png(rgb):
    def chunk(t,d):return struct.pack('!I',len(d))+t+d+struct.pack('!I',zlib.crc32(t+d)&0xffffffff)
    return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('!2I5B',32,32,8,2,0,0,0))+chunk(b'IDAT',zlib.compress((b'\0'+bytes(rgb)*32)*32))+chunk(b'IEND',b'')
class Artwork(http.server.BaseHTTPRequestHandler):
    requests=[]
    def do_GET(self):
        authorized=self.headers.get('Authorization')=='Bearer fictional-prototype-only'
        self.requests.append({'path':self.path,'authorized':authorized})
        data=png((100,180,90) if self.path.endswith('1') else (70,100,210))
        self.send_response(200 if authorized else 401);self.end_headers();self.wfile.write(data if authorized else b'')
    def log_message(self,*a):pass
def wait_for(fn,timeout=8):
    end=time.monotonic()+timeout;last=None
    while time.monotonic()<end:
        try:
            last=fn()
            if last:return last
        except (OSError,ValueError,subprocess.CalledProcessError):pass
        time.sleep(.04)
    raise TimeoutError(str(last))
def ipc(method,*args):
    return subprocess.check_output(['quickshell','ipc','-p',str(HERE/'shell.qml'),'call','integration',method,*map(str,args)],text=True,stderr=subprocess.DEVNULL).strip()
def qs():return json.loads(ipc('snapshot'))
def rss(pid):
    text=pathlib.Path(f'/proc/{pid}/status').read_text();return int(next(l.split()[1] for l in text.splitlines() if l.startswith('VmRSS:')))*1024
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true');args=parser.parse_args()
    subprocess.run(['cargo','build','--release','--locked','--manifest-path',str(HERE/'Cargo.toml')],check=True)
    run=pathlib.Path(tempfile.mkdtemp(prefix='qoda-rust-prototype-'));os.chmod(run,0o700)
    with wave.open(str(run/'fixture.wav'),'wb') as w:
        w.setparams((2,2,48000,0,'NONE','not compressed'));w.writeframes(b'\0'*4*48000*120)
    art_server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Artwork);threading.Thread(target=art_server.serve_forever,daemon=True).start()
    env=dict(os.environ,QODA_PROTO_RUNTIME=str(run),QODA_PROTO_ART_PORT=str(art_server.server_port),QT_QUICK_CONTROLS_STYLE='Basic')
    log=open(run/'backend.log','w+');qml_log=open(run/'qml.log','w+')
    backend=None;ui=None;peer=None
    results=[];metrics={};samples=[]
    def check(name,ok,detail=None):
        results.append({'name':name,'pass':bool(ok),'detail':detail});print(('PASS ' if ok else 'FAIL ')+name,flush=True)
    def start_backend():
        p=subprocess.Popen([str(HERE/'target/release/qoda-integration-prototype')],env=env,stdout=log,stderr=log)
        wait_for(lambda:(run/'backend.sock').exists() and '"ready":true' in (run/'backend.log').read_text());return p
    try:
        backend=start_backend();peer=Peer(run/'backend.sock');wait_for(lambda:peer.snapshot()['loaded'])
        ui=subprocess.Popen(['quickshell','-n','-p',str(HERE/'shell.qml')],env=env,stdout=qml_log,stderr=qml_log)
        wait_for(lambda:qs()['connected'] and qs()['state']['loaded'])
        if not args.check:
            print('Prototype running: fictional catalog, null audio. Ctrl+C stops its own processes. '+str(run),flush=True)
            ipc('search','',0,0)
            while True:time.sleep(1)
        check('private runtime and socket permissions',stat.S_IMODE(run.stat().st_mode)==0o700 and stat.S_IMODE((run/'backend.sock').stat().st_mode)==0o600)
        wrong=Peer(run/'backend.sock',99);check('incompatible protocol rejected',wrong.boot.get('error')=='protocol-mismatch');wrong.close()
        # Fragment one frame and coalesce two: stream boundaries cannot define messages.
        p=Peer(run/'backend.sock');data=b'{"op":"snapshot","id":77}\n{"op":"snapshot","id":78}\n'
        p.s.sendall(data[:9]);time.sleep(.02);p.s.sendall(data[9:]);a=p.until(lambda v:v.get('id')==77);b=p.until(lambda v:v.get('id')==78)
        check('fragmented and coalesced NDJSON frames',a['type']==b['type']=='ack');p.close()
        boot=peer.boot['state'];check('atomic snapshot subscription bootstrap',boot['instance']==qs()['instance'] and boot['revision']<=qs()['revision'])
        # A fresh subscriber and a concurrent command producer race through the actor.
        observer=Peer(run/'backend.sock');initial=observer.boot['state'];producer=Peer(run/'backend.sock')
        def burst():
            for i in range(12):producer.command('append',trackId=f'fixture:race:{i}')
        worker=threading.Thread(target=burst);worker.start();revisions=[];last=initial
        while len(last['queue'])<len(initial['queue'])+12:
            event=observer.read()
            if event.get('type')=='update':last=event['state'];revisions.append(last['revision'])
        worker.join();producer.close();observer.close()
        check('snapshot and racing updates have no revision gap',revisions==list(range(initial['revision']+1,last['revision']+1)) and len(last['queue'])==len(initial['queue'])+12)
        ipc('command','append');pending=qs();wait_for(lambda:len(qs()['state']['queue'])==13 and qs()['pending']==0)
        check('pending command then acknowledged authoritative update',any(x['kind']=='pending' for x in qs()['journal']) and any(x['kind']=='ack' for x in qs()['journal']))
        before_lost=len(qs()['state']['queue']);ipc('lostReply');wait_for(lambda:len(qs()['state']['queue'])==before_lost+1 and qs()['pending']==1)
        ipc('disconnect','true');wait_for(lambda:not qs()['connected']);ipc('disconnect','false');wait_for(lambda:qs()['connected'] and qs()['pending']==0)
        check('QML lost acknowledgement becomes uncertain without replay',qs()['uncertain']==1 and len(qs()['state']['queue'])==before_lost+1)
        before=len(peer.snapshot()['queue']);t=time.perf_counter();r1=peer.command('append',trackId='fixture:track:7',operationId='lost-1',instance=boot['instance']);metrics['command_ms']=(time.perf_counter()-t)*1000
        again=peer.command('append',trackId='fixture:track:7',operationId='lost-1',instance=boot['instance']);check('same-instance operation token prevents duplicate',len(peer.snapshot()['queue'])==before+1 and again['result']==r1['result'])
        rev=peer.snapshot()['revision'];a=peer.command('append',trackId='fixture:track:8',expectedRevision=rev);b=peer.command('append',trackId='fixture:track:9',expectedRevision=rev)
        check('concurrent stale revision rejected',a['result'].get('accepted') and b['result'].get('error')=='revision-conflict')
        fresh=Peer(run/'backend.sock');base=len(fresh.boot['state']['queue']);fresh.send({'op':'append','trackId':'fixture:track:10','drop_reply':True,'operationId':'lost-real'})
        wait_for(lambda:len(peer.snapshot()['queue'])==base+1);fresh.close();reconnected=Peer(run/'backend.sock');check('lost reply resynchronized without replay',len(reconnected.boot['state']['queue'])==base+1);reconnected.close()
        ipc('resetTiming');time.sleep(.15)
        page_times=[]
        for query,offset in [('',0),('Track 099',0),('',99000),('missing',0),('',0)]:
            ipc('search',query,0,offset);v=wait_for(lambda:(s if s['pending']==0 else None) if (s:=qs()) else None);page_times.append(v['pageMs']);samples.append(rss(backend.pid)+rss(ui.pid))
        metrics['page_ms']=page_times
        check('bounded catalog page from 100000 rows',qs()['rows']==100 and qs()['total']==100000 and qs()['firstTrack']=='fixture:track:0')
        cap=Peer(run/'backend.sock');cap.send({'op':'search','query':'','limit':100000,'id':901});page=cap.until(lambda v:v.get('id')==901)
        check('oversized page request capped at 200 rows',len(page['rows'])==200 and page['total']==100000);cap.close()
        old=ipc('search','Track 000',300,0);new=ipc('search','Track 099',0,0)
        wait_for(lambda:qs()['pending']==0 and qs()['stalePages']>=1)
        check('cancelled old search cannot overwrite new results',qs()['firstTrack']=='fixture:track:99000' and qs()['cancelledPages']>=1)
        # Also prove cancellation interrupts a scan rather than publishing its page.
        p=Peer(run/'backend.sock');p.send({'op':'search','query':'Track','delay_ms':200,'id':501});p.send({'op':'search','query':'missing','id':502});cancel=p.until(lambda v:v.get('id')==501)
        check('superseded search work notices cancellation',cancel['type']=='search-cancelled' and cancel['scanned']<100000,cancel.get('scanned'));p.close()
        ipc('artwork',1);wait_for(lambda:qs()['artVersion']==1 and qs()['artStatus']==1);hash1=hashlib.sha256((run/'art-1.png').read_bytes()).hexdigest()
        ipc('artwork',2);wait_for(lambda:qs()['artVersion']==2 and qs()['artStatus']==1);hash2=hashlib.sha256((run/'art-2.png').read_bytes()).hexdigest()
        check('backend authenticated artwork fetch and QML decode',len(Artwork.requests)==2 and all(x['authorized'] for x in Artwork.requests) and qs()['artWidth']>0)
        check('versioned local artwork URL invalidates image cache',hash1!=hash2 and qs()['artworkUrl'].endswith('art-2.png'))
        check('presentation state contains no provider credentials',all(x not in json.dumps(qs()) for x in ['fictional-prototype-only','Authorization','127.0.0.1']))
        peer.command('play');wait_for(lambda:qs()['state']['playing'] and qs()['state']['position']>.1)
        pos=qs()['state']['position'];ipc('visible','false');time.sleep(.4);check('QML hidden while Rust libmpv keeps progressing',qs()['state']['position']>pos)
        peer.command('pause');wait_for(lambda:not qs()['state']['playing']);time.sleep(.15);pos=qs()['state']['position'];time.sleep(.2)
        check('mpv pause events reach QML and position stops',abs(qs()['state']['position']-pos)<.01)
        def bus(*a):return subprocess.check_output(['busctl','--user',*a],text=True).strip()
        identity=bus('get-property',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2','Identity')
        bus('call',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','Play');wait_for(lambda:qs()['state']['playing'])
        status=bus('get-property',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','PlaybackStatus')
        bus('call',BUS,'/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','Pause');wait_for(lambda:not qs()['state']['playing'])
        check('MPRIS commands share backend authority and actual status', 'Prototype' in identity and 'Playing' in status)
        before=len(qs()['state']['queue']);ipc('disconnect','true');wait_for(lambda:not qs()['connected']);peer.command('append',trackId='fixture:track:11');ipc('disconnect','false');wait_for(lambda:qs()['connected'] and len(qs()['state']['queue'])==before+1)
        check('disconnected UI resnapshots missed queue edit',len(qs()['state']['queue'])==before+1 and qs()['gaps']==0)
        connections=qs()['connections'];peer.command('fault-gap');wait_for(lambda:qs()['connections']>connections and qs()['gaps']==1)
        check('missed revision forces disconnect and fresh snapshot',qs()['revision']==peer.snapshot()['revision'] and qs()['gaps']==1)
        # UI process restart must not affect the backend, queue, or position.
        old_instance=qs()['instance'];peer.command('play');wait_for(lambda:qs()['state']['playing']);pos=qs()['state']['position'];ipc('quit');ui.wait(timeout=5);time.sleep(.2)
        ui=subprocess.Popen(['quickshell','-n','-p',str(HERE/'shell.qml')],env=env,stdout=qml_log,stderr=qml_log);wait_for(lambda:qs()['instance']==old_instance and qs()['state']['playing'])
        check('UI process restart preserves backend and audio',qs()['state']['position']>pos and len(qs()['state']['queue'])==before+1)
        metrics['combined_rss_mib']=max(samples+[rss(backend.pid)+rss(ui.pid)])/1024**2
        # Abrupt backend crash: prototype deliberately has no durable queue; new epoch, paused.
        peer.close();peer=None;backend.kill();backend.wait(timeout=5);wait_for(lambda:not qs()['connected']);(run/'backend.sock').unlink(missing_ok=True);log.seek(0);log.truncate();backend=start_backend();wait_for(lambda:qs()['restarts']>=1 and qs()['state']['loaded'])
        check('backend restart changes epoch and never autoplays',qs()['instance']!=old_instance and not qs()['state']['playing'] and len(qs()['state']['queue'])==0)
        peer=Peer(run/'backend.sock');stale=peer.command('append',trackId='fixture:track:1',instance=old_instance,operationId='lost-1');check('old-epoch mutation rejected after restart',stale['result'].get('error')=='stale-instance' and not peer.snapshot()['queue'])
        ipc('search','',0,0);wait_for(lambda:qs()['rows']==100);ipc('resetTiming');time.sleep(.1)
        for i in range(10):ipc('search',f'Track {i:03}',0,0)
        wait_for(lambda:qs()['pending']==0);metrics['heartbeat_max_ms']=qs()['maxHeartbeatMs']
        check('local command budget 250ms',metrics['command_ms']<=250,metrics['command_ms'])
        check('catalog page budget 250ms',max(page_times)<=250,page_times)
        check('UI heartbeat interval budget 50ms',metrics['heartbeat_max_ms']<=50,metrics['heartbeat_max_ms'])
        check('combined sampled RSS budget 512MiB',metrics['combined_rss_mib']<=512,metrics['combined_rss_mib'])
        final=qs();ipc('visible','true')
        peer.command('shutdown');peer.close();peer=None;exit_code=backend.wait(timeout=5)
        check('graceful libmpv owner-thread teardown exits cleanly',exit_code==0,exit_code)
        qml_log.flush();warnings=(run/'qml.log').read_text();check('no QML runtime error',not any(x in warnings for x in ['ReferenceError:','TypeError:','Unable to assign','is not a type']))
        evidence={'fixture':{'tracks':100000,'artists':2000,'tracks_per_album':10,'page':100,'page_cap':200},'metrics':metrics,'checks':results,'qml_state':final,'artwork_requests':Artwork.requests,'limits':['Synthetic loopback HTTP only; no Plex or TLS qualification','Null audio only; no fidelity or gapless claim','No durable queue/checkpoint implementation; crash fixture starts empty and paused','Minimal MPRIS subset only; no full compliance claim','IPC controls, not physical keyboard acceptance','One local run; sampled RSS double counts shared mappings and is not a leak proof']}
        (HERE/'evidence/results.json').write_text(json.dumps(evidence,indent=2)+'\n')
        (HERE/'evidence/startup-warnings.txt').write_text(warnings.replace(str(run),'$PROTOTYPE_RUNTIME'))
        subprocess.run([sys.executable,str(HERE/"report.py")],check=True)
        print(json.dumps({'passed':sum(r['pass'] for r in results),'total':len(results),'metrics':metrics,'scratch':str(run)}),flush=True)
        if not all(r['pass'] for r in results):raise SystemExit(1)
    finally:
        if peer:
            try:peer.command('shutdown')
            except (OSError,ValueError):pass
            peer.close()
        if ui and ui.poll() is None:
            try:ipc('quit');ui.wait(timeout=3)
            except (subprocess.SubprocessError,OSError):ui.terminate();ui.wait(timeout=3)
        if backend and backend.poll() is None:backend.terminate();backend.wait(timeout=5)
        art_server.shutdown();log.close();qml_log.close()
if __name__=='__main__':
    try:main()
    except KeyboardInterrupt:pass
