#!/usr/bin/env python3
"""Disposable Plex validation. Browser secrets stay in memory/Secret Service.
No playlist mutation, playback reporting, media downloads or desktop changes.
"""
import base64,datetime,http.server,json,os,pathlib,secrets,subprocess,threading,time,urllib.parse,uuid,xml.etree.ElementTree as ET
import requests
from credential import store as store_envelope
HERE=pathlib.Path(__file__).resolve().parent
CLOUD='https://clients.plex.tv'
def b64(data):return base64.urlsafe_b64encode(data).rstrip(b'=').decode()
class Probe:
 def __init__(self):
  self.lock=threading.RLock();self.client='qoda-validation-'+str(uuid.uuid4());self.kid=str(uuid.uuid4());self.key=None;self.token=None;self.pin=None;self.resources=[];self.running=True;self.port=0;self.csrf=secrets.token_urlsafe(32)
  self.status={'phase':'starting','message':'Creating a fresh Qoda validation device.','servers':[],'observations':[],'untested':['account/server authorization','renewal and expiry/revocation','shared access/permission loss','playlist mutations and duplicate entries','original media/range seeking/reconnect','playback reporting','gain/peak provenance','locked/unavailable keyring and session-only behavior']}
  self.headers={'Accept':'application/json','X-Plex-Product':'Qoda Plex Validator','X-Plex-Version':'0.0.0-validation','X-Plex-Client-Identifier':self.client,'X-Plex-Platform':'Linux','X-Plex-Device-Name':'Qoda disposable validation'}
 def observe(self,name,outcome,**details):
  with self.lock:self.status['observations'].append(dict(name=name,outcome=outcome,**details))
  self.save()
 def save(self):
  with self.lock:
   # Whitelist export. Never export server names/addresses/identifiers, tokens or payloads.
   public={'recorded_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'phase':self.status['phase'],'observations':self.status['observations'],'untested':self.status['untested'],'limits':['No universal compatibility claim','No playlist mutation, playback reporting or media transfer performed by this harness','Only certificate-verified HTTPS server connections are attempted','Private response bodies remain in memory; exports contain schema/counts only']}
  (HERE/'evidence/progress.json').write_text(json.dumps(public,indent=2)+'\n')
 def sign(self,claims):
  header=b64(json.dumps({'kid':self.kid,'alg':'EdDSA','typ':'JWT'},separators=(',',':')).encode());payload=b64(json.dumps(claims,separators=(',',':')).encode());msg=(header+'.'+payload).encode();fds=[]
  try:
   for label,data in [('qoda-device-key',self.key),('qoda-device-jwt',msg)]:
    fd=os.memfd_create(label);os.write(fd,data);os.lseek(fd,0,0);fds.append(fd)
   sig=subprocess.check_output(['openssl','pkeyutl','-sign','-rawin','-inkey',f'/proc/self/fd/{fds[0]}','-in',f'/proc/self/fd/{fds[1]}'],pass_fds=fds,stderr=subprocess.DEVNULL,timeout=10)
   return header+'.'+payload+'.'+b64(sig)
  finally:
   for fd in fds:os.close(fd)
 def device_jwt(self,**kw):
  now=int(time.time());return self.sign(dict(aud='plex.tv',iss=self.client,iat=now,exp=now+300,**kw))
 def request(self,method,url,**kw):
  # Never print requests exceptions: their messages can include signed URLs.
  return requests.request(method,url,timeout=(5,15),allow_redirects=False,**kw)
 def auth(self):
  try:
   self.key=subprocess.check_output(['openssl','genpkey','-algorithm','ED25519'],stderr=subprocess.DEVNULL,timeout=10)
   pub=subprocess.check_output(['openssl','pkey','-pubout','-outform','DER'],input=self.key,stderr=subprocess.DEVNULL,timeout=10)
   jwk={'kty':'OKP','crv':'Ed25519','x':b64(pub[-32:]),'kid':self.kid,'alg':'EdDSA','use':'sig'}
   r=self.request('POST',CLOUD+'/api/v2/pins',headers=self.headers,json={'jwk':jwk,'strong':True})
   self.observe('Plex JWT PIN creation','pass' if r.status_code in (200,201) else 'fail',http_status=r.status_code)
   if r.status_code not in (200,201):raise RuntimeError('pin-response')
   self.pin=r.json();assert self.pin.get('id') and self.pin.get('code')
   self.status.update(phase='awaiting-browser-sign-in',message='Complete Plex sign-in for Qoda Plex Validator in Brave. Then return here to choose a server for read-only music checks.')
   self.save();params=urllib.parse.urlencode({'clientID':self.client,'code':self.pin['code'],'context[device][product]':'Qoda Plex Validator','forwardUrl':f'http://127.0.0.1:{self.port}/'})
   subprocess.Popen(['/usr/bin/brave','https://app.plex.tv/auth#?'+params],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   deadline=time.monotonic()+min(int(self.pin.get('expiresIn',900)),900)
   while self.running and time.monotonic()<deadline:
    r=self.request('GET',CLOUD+f'/api/v2/pins/{self.pin["id"]}',headers=self.headers,params={'deviceJWT':self.device_jwt()})
    if r.status_code==429:
     time.sleep(min(30,int(r.headers.get('Retry-After','5'))));continue
    if r.status_code not in (200,201):
     self.observe('Pending PIN exchange','fail',http_status=r.status_code);raise RuntimeError('pin-poll-response')
    data=r.json()
    if data.get('authToken'):
     self.token=data['authToken'];break
    time.sleep(3)
   if not self.token:
    self.status.update(phase='authorization-pending-expired',message='No completed authorization was received. Restart the validator to open a fresh Plex sign-in.');self.save();return
   self.observe('Browser authorization and device-JWT PIN exchange','pass',token_shape='jwt' if self.token.count('.')==2 else 'opaque')
   self.store_credential();self.renew();self.discover()
  except Exception as e:
   self.status.update(phase='needs-attention',message='The validation step stopped safely; see the redacted observation record. No error payload or credential was logged.')
   self.observe('Harness stopped','unverified',error_class=type(e).__name__)
 def store_credential(self):
  value={'client':self.client,'kid':self.kid,'key':self.key.decode(),'token':self.token}
  saved=store_envelope(value)
  self.observe('Own Secret Service versioned credential round trip','pass' if saved else 'fail')
  self.status['credentialMode']='keyring' if saved else 'session-only'

 def renew(self):
  r=self.request('GET',CLOUD+'/api/v2/auth/nonce',headers=self.headers)
  if r.status_code!=200:self.observe('Nonce request','fail',http_status=r.status_code);return
  nonce=r.json().get('nonce');assert nonce
  r=self.request('POST',CLOUD+'/api/v2/auth/token',headers=self.headers,json={'jwt':self.device_jwt(nonce=nonce,scope='')})
  data=r.json() if r.status_code==200 else {};token=data.get('auth_token') or data.get('authToken')
  self.observe('Immediate nonce-based JWT renewal','pass' if token else 'fail',http_status=r.status_code)
  if token:self.token=token;self.store_credential()
 def discover(self):
  r=self.request('GET','https://plex.tv/api/resources',headers={**self.headers,'Accept':'application/xml','X-Plex-Token':self.token},params={'includeHttps':1})
  self.observe('Account resource discovery','pass' if r.status_code==200 else 'fail',http_status=r.status_code)
  if r.status_code!=200:return
  root=ET.fromstring(r.content)
  self.resources=[d for d in root.findall('Device') if 'server' in d.get('provides','').split(',')]
  self.status['servers']=[{'index':i,'name':d.get('name','Unnamed server'),'owned':d.get('owned')=='1','https':sum(c.get('protocol')=='https' for c in d.findall('Connection'))} for i,d in enumerate(self.resources)]
  self.status.update(phase='select-server',message='Choose one authorized server for read-only music metadata checks. No playlists, playback history or server settings will change.');self.save()
 def probe_server(self,index):
  try:
   d=self.resources[index];self.status.update(phase='reading-server',message='Checking the selected server over certificate-verified HTTPS.')
   endpoint=None;headers={**self.headers,'Accept':'application/xml','X-Plex-Token':self.token}
   attempts=[]
   for c in sorted(d.findall('Connection'),key=lambda c:c.get('relay')=='1'):
    if c.get('protocol')!='https':continue
    url=c.get('uri','');parsed=urllib.parse.urlsplit(url)
    if parsed.scheme!='https' or not parsed.hostname or parsed.username or parsed.password:continue
    try:
     r=self.request('GET',url.rstrip('/')+'/',headers=headers);attempts.append({'relay':c.get('relay')=='1','http_status':r.status_code})
     if r.status_code==200:endpoint=url.rstrip('/');identity=ET.fromstring(r.content);break
    except requests.exceptions.SSLError:attempts.append({'error':'certificate-verification-failed'})
    except requests.exceptions.RequestException:attempts.append({'error':'connection-failed'})
   self.observe('Selected server secure endpoint','pass' if endpoint else 'unverified',owned=d.get('owned')=='1',attempts=attempts)
   if not endpoint:
    self.status.update(phase='needs-attention',message='No advertised HTTPS endpoint accepted the device JWT. HTTP and legacy-token fallback were not silently enabled.');self.save();return
   self.observe('PMS identity','observed',pms_version=identity.get('version','unknown'),api_requested='1.2.3')
   headers['X-Plex-Pms-Api-Version']='1.2.3'
   r=self.request('GET',endpoint+'/library/sections',headers=headers)
   self.observe('Library discovery','pass' if r.status_code==200 else 'fail',http_status=r.status_code)
   if r.status_code!=200:return
   sections=ET.fromstring(r.content);music=[x for x in sections.findall('Directory') if x.get('type')=='artist']
   self.observe('Music libraries','observed',count=len(music))
   for library in music[:2]:
    key=library.get('key');pages=[];versions=[]
    for start in (0,5):
     r=self.request('GET',endpoint+f'/library/sections/{urllib.parse.quote(key,safe="")}/all',headers={**headers,'X-Plex-Container-Start':str(start),'X-Plex-Container-Size':'5'},params={'type':10,'sort':'album.titleSort,track.index','includeGuids':1})
     if r.status_code!=200:pages.append({'status':r.status_code});continue
     page=ET.fromstring(r.content);tracks=page.findall('Track');pages.append({'status':r.status_code,'rows':len(tracks),'total':page.get('totalSize'),'offset':page.get('offset'),'attribute_names':sorted(set(k for t in tracks for k in t.attrib))})
     versions.extend(tracks)
    self.observe('Representative music metadata pages','observed',pages=pages)
    if versions:
     track=versions[0];gain_names=sorted({k for e in track.iter() for k in e.attrib if any(x in k.lower() for x in ('gain','peak','loudness'))})
     self.observe('Optional gain-related field presence','observed',fields=gain_names,units_verified=False,provenance_verified=False)
     art=track.get('thumb') or track.get('parentThumb')
     if art and art.startswith('/') and not art.startswith('//'):
      ar=self.request('GET',endpoint+art,headers=headers,stream=True);first=next(ar.iter_content(128),b'');ar.close()
      self.observe('Authenticated artwork response prefix','pass' if ar.status_code==200 and first else 'fail',http_status=ar.status_code,content_type=ar.headers.get('Content-Type','unknown'))
   self.status.update(phase='read-only-checks-complete',message='Bounded read-only observations captured. The wider live matrix remains incomplete; no playlist or playback-state mutation was performed.');self.save()
  except Exception as e:self.status.update(phase='needs-attention',message='A server check stopped safely.');self.observe('Server check stopped','unverified',error_class=type(e).__name__)
 def cleanup(self):
  self.running=False
  p=subprocess.run(['secret-tool','clear','application','qoda-plex-validation','run',self.client],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=15)
  self.token=None;self.key=None;self.resources=[];self.status['servers']=[]
  self.status.update(phase='local-sign-out',message='Local validation credentials cleared. Remove Qoda Plex Validator from Plex Authorized Devices to revoke the device remotely.')
  self.observe('Own local credential cleanup','pass' if p.returncode==0 else 'incomplete',returncode=p.returncode)
P=Probe()
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def host_ok(self):return self.headers.get('Host')==f'127.0.0.1:{P.port}'
 def send(self,body,kind='application/json',code=200):
  self.send_response(code);self.send_header('Content-Type',kind);self.send_header('Cache-Control','no-store');self.send_header('X-Frame-Options','DENY');self.send_header('Content-Security-Policy',"default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'");self.end_headers();self.wfile.write(body)
 def do_GET(self):
  if not self.host_ok():self.send(b'{}',code=403);return
  if self.path=='/status':
   with P.lock:out=json.dumps(P.status).encode()
   self.send(out);return
  if self.path=='/authorize':
   if P.status['phase']!='awaiting-browser-sign-in' or not P.pin:
    self.send(b'Authorization link is not currently active. Return to the validator status page.','text/plain',410);return
   params=urllib.parse.urlencode({'clientID':P.client,'code':P.pin['code'],'context[device][product]':'Qoda Plex Validator','forwardUrl':f'http://127.0.0.1:{P.port}/'})
   self.send_response(302);self.send_header('Location','https://app.plex.tv/auth#?'+params);self.send_header('Cache-Control','no-store');self.end_headers();return
  if self.path!='/':self.send(b'{}',code=404);return
  page='''<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Qoda Plex Validator</title><style>body{font:17px/1.6 system-ui;background:#142019;color:#e5eee6;max-width:850px;margin:50px auto;padding:20px}h1{color:#c6e3a1}button{padding:12px;margin:8px;background:#c6e3a1;border:0;border-radius:6px}pre{white-space:pre-wrap;background:#1c2b21;padding:16px}small{color:#b9c8bc}</style><h1>Qoda / live Plex validation</h1><p id="message">Connecting…</p><p><a href="/authorize" target="_blank" rel="noopener" style="color:#c6e3a1">Open Plex sign-in</a></p><p><small>This is a disposable validator. Read-only music metadata, bounded artwork response checks, and its own keyring entry only. No playlist changes, playback reporting, media download, or server configuration changes.</small></p><div id="servers"></div><h2>Observed / unverified</h2><pre id="checks"></pre><button onclick="post('/cleanup',{})">Clear local validation credentials</button><p><small>Revoking the device remotely is a separate Plex Authorized Devices action. No credentials or server names are included in public evidence.</small></p><script>
const csrf='__CSRF__'; async function post(path,data){await fetch(path,{method:'POST',headers:{'Content-Type':'application/json','X-Qoda-CSRF':csrf},body:JSON.stringify(data)});}
async function refresh(){const s=await(await fetch('/status')).json();document.getElementById('message').textContent=s.message;document.getElementById('checks').textContent=JSON.stringify({phase:s.phase,observations:s.observations,untested:s.untested},null,2);const div=document.getElementById('servers');div.replaceChildren();if(s.phase==='select-server')for(const server of s.servers){const b=document.createElement('button');b.textContent=server.name+' · '+(server.owned?'owned':'shared')+' · '+server.https+' HTTPS endpoints';b.onclick=()=>post('/select',{index:server.index});div.append(b);}}
setInterval(refresh,1500);refresh();</script>'''.replace('__CSRF__',P.csrf)
  self.send(page.encode(),'text/html; charset=utf-8')
 def do_POST(self):
  origin=f'http://127.0.0.1:{P.port}'
  if not self.host_ok() or self.headers.get('Origin')!=origin or self.headers.get('X-Qoda-CSRF')!=P.csrf:self.send(b'{}',code=403);return
  n=int(self.headers.get('Content-Length','0'))
  if n>1024:self.send(b'{}',code=413);return
  try:data=json.loads(self.rfile.read(n))
  except ValueError:self.send(b'{}',code=400);return
  if self.path=='/select' and P.status['phase']=='select-server' and isinstance(data.get('index'),int) and 0<=data['index']<len(P.resources):
   P.status['phase']='reading-server';threading.Thread(target=P.probe_server,args=(data['index'],),daemon=True).start()
  elif self.path=='/cleanup':threading.Thread(target=P.cleanup,daemon=True).start()
  else:self.send(b'{}',code=400);return
  self.send(b'{"accepted":true}')
if __name__=='__main__':
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);P.port=server.server_port
 print(json.dumps({'local_validator':f'http://127.0.0.1:{P.port}/','credential_mode':'memory until own-keyring round trip','public_evidence':'schema/counts only'}),flush=True)
 threading.Thread(target=P.auth,daemon=True).start()
 try:server.serve_forever()
 except KeyboardInterrupt:pass
 finally:server.server_close()
