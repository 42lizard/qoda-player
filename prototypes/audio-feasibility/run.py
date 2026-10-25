#!/usr/bin/env python3
"""THROWAWAY: measure libmpv PCM output and async transitions; no real audio devices.
Run: python3 prototypes/audio-feasibility/run.py
"""
import array
import ctypes as C
import ctypes.util
import hashlib
import html
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
import wave

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUTPUT = ROOT / '.scratch' / ('audio-prototype-' + time.strftime('%Y%m%d-%H%M%S') + '-' + str(os.getpid()))
OUTPUT.mkdir(parents=True)
lib = C.CDLL(ctypes.util.find_library('mpv'))

class Event(C.Structure):
    _fields_ = [('id', C.c_int), ('error', C.c_int), ('reply', C.c_uint64), ('data', C.c_void_p)]

class Property(C.Structure):
    _fields_ = [('name', C.c_char_p), ('format', C.c_int), ('data', C.c_void_p)]

class End(C.Structure):
    _fields_ = [('reason', C.c_int), ('error', C.c_int), ('entry', C.c_int64), ('insert', C.c_int64), ('count', C.c_int)]

class Log(C.Structure):
    _fields_ = [('prefix', C.c_char_p), ('level', C.c_char_p), ('text', C.c_char_p), ('number', C.c_int)]

for name, result, args in [
    ('mpv_create', C.c_void_p, []),
    ('mpv_set_option_string', C.c_int, [C.c_void_p, C.c_char_p, C.c_char_p]),
    ('mpv_initialize', C.c_int, [C.c_void_p]),
    ('mpv_command_async', C.c_int, [C.c_void_p, C.c_uint64, C.POINTER(C.c_char_p)]),
    ('mpv_observe_property', C.c_int, [C.c_void_p, C.c_uint64, C.c_char_p, C.c_int]),
    ('mpv_wait_event', C.POINTER(Event), [C.c_void_p, C.c_double]),
    ('mpv_event_name', C.c_char_p, [C.c_int]),
    ('mpv_request_log_messages', C.c_int, [C.c_void_p, C.c_char_p]),
    ('mpv_terminate_destroy', None, [C.c_void_p]),
    ('mpv_client_api_version', C.c_ulong, []),
]:
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args

def checked(code):
    if code < 0:
        raise RuntimeError('libmpv error ' + str(code))

files = {}
raw = {}
fixture_info = {}

