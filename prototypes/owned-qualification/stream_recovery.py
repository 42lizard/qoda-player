#!/usr/bin/env python3
"""THROWAWAY: one bounded controlled-recovery follow-up, same intact owned-library item.
No deletion/scrobble/metadata edits. Validation branch has null audio + reporting enabled.
Private credentials, raw responses and player state stay in memory/temporary directories.
"""
import base64
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
import xml.etree.ElementTree as ET
import requests

HERE = Path(__file__).resolve().parent
BINARY = Path('/tmp/qoda-reporting-validation-target/debug/qoda-backend')
OBS = []
STAGE = 'preflight'

def observe(name, **values):
    item = {'name': name, **values}
    OBS.append(item)
    print(json.dumps(item), flush=True)

def get_xml(url, headers, params=None, extra=None):
    with requests.get(url, headers={**headers, **(extra or {})}, params=params,
                      timeout=(5, 10), allow_redirects=False, stream=True) as reply:
        if reply.status_code != 200:
            raise RuntimeError('read-not-accepted')
        data = bytearray()
        for chunk in reply.iter_content(65536):
            data.extend(chunk)
            if len(data) > 2 * 1024 * 1024:
                raise RuntimeError('read-too-large')
        return ET.fromstring(data)

def safe_path(base, key):
    if not key.startswith('/') or key.startswith('//'):
        raise RuntimeError('unsafe-feature-path')
    result = urllib.parse.urljoin(base + '/', key)
    a, b = urllib.parse.urlsplit(base), urllib.parse.urlsplit(result)
    if (a.scheme, a.netloc) != (b.scheme, b.netloc) or b.query or b.fragment:
        raise RuntimeError('unsafe-feature-origin')
    return result

class Client:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(2)
        self.socket.connect(str(path))
        self.buffer = b''
        self.state = {}
        self.socket.sendall(b'{"op":"hello","protocol":1}\n')
        self.read(5)
    def read(self, seconds):
        deadline = time.monotonic() + seconds
        while b'\n' not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.socket], [], [], remaining)[0]:
                return False
            data = self.socket.recv(65536)
            if not data: raise RuntimeError('backend-closed')
            self.buffer += data
            if len(self.buffer) > 16 * 1024 * 1024: raise RuntimeError('ipc-too-large')
        line, self.buffer = self.buffer.split(b'\n', 1)
        self.state = json.loads(line)
        return True
    def wait(self, predicate, seconds=20):
        deadline = time.monotonic() + seconds
        while True:
            if self.state.get('error'): raise RuntimeError('backend-error')
            if self.state.get('reporting',{}).get('state') in ('unavailable','suspended','server_termination'):
                raise RuntimeError('reporting-failure')
            if predicate(self.state): return self.state
            if time.monotonic() >= deadline: raise TimeoutError('backend-state-deadline')
            self.read(min(.5, deadline-time.monotonic()))
    def command(self, op, **fields):
        payload = {'op': op, 'instance': self.state['instance'], **fields}
        self.socket.sendall((json.dumps(payload)+'\n').encode())
    def close(self): self.socket.close()

def field(node, key):
    value = node.get(key)
    return int(value) if value is not None else None

