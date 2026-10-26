#!/usr/bin/env python3
"""THROWAWAY: one bounded duplicate-handoff follow-up, same intact owned-library item.
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
            if self.state.get('reporting',{}).get('state') in ('suspended','server_termination'):
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
    process = None
    client = None
    watchdog = None
    with tempfile.TemporaryDirectory(prefix='qoda-live-reporting-') as scratch:
        root = Path(scratch)
        env = os.environ.copy()
        for name, subdir in [('XDG_CONFIG_HOME','config'),('XDG_STATE_HOME','state'),
            ('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_RUNTIME_DIR','runtime')]:
            target = root/subdir; target.mkdir(mode=0o700); env[name]=str(target)
        env['DBUS_SESSION_BUS_ADDRESS']='unix:path='+str(root/'no-session-bus')
        log_path = root/'private-backend.log'
        with log_path.open('wb') as log:
            try:
                STAGE = 'isolated-backend-connect'
                process = subprocess.Popen([str(BINARY),'--credential-stdin'], env=env,
                    stdin=subprocess.PIPE, stdout=log, stderr=log)
                process.stdin.write(json.dumps(credential).encode()); process.stdin.close()
                sock = Path(env['XDG_RUNTIME_DIR'])/'qoda/backend.sock'
                deadline = time.monotonic()+30
                while not sock.exists():
                    if process.poll() is not None or time.monotonic()>deadline:
                        raise TimeoutError('backend-readiness')
                    time.sleep(.1)
                client=Client(sock)
                client.wait(lambda s:s.get('sign_in',{}).get('connected'),45)
                if client.state.get('reporting',{}).get('state') != 'ready':
                    raise RuntimeError('validation-reporting-not-enabled')
                identities=json.loads((Path(env['XDG_CONFIG_HOME'])/'qoda/source-identities.json').read_text())
                allowed={source:json.loads(scope)[2] for scope,source in identities.items()
                    if json.loads(scope)[1]==server.get('clientIdentifier')}
                if not allowed: raise RuntimeError('no-owned-source')
                STAGE='bounded-subject-selection'
                candidates=[]
                for source in allowed:
                    client.command('browse',source=source,offset='0',query='')
                    page=client.wait(lambda s:s.get('page') is not None and
                        s['page'].get('source')==source and not s.get('catalog',{}).get('busy'))['page']
                    for track in page['tracks']:
                        duration=int(track.get('duration_ms') or 0)
                        if duration>=15000:
                            candidates.append((duration,source,track['reference']['key']))

                if not candidates: raise RuntimeError('no-bounded-fixture-candidate')
                matches=[candidate for candidate in candidates if candidate[0]==19737]
                if len(matches)!=1: raise RuntimeError("previous-subject-not-unique")
                duration,source,key=matches[0]
                if not key.isascii() or not key.isdecimal(): raise RuntimeError('unsupported-fixture-key')
                section=allowed[source]
                if client.state.get('page',{}).get('source') != source:
                    client.command('browse',source=source,offset='0',query='')
                    client.wait(lambda s:s.get('page') is not None and s['page'].get('source')==source
                        and not s.get('catalog',{}).get('busy')
                        and any(t['reference']['key']==key for t in s['page']['tracks']))
                observe('Subject selected', owned_library=True, track_duration_ms=duration,
                    short_enough_for_complete_second_run=duration<=55000,
                    private_library_track_names_and_ids_exported=False, existing_item_preserved=True)
                def snapshot(label):
                    detail=get_xml(metadata+'/'+urllib.parse.quote(key,safe=''),headers,{'skipRefresh':1})
                    track=next(node for node in detail if node.get('ratingKey')==key)
                    sessions=get_xml(base+'/status/sessions',headers)
                    own=[node for node in sessions if node.get('ratingKey')==key
                        and node.find('Player') is not None
                        and node.find('Player').get('machineIdentifier')==credential['client']]
                    history=get_xml(base+'/status/sessions/history/all',headers,
                        {'metadataItemID':key,'librarySectionID':section,'sort':'viewedAt:desc'},
                        {'X-Plex-Container-Start':'0','X-Plex-Container-Size':'100'})
                    rows=[node for node in history if node.get('ratingKey')==key]
                    result={'offset':field(track,'viewOffset'),'count':field(track,'viewCount'),
                        'last':field(track,'lastViewedAt'), 'history':{node.get('historyKey') for node in rows
                            if node.get('historyKey')}, 'history_total':field(history,'totalSize'), 'own_states':[node.find('Player').get('state') for node in own]}
                    observe('Server observation', phase=label, matching_client_sessions=len(own),
                        client_states=[node.find('Player').get('state') for node in own],
                        metadata_fields_present=[name for name in ['viewOffset','viewCount','lastViewedAt'] if track.get(name) is not None],
                        bounded_history_rows=len(rows), history_server_total=result['history_total'],
                        raw_timestamps_offsets_and_identifiers_exported=False)
                    return result
                before=snapshot('before')
                for run in (4,):
                    STAGE='duplicate-handoff'
                    started=time.monotonic()
                    watchdog=threading.Timer(60,process.kill)
                    watchdog.daemon=True; watchdog.start()
                    client.command('play',source=source,key=key)
                    client.wait(lambda s:s.get('presentation',{}).get('known') and
                        s['presentation'].get('status')=='Playing',20)
                    time.sleep(.1)
                    while client.read(.02): pass
                    first=client.state['queue']['selected']
                    first_generation=client.state['audio']['generation']
                    client.command('enqueue',source=source,key=key,
                        queue_session=client.state['queue']['session'],queue_revision=client.state['queue']['revision'])
                    client.wait(lambda s:len(s.get('queue',{}).get('entries',[]))==2,5)
                    client.wait(lambda s:(s.get('presentation',{}).get('position_seconds') or 0)>=3,8)
                    live=snapshot('first-playing')
                    if live['own_states']!=['playing']:
                        raise RuntimeError('playing-session-not-observed')
                    staged=client.wait(lambda s:s.get('gapless',{}).get('phase')=='staged',10)
                    observe('Native successor staged', same_track=True, distinct_local_occurrences=True,
                        source_scope_matches=True, phase=staged['gapless']['phase'])
                    successor=client.wait(lambda s:s.get('queue',{}).get('selected') not in (None,first)
                        and s.get('presentation',{}).get('known')
                        and s['presentation'].get('status')=='Playing',25)
                    position=successor['presentation']['position_seconds']
                    observe('Confirmed duplicate handoff', distinct_occurrence=True,
                        distinct_render_generation=successor['audio']['generation']!=first_generation,
                        successor_nominal_position_ms=round(position*1000),
                        source_and_track_match=successor['selection']['reference']=={'source':source,'key':key})
                    if not 0<position<3: raise RuntimeError('successor-clock-not-local')
                    client.wait(lambda s:(s.get('presentation',{}).get('position_seconds') or 0)>=12,17)
                    observe('Successor reporting status',state=client.state.get('reporting',{}).get('state'))
                    live=snapshot('successor-playing')
                    if live['own_states']!=['playing']:
                        raise RuntimeError('successor-playing-session-not-observed')
                    client.command('stop')
                    client.wait(lambda s:s.get('audio') is None and s.get('intent')=='paused',4)
                    # Give native idle acknowledgement and the serialized terminal request
                    # a bounded opportunity; the raw audit log distinguishes actual sends.
                    stop_until=time.monotonic()+3.5
                    while time.monotonic()<stop_until: client.read(.25)
                    elapsed=time.monotonic()-started
                    if elapsed>60: raise RuntimeError('run-budget-exceeded')
                    watchdog.cancel(); watchdog=None
                    observe('Run stopped',run=run,wall_seconds=round(elapsed,2),
                        queue_ended=client.state.get('queue',{}).get('ended',False),speaker_output_opened=False)
                    after=snapshot('after-run-'+str(run))
                    count_delta=None if before['count'] is None or after['count'] is None else after['count']-before['count']
                    observe('Separate observed effects',run=run,
                        offset_changed=before['offset']!=after['offset'], view_count_delta=count_delta,
                        view_count_missing_before=before['count'] is None, view_count_missing_after=after['count'] is None,
                        last_viewed_changed=before['last']!=after['last'],
                        new_subject_history_rows=len(after['history']-before['history']),
                        history_attribution_to_client_proven=False)
                    before=after
                STAGE='orderly-quit'
                client.command('quit'); process.wait(timeout=10)
                if process.returncode!=0: raise RuntimeError('backend-exit-not-clean')
                client.close(); client=None
            finally:
                if process is not None and process.poll() is None:
                    if client is not None:
                        try: client.command('quit'); process.wait(timeout=8)
                        except (OSError,RuntimeError,subprocess.TimeoutExpired): pass
                    if process.poll() is None: process.terminate()
                    try: process.wait(timeout=8)
                    except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
                if client is not None: client.close()
                if watchdog is not None: watchdog.cancel()
                observe('Owned backend cleanup', process_stopped=process is None or process.poll() is not None)
                audit=[]
                for line in log_path.read_text(errors='replace').splitlines():
                    for prefix in ('QODA_VALIDATION_TIMELINE ','QODA_VALIDATION_RESULT ','QODA_VALIDATION_DEBUG '):
                        if line.startswith(prefix): audit.append(json.loads(line[len(prefix):]))
                observe('Sanitized transport audit', events=audit, backend_exit=process.returncode,
                    exact_production_transport=True, validation_build=True)
    observe('Cleanup',owned_backend_stopped=True,temporary_state_removed=True,
        library_track_preserved=True,delete_scrobble_and_metadata_edit_calls=0)

if __name__=='__main__':
    success=False
    try: main(); success=True
    except Exception as error:
        observe('Stopped',stage=STAGE,error_class=type(error).__name__,safe_reason=str(error) if type(error) in (RuntimeError,TimeoutError) and str(error).replace('-','').isalnum() else None,raw_error_omitted=True)
    finally:
        (HERE/'evidence'/'diagnostic.json').write_text(json.dumps({'scope':'same intact item, one diagnostic duplicate-handoff run, 60-second watchdog',
            'completed':success,'observations':OBS,'limits':['No universal server/account compatibility claim','No speaker/DAC/analog or final-device continuity claim','No scrobble/completion rule/history import','History row device attribution requires separate proof']},indent=2)+'\n')
    raise SystemExit(0 if success else 1)