def fixture(name, rate=44100, bits=16, channels=2, offset=0, frames=88201, tags=None):
    """Bounded deterministic nonperiodic PCM, <= 1/8 full scale; reproducible by sample index."""
    samples = []
    for i in range(offset, offset + frames):
        for ch in range(channels):
            x = ((i * 1664525 + ch * 1013904223 + 0x12345678) & 0xffffffff)
            x ^= x >> 13
            samples.append(((x & ((1 << (bits-3))-1)) - (1 << (bits-4))) << (32-bits))
    raw[name] = array.array('i', samples).tobytes()
    wav = OUTPUT / (name + '.wav')
    with wave.open(str(wav), 'wb') as w:
        w.setparams((channels, bits//8, rate, frames, 'NONE', 'not compressed'))
        w.writeframes(b''.join(int(s >> (32-bits)).to_bytes(bits//8, 'little', signed=True) for s in samples))
    destination = wav
    if tags is not None or name.startswith('flac'):
        destination = OUTPUT / (name + '.flac')
        command = ['ffmpeg', '-nostdin', '-v', 'error', '-y', '-i', str(wav), '-c:a', 'flac']
        for key, value in (tags or {}).items():
            command += ['-metadata', key + '=' + str(value)]
        subprocess.run(command + [str(destination)], check=True)
    files[destination.name] = destination.read_bytes()
    fixture_info[name] = {'file': destination.name, 'rate': rate, 'bits': bits, 'channels': channels, 'frames': frames, 'tags': tags or {}}
    return name

REQUESTS = []
delayed = set()

class Media(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.respond(False)

    def do_GET(self):
        self.respond(True)

    def respond(self, body):
        url = urlsplit(self.path)
        name = Path(url.path).name
        data = files.get(name)
        if data is None:
            self.send_error(404)
            return
        start, end = 0, len(data)-1
        range_header = self.headers.get('Range')
        if range_header:
            first, last = range_header.removeprefix('bytes=').split('-', 1)
            start = int(first or 0)
            end = min(int(last) if last else end, end)
        if start > end:
            self.send_error(416)
            return
        request = {'t': time.monotonic(), 'path': url.path, 'range': range_header, 'bytes': end-start+1}
        REQUESTS.append(request)
        delay = float(parse_qs(url.query).get('delay', [0])[0])
        if body and delay and url.path not in delayed:
            delayed.add(url.path)
            request['delay_seconds'] = delay
            time.sleep(delay)
        self.send_response(206 if range_header else 200)
        self.send_header('Content-Type', 'audio/flac' if name.endswith('.flac') else 'audio/wav')
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('Content-Length', str(end-start+1))
        if range_header:
            self.send_header('Content-Range', f'bytes {start}-{end}/{len(data)}')
        self.end_headers()
        if body:
            try:
                self.wfile.write(data[start:end+1])
            except (BrokenPipeError, ConnectionResetError):
                pass

server = ThreadingHTTPServer(('127.0.0.1', 0), Media)
server.daemon_threads = True
threading.Thread(target=server.serve_forever, daemon=True).start()
BASE = 'http://127.0.0.1:' + str(server.server_port)

def value(prop):
    if not prop.data or prop.format == 0:
        return None
    if prop.format == 1:
        text = C.cast(prop.data, C.POINTER(C.c_char_p))[0]
        return text.decode(errors='replace') if text else None
    return C.cast(prop.data, C.POINTER({3:C.c_int, 4:C.c_int64, 5:C.c_double}[prop.format]))[0]

def experiment(name, names, options=None, timed=False, actions=None, delays=None, expected_ends=None, per_file=None):
    start = time.monotonic()
    capture = OUTPUT / (name + '.s32le')
    opts = {'config':'no','load-scripts':'no','load-auto-profiles':'no','ytdl':'no','osc':'no','terminal':'no','vo':'null','vid':'no','audio-display':'no',
            'ao':'null' if timed else 'pcm','audio-format':'s32','audio-channels':'auto','volume':'100',
            'volume-gain':'0','mute':'no','speed':'1','replaygain':'no','replaygain-preamp':'0',
            'replaygain-fallback':'0','replaygain-clip':'no','gapless-audio':'yes','prefetch-playlist':'yes',
            'audio-buffer':'0.1','ao-null-buffer':'0.05','ao-null-untimed':'no',
            'ao-pcm-waveheader':'no','ao-pcm-append':'yes','ao-pcm-file':str(capture)}
    opts.update(options or {})
    if per_file:
        opts['pause']='yes'
    handle = lib.mpv_create()
    assert handle
    for key, val in opts.items():
        checked(lib.mpv_set_option_string(handle, key.encode(), val.encode()))
    checked(lib.mpv_initialize(handle))
    lib.mpv_request_log_messages(handle, b'warn')
    props = {'path':1,'time-pos':5,'pause':3,'idle-active':3,'playlist-pos':4,'mpv-version':1,
             'audio-params/samplerate':4,'audio-params/format':1,'audio-params/channels':1,
             'audio-out-params/samplerate':4,'audio-out-params/format':1,'audio-out-params/channels':1}
    for uid, (key, fmt) in enumerate(props.items(), 1):
        checked(lib.mpv_observe_property(handle, uid, key.encode(), fmt))
    events, state, replies, commands = [], {}, {}, []
    request_id = 100
    def command(args):
        nonlocal request_id
        request_id += 1
        argv = (C.c_char_p * (len(args)+1))(*(str(x).encode() for x in args), None)
        checked(lib.mpv_command_async(handle, request_id, argv))
        commands.append({'t':round(time.monotonic()-start,6),'id':request_id,'command':args})
        return request_id
    playlist = OUTPUT / (name + '.m3u')
    urls = [BASE + '/' + name + '/' + fixture_info[n]['file'] + ('?delay='+str(delays[n]) if n in (delays or {}) else '') for n in names]
    playlist.write_text('#EXTM3U\n'+'\n'.join(urls)+'\n')
    if per_file:
        for i,(source,url) in enumerate(zip(names,urls)):
            command(['loadfile',url,'replace' if i==0 else 'append','-1',per_file.get(source,'')])
        command(['set','pause','no'])
    else:
        command(['loadlist', str(playlist), 'replace'])
    ends, idle, last_event = [], False, time.monotonic()
    first_restart = None
    pending_actions = list(actions or [])
    done_actions = []
    try:
        while time.monotonic()-start < 18:
            event = lib.mpv_wait_event(handle, 0.02).contents
            now = time.monotonic()-start
            if event.id:
                item = {'t':round(now,6),'event':lib.mpv_event_name(event.id).decode()}
                if event.id == 22:
                    p = C.cast(event.data, C.POINTER(Property)).contents
                    key, val = p.name.decode(), value(p)
                    state[key] = val
                    item.update(property=key,value=val)
                    if key == 'idle-active':
                        idle = bool(val)
                elif event.id == 5:
                    replies[event.reply] = event.error
                    item.update(id=event.reply,error=event.error)
                elif event.id == 7:
                    end = C.cast(event.data, C.POINTER(End)).contents
                    item.update(reason=end.reason,error=end.error,entry_id=end.entry)
                    ends.append(item.copy())
                elif event.id == 2:
                    log = C.cast(event.data, C.POINTER(Log)).contents
                    item.update(prefix=log.prefix.decode(),text=log.text.decode().strip())
                if event.id == 21 and first_restart is None:
                    first_restart = now
                events.append(item)
                last_event = time.monotonic()
            for action in pending_actions[:]:
                threshold = action['at']
                reached = (first_restart is not None and now-first_restart >= threshold) if action.get('clock') else (state.get('playlist-pos') == 0 and (state.get('time-pos') or 0) >= threshold)
                if reached:
                    uid = command(action['command'])
                    done_actions.append({'label':action['label'],'id':uid,'at':round(now,6),'position':state.get('time-pos'),'playlist_pos':state.get('playlist-pos'),'path':state.get('path')})
                    pending_actions.remove(action)
            need = expected_ends if expected_ends is not None else len(names)
            hold = actions and all(a['label'] in ['pause','stop'] for a in actions)
            if not hold and idle and len(ends) >= need and time.monotonic()-last_event > 0.08:
                break
            if hold and done_actions and now > done_actions[-1]['at']+max((delays or {}).values(),default=0)+0.5:
                break
        elapsed = time.monotonic()-start
    finally:
        lib.mpv_terminate_destroy(handle)
    relevant_requests = [{**r,'t':round(r['t']-start,6)} for r in REQUESTS if r['path'].startswith('/'+name+'/')]
    result = {'name':name,'options':{k:v for k,v in opts.items() if k!='ao-pcm-file'},'timed_null':timed,
              'elapsed_seconds':round(elapsed,6),'events':events,'commands':commands,'actions':done_actions,
              'replies':replies,'ends':ends,'final_state':state,'http':relevant_requests,
              'capture_bytes':capture.stat().st_size if capture.exists() else 0,'timeout':elapsed >= 18}
    result['unexecuted_actions']=pending_actions
    result['all_commands_succeeded']=all(error>=0 for error in replies.values()) and all(c['id'] in replies for c in commands)
    (OUTPUT/(name+'.json')).write_text(json.dumps(result,indent=2))
    print(name+': '+str(len(ends))+' endings, '+str(result['capture_bytes'])+' capture bytes, '+str(round(elapsed,2))+'s',flush=True)
    return result, capture.read_bytes() if capture.exists() else b''

def comparison(data, expected):
    actual = array.array('i'); actual.frombytes(data)
    reference = array.array('i'); reference.frombytes(expected)
    limit = min(len(actual),len(reference))
    first = next((i for i in range(limit) if actual[i]!=reference[i]),None)
    mismatches = sum(actual[i]!=reference[i] for i in range(limit))
    return {'exact':data==expected,'actual_samples':len(actual),'expected_samples':len(reference),
            'first_mismatch_sample':first,'mismatched_samples':mismatches,
            'max_abs_error':max((abs(actual[i]-reference[i]) for i in range(limit)),default=0),
            'sha256':hashlib.sha256(data).hexdigest(),'reference_sha256':hashlib.sha256(expected).hexdigest()}

def gain_comparison(data, sources, gains):
    expected=[]
    for name,gain in zip(sources,gains):
        samples=array.array('i');samples.frombytes(raw[name])
        expected.extend(max(-2147483648,min(2147483647,round(s*gain))) for s in samples)
    expected=array.array('i',expected).tobytes()
    out=comparison(data,expected)
    out['s32_tolerance']=256
    out['within_tolerance']=out['actual_samples']==out['expected_samples'] and out['max_abs_error']<=256
    offset=0;segments=[]
    actual=array.array('i');actual.frombytes(data)
    integer_reference=[]
    for name,gain in zip(sources,gains):
        ref=array.array('i');ref.frombytes(raw[name]);values=actual[offset:offset+len(ref)];offset+=len(ref)
        fitted=sum(a*b for a,b in zip(values,ref))/sum(b*b for b in ref) if len(values)==len(ref) else None
        gi=round(256*gain)
        segments.append({'source':name,'nominal_gain':gain,'fitted_gain':fitted,'integer_gain_256':gi})
        integer_reference.extend(max(-2147483648,min(2147483647,(v*gi+128)>>8)) for v in ref)
    out['segments']=segments
    out['integer_256_model']=comparison(data,array.array('i',integer_reference).tobytes())
    return out

def main():
    fixture('wav-a',offset=0)
    fixture('wav-b',offset=88201)
    fixture('wav-c',offset=176402)
    fixture('flac-a',offset=0)
    fixture('flac-b',offset=88201)
    fixture('flac-24',bits=24,offset=88201)
    fixture('rate-48',rate=48000,offset=0,frames=96000)
    fixture('mono',channels=1,offset=0)
    fixture('rg-a',offset=0,tags={'REPLAYGAIN_TRACK_GAIN':'-6.0 dB','REPLAYGAIN_ALBUM_GAIN':'-12.0 dB','REPLAYGAIN_TRACK_PEAK':'0.125','REPLAYGAIN_ALBUM_PEAK':'0.125'})
    fixture('rg-b',offset=88201,tags={'REPLAYGAIN_TRACK_GAIN':'-12.0 dB','REPLAYGAIN_ALBUM_GAIN':'-12.0 dB','REPLAYGAIN_TRACK_PEAK':'0.125','REPLAYGAIN_ALBUM_PEAK':'0.125'})
    fixture('rg-track-only',offset=0,tags={'REPLAYGAIN_TRACK_GAIN':'-6.0 dB','REPLAYGAIN_TRACK_PEAK':'0.125'})
    fixture('rg-clip',offset=0,tags={'REPLAYGAIN_TRACK_GAIN':'+24.0 dB','REPLAYGAIN_TRACK_PEAK':'0.125'})
    results=[]
    for name,names,opts in [
        ('wav-unity',['wav-a','wav-b'],{}),
        ('flac-unity',['flac-a','flac-b'],{}),
        ('flac-no-prefetch',['flac-a','flac-b'],{'prefetch-playlist':'no'}),
        ('bit-depth-16-24',['flac-a','flac-24'],{}),
        ('rate-strong',['wav-a','rate-48'],{}),
        ('rate-weak',['wav-a','rate-48'],{'gapless-audio':'weak'}),
        ('channels-strong',['wav-a','mono'],{}),
        ('channels-weak',['wav-a','mono'],{'gapless-audio':'weak'}),
        ('rg-track',['rg-a','rg-b'],{'replaygain':'track'}),
        ('rg-album',['rg-a','rg-b'],{'replaygain':'album'}),
        ('rg-missing',['flac-a','flac-b'],{'replaygain':'track'}),
        ('rg-album-fallback',['rg-track-only'],{'replaygain':'album'}),
        ('rg-clipping-prevention',['rg-clip'],{'replaygain':'track'}),
    ]:
        result,data=experiment(name,names,opts)
        if name=='rg-track': result['comparison']=gain_comparison(data,names,[10**(-6/20),10**(-12/20)])
        elif name=='rg-album':result['comparison']=gain_comparison(data,names,[10**(-12/20)]*2)
        elif name=='rg-album-fallback':result['comparison']=gain_comparison(data,names,[10**(-6/20)])
        elif name=='rg-clipping-prevention':result['comparison']=gain_comparison(data,names,[8])
        elif name not in ['rate-strong','channels-strong']:result['comparison']=comparison(data,b''.join(raw[n] for n in names))
        results.append(result)
    for name,per_file,gains in [
        ('high-precision-track',{'rg-a':'af=lavfi=[volume=-6dB:precision=double]','rg-b':'af=lavfi=[volume=-12dB:precision=double]'},[10**(-6/20),10**(-12/20)]),
        ('high-precision-album',{'rg-a':'af=lavfi=[volume=-12dB:precision=double]','rg-b':'af=lavfi=[volume=-12dB:precision=double]'},[10**(-12/20)]*2),
    ]:
        result,data=experiment(name,['rg-a','rg-b'],per_file=per_file)
        result['comparison']=gain_comparison(data,['rg-a','rg-b'],gains)
        results.append(result)
    result,data=experiment('delayed-dump',['wav-a','wav-b'],delays={'wav-b':1.2})
    result['comparison']=comparison(data,raw['wav-a']+raw['wav-b']);results.append(result)
    for name,opts,delay in [('timed-normal',{},0),('timed-delayed',{},1.2),('timed-delay-overrun',{},3.2),('timed-no-prefetch-delay',{'prefetch-playlist':'no'},1.2)]:
        result,_=experiment(name,['wav-a','wav-b'],opts,timed=True,delays={'wav-b':delay} if delay else {})
        results.append(result)
    for name,at in [('remove-early',0.5),('remove-late',1.9)]:
        result,_=experiment(name,['wav-a','wav-b','wav-c'],timed=True,actions=[{'label':'remove upcoming B','at':at,'command':['playlist-remove','1']}],expected_ends=2)
        results.append(result)
    for name,at in [('remove-clock-near-boundary',1.9),('remove-clock-after-boundary',2.05)]:
        result,_=experiment(name,['wav-a','wav-b','wav-c'],timed=True,actions=[{'label':'remove B by engine index','at':at,'clock':True,'command':['playlist-remove','1']}],expected_ends=2)
        results.append(result)
    for name,args in [('pause-delayed',['set','pause','yes']),('stop-delayed',['stop'])]:
        result,_=experiment(name,['wav-a','wav-b'],timed=True,delays={'wav-b':1.2},actions=[{'label':'pause' if name.startswith('pause') else 'stop','at':0.4,'command':args}],expected_ends=1)
        results.append(result)
    versions={'mpv':subprocess.check_output(['mpv','--version'],text=True).splitlines(),
              'libmpv_client_api':hex(lib.mpv_client_api_version()),'python':sys.version.split()[0],
              'byteorder':sys.byteorder}
    report={'scope':'Player-side PCM file capture and timed null output only. No physical audio device, PipeWire monitor or microphone used.',
            'date':time.strftime('%Y-%m-%d'),'versions':versions,'fixtures':fixture_info,'cases':results,
            'limits':['PCM output is untimed; exact dump equality does not establish real-time device gaplessness.',
                      'Timed null observations establish engine/control behavior, not actual output samples or DAC continuity.',
                      'The harness exercises libmpv through ctypes, not the future Rust FFI adapter.',
                      'Lossy trimming, real Plex URLs, authorization expiry, hardware paths and rate-change muting are not measured.']}
    evidence=HERE/'evidence';evidence.mkdir(exist_ok=True)
    (evidence/'results.json').write_text(json.dumps(report,indent=2))
    rows=[]
    for case in results:
        c=case.get('comparison',{})
        verdict='PRECONDITION MISSED' if case['unexecuted_actions'] else 'EXACT' if c.get('exact') else 'GAIN WITHIN TOLERANCE' if c.get('within_tolerance') else 'INTEGER-GAIN MODEL EXACT' if c.get('integer_256_model',{}).get('exact') else 'DIFFERS' if c else 'OBSERVATION'
        rates=sorted(set(e['value'] for e in case['events'] if e.get('property')=='audio-out-params/samplerate' and e.get('value')))
        rows.append(f"| {case['name']} | {verdict} | {c.get('max_abs_error','—')} | {rates} | {case['elapsed_seconds']:.3f}s |")
    (evidence/'summary.md').write_text('# Audio feasibility measurements\n\n'+report['scope']+'\n\nMeasured '+report['date']+'. libmpv client API '+versions['libmpv_client_api']+'.\n\n| Case | PCM result | Max s32 sample error | Observed output rates | Run time |\n| --- | --- | --- | --- | --- |\n'+'\n'.join(rows)+'\n\n'+ '\n'.join('- '+limit for limit in report['limits'])+'\n')
    render_html(report,evidence/'report.html')
    print('Evidence: '+str(evidence),flush=True)
    print('Raw captures: '+str(OUTPUT),flush=True)

def render_html(report,path):
    payload=json.dumps(report).replace('</','<\\/')
    path.write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Qoda audio evidence</title><style>body{font:15px/1.6 system-ui;background:#111713;color:#e6eee4;margin:0;padding:28px;max-width:1100px}h1{font-size:26px}button{font:inherit;padding:8px 12px;margin:4px;background:#263626;color:inherit;border:1px solid #52654d;border-radius:6px}button.active{background:#bfdc9f;color:#152012}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#1c261e;padding:16px}small{color:#a7b49e}.limits{border-left:3px solid #d9b775;padding-left:18px}.metrics{display:flex;gap:35px;flex-wrap:wrap}nav{margin:25px 0}dt{color:#b7c6ae}dd{margin:0 0 12px;font-weight:600}</style><h1>Qoda · audio feasibility evidence</h1><p>Does the installed embedded engine preserve the synthetic source samples, and what happens at asynchronous transitions?</p><p class="limits">Player-side capture only. Exact sample files do not prove real-time speaker/DAC gaplessness. Timed null output does not capture device samples.</p><nav id="cases" aria-label="Measured cases"></nav><section id="current" aria-live="polite"></section><h2>Measurement limits</h2><ul id="limits"></ul><details><summary>Versions and fixture state</summary><pre id="versions"></pre></details><script>const report='''+payload+''';const esc=x=>String(x).replace(/[&<>]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;'}[c]));function choose(index){const c=report.cases[index],m=c.comparison||{};document.getElementById('cases').innerHTML=report.cases.map((v,i)=>'<button class="'+(i===index?'active':'')+'" onclick="choose('+i+')">'+esc(v.name)+'</button>').join('');document.getElementById('current').innerHTML='<h2>'+esc(c.name)+'</h2><div class="metrics"><dl><dt>Output samples</dt><dd>'+esc(m.actual_samples??'Timed null — no capture')+'</dd></dl><dl><dt>Exact equality</dt><dd>'+esc(m.exact??'Not measured')+'</dd></dl><dl><dt>Max s32 error</dt><dd>'+esc(m.max_abs_error??'Not measured')+'</dd></dl><dl><dt>Elapsed</dt><dd>'+c.elapsed_seconds+' s</dd></dl></div><details open><summary>Commands and observed events</summary><pre>'+esc(JSON.stringify({actions:c.actions,commands:c.commands,replies:c.replies,http:c.http,ends:c.ends,final_state:c.final_state,comparison:c.comparison},null,2))+'</pre></details><details><summary>Full event trace</summary><pre>'+esc(JSON.stringify(c.events,null,2))+'</pre></details>'}document.getElementById('limits').innerHTML=report.limits.map(x=>'<li>'+esc(x)+'</li>').join('');document.getElementById('versions').textContent=JSON.stringify({versions:report.versions,fixtures:report.fixtures},null,2);choose(0);</script></html>''')

if __name__=='__main__':
    try:
        main()
    finally:
        server.shutdown()