def main():
    global STAGE
    os.umask(0o077)
    if not BINARY.is_file(): raise RuntimeError('build-validation-binary-first')
    STAGE = 'credential-load'
    lookup = subprocess.run(['/usr/bin/secret-tool', 'lookup', 'application',
        'qoda-plex-legacy-validation'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        timeout=15, check=True)
    prefix = b'qoda-legacy-credential-v1:'
    if not lookup.stdout.startswith(prefix): raise RuntimeError('missing-validation-signin')
    credential = json.loads(base64.b64decode(lookup.stdout[len(prefix):].strip(), validate=True))
    del lookup
    if set(credential) != {'client', 'token'}: raise RuntimeError('invalid-credential-envelope')
    common = {'Accept': 'application/xml', 'X-Plex-Product': 'Qoda Player',
        'X-Plex-Client-Identifier': credential['client'], 'X-Plex-Token': credential['token']}
    STAGE = 'owned-resource-discovery'
    resource = get_xml('https://plex.tv/api/resources', common, {'includeHttps': 1})
    servers = [node for node in resource.findall('Device') if node.get('owned') == '1'
        and 'server' in node.get('provides', '').split(',')]
    if not servers: raise RuntimeError('no-owned-server')
    server = servers[0]
    headers = {**common, 'X-Plex-Token': server.get('accessToken') or credential['token']}
    base = None
    for connection in sorted(server.findall('Connection'), key=lambda c: c.get('relay') == '1'):
        candidate = connection.get('uri', '').rstrip('/')
        parsed = urllib.parse.urlsplit(candidate)
        if parsed.scheme != 'https' or parsed.username or parsed.password or parsed.query or parsed.fragment:
            continue
        try:
            identity = get_xml(candidate+'/identity', headers)
            if identity.get('machineIdentifier') != server.get('clientIdentifier'): continue
            base = candidate
            pms_version = identity.get('version')
            break
        except (requests.RequestException, RuntimeError, ET.ParseError):
            continue
    if base is None: raise RuntimeError('no-verified-owned-endpoint')
    providers = get_xml(base+'/media/providers', headers)
    library_provider = next(node for node in providers.findall('MediaProvider')
        if node.get('identifier') == 'com.plexapp.plugins.library')
    features = {node.get('type'): node.get('key') for node in library_provider.findall('Feature')}
    timeline = safe_path(base, features['timeline'])
    metadata = safe_path(base, features['metadata']).rstrip('/')
    observe('Preflight', owned_server=True, tls_verified=True, pms_version=pms_version,
        timeline_discovered=True, metadata_discovered=True, production_gates_unchanged=True,
        output='libmpv ao=null', maximum_runs=1, per_run_wall_seconds=60)
    process = client = watchdog = None
    with tempfile.TemporaryDirectory(prefix='qoda-owned-recovery-') as scratch:
        root=Path(scratch)
        env=os.environ.copy()
        for name,subdir in [('XDG_CONFIG_HOME','config'),('XDG_STATE_HOME','state'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_RUNTIME_DIR','runtime')]:
            target=root/subdir;target.mkdir(mode=0o700);env[name]=str(target)
        env['DBUS_SESSION_BUS_ADDRESS']='unix:path='+str(root/'no-session-bus')
        private_log=root/'native.log'
        def start(with_credential):
            nonlocal process,client
            with private_log.open('ab') as log:
                process=subprocess.Popen([str(BINARY),*(['--credential-stdin'] if with_credential else [])],env=env,
                    stdin=subprocess.PIPE if with_credential else subprocess.DEVNULL,stdout=log,stderr=log)
            if with_credential:
                process.stdin.write(json.dumps(credential).encode());process.stdin.close()
            path=Path(env['XDG_RUNTIME_DIR'])/'qoda/backend.sock'
            deadline=time.monotonic()+30
            while not path.exists():
                if process.poll() is not None or time.monotonic()>deadline:raise RuntimeError('native-readiness-failed')
                time.sleep(.05)
            client=Client(path)
        def wait_allowing_recovery(predicate,seconds):
            deadline=time.monotonic()+seconds
            while True:
                if predicate(client.state):return client.state
                if client.state.get('recovery',{}).get('phase')=='exhausted':raise RuntimeError('recovery-exhausted')
                if time.monotonic()>deadline:raise TimeoutError('recovery-observation-deadline')
                client.read(.2)
        try:
            STAGE='private-native-connect';start(True)
            client.wait(lambda s:s.get('sign_in',{}).get('connected'),45)
            if not client.state.get('validation',{}).get('sink')=='null':raise RuntimeError('forced-null-not-observed')
            identities=json.loads((Path(env['XDG_CONFIG_HOME'])/'qoda/source-identities.json').read_text())
            allowed={source:json.loads(scope)[2] for scope,source in identities.items() if json.loads(scope)[1]==server.get('clientIdentifier')}
            if not allowed:raise RuntimeError('no-owned-source')
            candidates=[]
            for source in allowed:
                client.command('browse',source=source,offset='0',query='')
                page=client.wait(lambda s:s.get('page') is not None and s['page'].get('source')==source and not s.get('catalog',{}).get('busy'))['page']
                for track in page['tracks']:
                    if int(track.get('duration_ms') or 0)==19737:candidates.append((source,track['reference']['key']))
            if len(candidates)!=1:raise RuntimeError('previous-subject-not-unique')
            source,key=candidates[0]
            if client.state.get('page',{}).get('source')!=source:
                client.command('browse',source=source,offset='0',query='')
                client.wait(lambda s:s.get('page') is not None and s['page'].get('source')==source and not s.get('catalog',{}).get('busy'))
            STAGE='owned-artwork-cache-refresh'
            def album_page(art_source):
                client.command('album_list',source=art_source,offset='0',query='')
                client.wait(lambda s:s.get('albums',{}).get('page') is not None and s['albums']['page'].get('source')==art_source
                    and not s['albums'].get('busy'),20)
                return client.state['albums']['covers']
            def image(url):
                parsed=urllib.parse.urlsplit(url)
                if parsed.scheme!='http' or parsed.hostname!='127.0.0.1' or not parsed.path.startswith('/art/'):
                    raise RuntimeError('nonlocal-artwork-grant')
                with requests.Session() as local:
                    local.trust_env=False
                    with local.get(url,timeout=(3,25),allow_redirects=False,stream=True) as response:
                        if response.status_code!=200:return response.status_code,b''
                        data=bytearray()
                        for chunk in response.iter_content(65536):
                            data.extend(chunk)
                            if len(data)>4*1024*1024:raise RuntimeError('artwork-too-large')
                        if not data.startswith(b'\xff\xd8') or not data.endswith(b'\xff\xd9'):
                            raise RuntimeError('invalid-owned-jpeg')
                        return response.status_code,bytes(data)
            cover_key=cover_bytes=live_cover_url=None
            art_source=source
            examined=0
            for candidate_source in dict.fromkeys([source,*allowed]):
                covers=album_page(candidate_source)
                observe('Owned album-page response',album_rows=len(client.state['albums']['page']['albums']),local_grants=len(covers))
                for candidate_key,url in list(covers.items())[:max(0,5-examined)]:
                    examined+=1
                    status,data=image(url)
                    if status==200:
                        cover_key,cover_bytes,live_cover_url=candidate_key,data,url
                        art_source=candidate_source
                        break
                if cover_key is not None or examined>=5:break
            if cover_key is None:raise RuntimeError('no-owned-artwork-fixture')
            STAGE='private-artwork-disk-fill'
            client.wait(lambda s:s.get('artwork_cache',{}).get('entries',0)>0,5)
            cached=list((Path(env['XDG_CACHE_HOME'])/'qoda/artwork-v1').glob('*.jpg'))
            if len(cached)!=1:raise RuntimeError('unexpected-private-artwork-file-count')
            old=time.time()-8*86400
            for path in cached:os.utime(path,(old,old),follow_symlinks=False)
            STAGE='private-artwork-memory-eviction'
            client.command('validation_artwork_forget_memory')
            client.wait(lambda s:s.get('albums',{}).get('covers',{}).get(cover_key) not in (None,live_cover_url),5)
            covers=client.state['albums']['covers']
            stale_url=covers[cover_key]
            status,stale_bytes=image(stale_url)
            if status!=200 or stale_bytes!=cover_bytes:raise RuntimeError('stale-disk-artwork-not-restored')
            STAGE='owned-artwork-background-refresh'
            fresh=client.wait(lambda s:s.get('albums',{}).get('covers',{}).get(cover_key) not in (None,stale_url)
                and not s.get('artwork_cache',{}).get('stale') and not s.get('artwork_cache',{}).get('refreshing'),30)
            live_cover_url=fresh['albums']['covers'][cover_key]
            status,fresh_bytes=image(live_cover_url)
            if status!=200 or image(stale_url)[0]!=404:raise RuntimeError('artwork-rotation-not-confirmed')
            if not all(path.stat().st_mtime>time.time()-30 for path in cached):raise RuntimeError('disk-artwork-not-refreshed')
            observe('Owned artwork expiry refresh',initial_and_fresh_jpeg=True,expired_private_disk_restored=True,same_session_validation_memory_eviction=True,
                background_provider_refresh_rotated_local_url=True,old_grant_http=404,
                cached_and_fresh_bytes_equal=fresh_bytes==cover_bytes,only_private_cache_mtime_changed=True,
                remote_artwork_edited=False,remote_changed_content_invalidation_not_measured=True)
            client.command('browse',source=source,offset='0',query='')
            client.wait(lambda s:s.get('page') is not None and s['page'].get('source')==source and not s.get('catalog',{}).get('busy'))
            metadata_before=get_xml(metadata+'/'+urllib.parse.quote(key,safe=''),headers)
            node_before=next(node for node in metadata_before if node.tag=='Track' and node.get('ratingKey')==key)
            before_fields={name:node_before.get(name) for name in ('viewOffset','viewCount','lastViewedAt')}
            observe('Subject selected',owned_library=True,track_duration_ms=19737,library_track_intact=True)
            STAGE='bounded-render';started=time.monotonic()
            watchdog=threading.Timer(60,process.kill);watchdog.daemon=True;watchdog.start()
            client.command('play',source=source,key=key)
            client.wait(lambda s:s.get('presentation',{}).get('known') and s['presentation'].get('status')=='Playing'
                and (s['presentation'].get('position_seconds') or 0)>=.5,20)
            before=client.state
            occurrence=before['queue']['selected'];session=before['queue']['session'];generation=before['audio']['generation']
            visit=before['validation']['visit'];version=before['validation']['version'];position=before['presentation']['position_seconds']
            if not version:raise RuntimeError('no-pinned-version')
            observe('Confirmed original presentation',position_ms=round(position*1000),source_and_track_match=before['selection']['reference']=={'source':source,'key':key},
                fault_not_yet_fired=not before['validation']['fired'])
            client.command('validation_fault')
            resumed=wait_allowing_recovery(lambda s:s.get('validation',{}).get('fired') and s.get('recovery',{}).get('attempts',0)>=1
                and (s.get('audio') or {}).get('generation')!=generation and s.get('presentation',{}).get('known')
                and s['presentation'].get('status')=='Playing' and (s['presentation'].get('position_seconds') or 0)>max(.25,position-.35),25)
            if resumed.get('error'):raise RuntimeError('resumed-state-has-error')
            pinned=(resumed['queue']['selected']==occurrence and resumed['queue']['session']==session
                and resumed['validation']['visit']==visit and resumed['validation']['version']==version
                and resumed['selection']['reference']=={'source':source,'key':key})
            if not pinned:raise RuntimeError('recovery-identity-drift')
            observe('Controlled current-stream recovery',single_body_fault=True,attempts=resumed['recovery']['attempts'],phase=resumed['recovery']['phase'],
                pinned_occurrence_session_visit_version_and_reference=pinned,new_render_generation=True,
                resumed_position_ms=round(resumed['presentation']['position_seconds']*1000),proxy_body_opens=resumed['validation']['proxy_body_opens'],
                nonzero_range_opens=resumed['validation']['nonzero_range_opens'],client_side_transport_boundary=True,physical_network_outage_simulated=False)
            client.command('pause')
            paused=client.wait(lambda s:s.get('intent')=='paused' and (s.get('audio') or {}).get('paused'),5)
            observe('Explicit pause retained',same_occurrence=paused['queue']['selected']==occurrence,
                same_visit=paused['validation']['visit']==visit,position_ms=round((paused['queue'].get('position_seconds') or 0)*1000))
            STAGE='disconnect-cleanup';client.command('disconnect')
            disconnected=client.wait(lambda s:not s.get('sign_in',{}).get('connected') and not s.get('libraries') and s.get('page') is None
                and s.get('selection') is None and s.get('audio') is None and not s.get('queue',{}).get('entries')
                and not s.get('storage',{}).get('cleanup_pending') and not s.get('archive',{}).get('cleanup_pending')
                and not s.get('metadata_cache',{}).get('cleanup_pending'),10)
            if disconnected.get('archive',{}).get('total')!=0 or disconnected.get('metadata_cache',{}).get('tracks')!=0:
                raise RuntimeError('account-cache-cleanup-incomplete')
            observe('Native disconnect cleanup',connected=False,libraries=0,queue_entries=0,archive_entries=0,
                metadata_tracks=0,cleanup_pending=False,remember_or_forget_not_requested=True,revoked_artwork_grant_http=image(live_cover_url)[0],
                private_artwork_files_remaining=len(list((Path(env['XDG_CACHE_HOME'])/'qoda/artwork-v1').glob('*.jpg'))))
            if image(live_cover_url)[0]!=404 or list((Path(env['XDG_CACHE_HOME'])/'qoda/artwork-v1').glob('*.jpg')):
                raise RuntimeError('artwork-disconnect-cleanup-incomplete')
            elapsed=time.monotonic()-started
            if elapsed>60:raise RuntimeError('playback-budget-exceeded')
            watchdog.cancel();watchdog=None
            client.command('quit');process.wait(timeout=10)
            if process.returncode!=0:raise RuntimeError('native-exit-not-clean')
            client.close();client=None
            observe('Bounded run ended',wall_seconds=round(elapsed,2),speaker_output_opened=False,native_exit=process.returncode)
            STAGE='session-only-restart';start(False)
            deadline=time.monotonic()+2
            while time.monotonic()<deadline:client.read(.2)
            state=client.state
            if state.get('sign_in',{}).get('connected') or state.get('libraries') or state.get('queue',{}).get('entries') or state.get('intent')!='paused':
                raise RuntimeError('session-only-credential-or-selection-restored')
            if state.get('archive',{}).get('total')!=0:raise RuntimeError('archive-data-restored-after-disconnect')
            observe('Fresh process without credential',connected=False,libraries=0,queue_entries=0,archive_entries=0,paused=True,
                same_private_xdg_profile=True,desktop_keyring_unavailable=True)
            client.command('quit');process.wait(timeout=10)
            if process.returncode!=0:raise RuntimeError('restart-exit-not-clean')
            client.close();client=None
            after=get_xml(metadata+'/'+urllib.parse.quote(key,safe=''),headers)
            node_after=next(node for node in after if node.tag=='Track' and node.get('ratingKey')==key)
            observe('Original metadata after test',track_still_present=True,
                observed_offset_changed=before_fields['viewOffset']!=node_after.get('viewOffset'),
                observed_count_changed=before_fields['viewCount']!=node_after.get('viewCount'),
                observed_last_viewed_changed=before_fields['lastViewedAt']!=node_after.get('lastViewedAt'),
                deletion_scrobble_metadata_and_playlist_mutations=0)
        finally:
            if client is not None:
                current=client.state
                observe('Final private-state shape',stage=STAGE,connected=bool(current.get('sign_in',{}).get('connected')),
                    albums_page_present=current.get('albums',{}).get('page') is not None,
                    album_rows=len((current.get('albums',{}).get('page') or {}).get('albums',[])),
                    cover_grants=len(current.get('albums',{}).get('covers',{})),
                    albums_busy=bool(current.get('albums',{}).get('busy')),
                    albums_message_present=bool(current.get('albums',{}).get('message')),
                    no_albums_message=current.get('albums',{}).get('message')=='No albums found.',
                    audio_present=current.get('audio') is not None,validation_fault_fired=current.get('validation',{}).get('fired'),
                    intent_paused=current.get('intent')=='paused',
                    artwork_state=current.get('artwork_cache',{}).get('state'),
                    artwork_entries=current.get('artwork_cache',{}).get('entries'),
                    artwork_usage_known=current.get('artwork_cache',{}).get('usage_known'))
            if watchdog is not None:watchdog.cancel()
            if process is not None and process.poll() is None:
                if client is not None:
                    try:client.command('quit');process.wait(timeout=8)
                    except (OSError,RuntimeError,subprocess.TimeoutExpired):pass
                if process.poll() is None:
                    process.terminate()
                    try:process.wait(timeout=5)
                    except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
            if client is not None:client.close()
            observe('Owned backend stopped',stopped=process is None or process.poll() is not None)
    observe('Private state removed',temporary_xdg_removed=True,original_credential_untouched=True)

if __name__=='__main__':
    success=False
    try:main();success=True
    except Exception as error:
        observe('Stopped',stage=STAGE,error_class=type(error).__name__,safe_reason=str(error) if type(error) in (RuntimeError,TimeoutError) and str(error).replace('-','').isalnum() else None,raw_error_omitted=True)
    finally:
        (HERE/'stream-recovery-evidence.json').write_text(json.dumps({'scope':'same intact owned item; one <=60-second client-side body interruption/null-output recovery and private disconnect/session-only restart',
            'completed':success,'observations':OBS,'limits':['No physical network-outage or hardware qualification','Native backend path; no browser/QML/PAM acceptance','No universal server or revocation guarantee','Timeline effects possible; no scrobble/completion claim']},indent=2)+'\n')
    raise SystemExit(0 if success else 1)
