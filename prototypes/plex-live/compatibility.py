#!/usr/bin/env python3
"""Explicit legacy PIN compatibility experiment after the documented JWT failed PMS.
Only its own token is stored; no other application's credential is used.
"""
import argparse,datetime,http.server,json,pathlib,secrets,subprocess,threading,time,urllib.parse,uuid
import requests
from credential import store_legacy
HERE=pathlib.Path(__file__).resolve().parent
class Flow:
 def __init__(self):
  self.client='qoda-legacy-validation-'+str(uuid.uuid4());self.code=None;self.pin_id=None;self.port=0;self.stopped=False;self.epoch=0;self.lock=threading.RLock();self.csrf=secrets.token_urlsafe(32)
  self.headers={'Accept':'application/json','X-Plex-Product':'Qoda Plex Compatibility Validator','X-Plex-Version':'0.0.0-validation','X-Plex-Client-Identifier':self.client,'X-Plex-Platform':'Linux'}
  self.status={'phase':'starting','message':'Starting an explicit legacy PIN compatibility test.','observations':[]}
 def observe(self,name,**kw):
  with self.lock:
   self.status['observations'].append({'name':name,**kw});self.save()
 def save(self):
  (HERE/'evidence/legacy-auth.json').write_text(json.dumps({'phase':self.status['phase'],'observations':self.status['observations'],'recorded_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'scope':'own dedicated legacy-PIN test device only; no playlist/timeline/music-file actions','limits':['No legacy token expiry or automatic renewal assumed','This is a separate explicitly selected authentication experiment; not silently substituted for the rejected JWT']},indent=2)+'\n')
 def url(self):return 'https://app.plex.tv/auth#?'+urllib.parse.urlencode({'clientID':self.client,'code':self.code,'context[device][product]':'Qoda Plex Compatibility Validator','forwardUrl':f'http://127.0.0.1:{self.port}/'})
 def run(self):
  try:
   r=requests.post('https://plex.tv/api/v2/pins',headers=self.headers,data={'strong':'true'},timeout=(5,15),allow_redirects=False);self.observe('Legacy PIN creation',http_status=r.status_code)
   assert r.status_code in (200,201);pin=r.json();self.code=pin['code'];self.pin_id=pin['id'];self.status.update(phase='awaiting-browser-sign-in',message='Complete Qoda Plex Compatibility Validator sign-in in Brave. This explicitly tests the older PIN flow against the server that rejected JWTs.');self.save()
   subprocess.Popen(['/usr/bin/brave',self.url()],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   end=time.monotonic()+min(int(pin.get('expiresIn',900)),900)
   while not self.stopped and time.monotonic()<end:
    r=requests.get(f'https://plex.tv/api/v2/pins/{self.pin_id}',headers=self.headers,params={'code':self.code},timeout=(5,15),allow_redirects=False)
    if r.status_code==429:time.sleep(5);continue
    assert r.status_code==200;token=r.json().get('authToken')
    if token:
     with self.lock:
      if self.stopped:return
      stored=store_legacy({'client':self.client,'token':token});self.observe('Legacy browser exchange and own keyring round trip',token_shape='jwt' if token.count('.')==2 else 'opaque',keyring_verified=stored)
      assert stored;self.status.update(phase='authorized-read-only-probe',message='Compatibility authorization completed. Running the same bounded read-only music checks.');self.save()
     p=subprocess.run(['python3',str(HERE/'read_probe.py'),'--legacy'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=150)
     with self.lock:
      if self.stopped:return
      self.status.update(phase='read-only-probe-complete',message='Read-only compatibility observations captured. Playlist edits, playback reporting and controlled permission tests remain unverified.');self.observe('Read-only probe process',exit_code=p.returncode)
     return
    time.sleep(3)
   self.status.update(phase='authorization-expired',message='The compatibility sign-in expired. Restart this helper for a fresh link.');self.save()
  except Exception as e:self.status.update(phase='needs-attention',message='The compatibility probe stopped safely. No credential/error body was logged.');self.observe('Compatibility step stopped',error_class=type(e).__name__)
F=Flow()
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def send(self,body,kind='application/json',status=200):
  self.send_response(status);self.send_header('Content-Type',kind);self.send_header('Cache-Control','no-store');self.send_header('X-Frame-Options','DENY');self.send_header('Content-Security-Policy',"default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'");self.end_headers();self.wfile.write(body)
 def do_GET(self):
  if self.headers.get('Host')!=f'127.0.0.1:{F.port}':self.send(b'{}',status=403);return
  if self.path=='/authorize':
   if F.status['phase']!='awaiting-browser-sign-in':self.send(b'No active sign-in link.','text/plain',410);return
   self.send_response(302);self.send_header('Location',F.url());self.send_header('Cache-Control','no-store');self.end_headers();return
  if self.path=='/status':
   with F.lock:out=json.dumps(F.status).encode()
   self.send(out);return
  if self.path!='/':self.send(b'{}',status=404);return
  page='''<!doctype html><meta charset="utf-8"><title>Qoda Plex compatibility check</title><style>body{font:18px/1.6 system-ui;max-width:850px;margin:50px auto;background:#142019;color:#dce9de;padding:20px}a{color:#c8e4a5}pre{white-space:pre-wrap;font-size:14px}</style><h1>Qoda / Plex compatibility check</h1><p>The device JWT signed in successfully with Plex's cloud API, but the tested server rejected it. This page tests a separate older browser PIN flow.</p><p><a href="/authorize" target="_blank" rel="noopener">Open Plex compatibility sign-in</a></p><p id="message"></p><pre id="observations"></pre><p>Read-only metadata and a bounded artwork prefix only. No playlist edits, playback reporting or music-file transfers. Credentials stay in this test device's own keyring item.</p><script>async function refresh(){const s=await(await fetch('/status')).json();document.getElementById('message').textContent=s.message;document.getElementById('observations').textContent=JSON.stringify(s.observations,null,2)}setInterval(refresh,1500);refresh()</script>'''
  self.send(page.encode(),'text/html; charset=utf-8')
if __name__=='__main__':
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);F.port=server.server_port;print(json.dumps({'compatibility_validator':f'http://127.0.0.1:{F.port}/'}),flush=True);threading.Thread(target=F.run,daemon=True).start()
 try:server.serve_forever()
 except KeyboardInterrupt:pass
 finally:
  with F.lock:F.stopped=True
  server.server_close()
