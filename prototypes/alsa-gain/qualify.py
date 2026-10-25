#!/usr/bin/env python3
"""One-command bounded qualification; short quiet synthetic speaker audio."""
import datetime,json,pathlib,subprocess,sys
HERE=pathlib.Path(__file__).resolve().parent;E=HERE/'evidence'
attempt={'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'status':'running','stage':'mapping-check'}
def save():(E/'attempt.json').write_text(json.dumps(attempt,indent=2)+'\n')
save()
try:
 if pathlib.Path('/proc/asound/card1/id').read_text().strip()!='avsmax98927':raise RuntimeError('Measurement card mapping changed; select/requalify before playback')
 print('Selected direct hw built-in speakers; short low-level synthetic PCM. No mixer or PipeWire reconfiguration. Busy/unsupported output stops.',flush=True)
 attempt['stage']='preflight';save();subprocess.run([sys.executable,str(HERE/'preflight.py')],check=True,timeout=15)
 matrix=json.loads((E/'preflight.json').read_text())['matrix']
 if not any(x['rate']==48000 and x['format']=='S16_LE' and x['supported'] for x in matrix):raise RuntimeError('Selected native profile unavailable; no playback cases run')
 for script in ['run.py','followups.py','confirmation.py','source_guard.py','environment.py','review.py']:
  attempt['stage']=script;save();subprocess.run([sys.executable,str(HERE/script)],check=True,timeout=120)
 review=json.loads((E/'review.json').read_text())
 if not all(x['pass'] for x in review['checks']):raise RuntimeError('One or more scoped conditions failed; inspect captured evidence')
 attempt.update(status='complete',stage='reviewed',finished_utc=datetime.datetime.now(datetime.timezone.utc).isoformat());save()
 print('Scoped digital handoff qualification complete. Downstream fidelity and presentation-boundary mapping remain separate.',flush=True)
except BaseException as e:
 attempt.update(status='failed',error_class=type(e).__name__,historical_evidence_not_current=True);save();raise
finally:
 if (E/'review.json').exists():subprocess.run([sys.executable,str(HERE/'report.py')],check=True,timeout=10)
