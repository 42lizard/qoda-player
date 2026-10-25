"""Own-item Secret Service adapter for the disposable probe; never prints secrets."""
import base64,json,subprocess
PREFIX='qoda-credential-v1:'
def decode(data):
 text=data.decode().rstrip('\n') if isinstance(data,bytes) else data.rstrip('\n')
 repaired=False
 if text.startswith(PREFIX):value=json.loads(base64.b64decode(text[len(PREFIX):],validate=True))
 else:
  try:value=json.loads(text)
  except json.JSONDecodeError:
   # Migrate this harness's old single-line JSON envelope only. The observed
   # stored PEM newlines were literal controls. Reject arbitrary non-envelope data.
   assert text.startswith('{') and text.endswith('}')
   value=json.loads(text.replace('\n','\\n').replace('\r','\\r'));repaired=True
 assert isinstance(value,dict) and set(value)=={'client','kid','key','token'}
 assert value['client'].startswith('qoda-validation-') and value['key'].startswith('-----BEGIN PRIVATE KEY-----\n')
 assert value['token'] and value['kid']
 return value,repaired

def load():
 p=subprocess.run(['/usr/bin/secret-tool','lookup','application','qoda-plex-validation'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10)
 assert p.returncode==0
 return decode(p.stdout)

def store(value):
 payload=PREFIX+base64.b64encode(json.dumps(value,separators=(',',':')).encode()).decode()
 attrs=['application','qoda-plex-validation','run',value['client']]
 p=subprocess.run(['/usr/bin/secret-tool','store','--label=Qoda disposable Plex validation',*attrs],input=payload,text=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=30)
 if p.returncode:return False
 p=subprocess.run(['/usr/bin/secret-tool','lookup',*attrs],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10)
 return p.returncode==0 and p.stdout.rstrip(b'\n')==payload.encode() and decode(p.stdout)[0]==value

LEGACY_PREFIX='qoda-legacy-credential-v1:'
def load_legacy():
 p=subprocess.run(['/usr/bin/secret-tool','lookup','application','qoda-plex-legacy-validation'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10)
 assert p.returncode==0 and p.stdout.startswith(LEGACY_PREFIX.encode())
 v=json.loads(base64.b64decode(p.stdout[len(LEGACY_PREFIX):].rstrip(b'\n'),validate=True))
 assert set(v)=={'client','token'} and v['client'].startswith('qoda-legacy-validation-') and v['token']
 return v

def store_legacy(value):
 payload=LEGACY_PREFIX+base64.b64encode(json.dumps(value,separators=(',',':')).encode()).decode();attrs=['application','qoda-plex-legacy-validation','run',value['client']]
 p=subprocess.run(['/usr/bin/secret-tool','store','--label=Qoda disposable Plex compatibility validation',*attrs],input=payload,text=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=30)
 if p.returncode:return False
 p=subprocess.run(['/usr/bin/secret-tool','lookup',*attrs],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10)
 return p.returncode==0 and p.stdout.rstrip(b'\n')==payload.encode()
