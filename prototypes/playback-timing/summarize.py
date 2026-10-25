#!/usr/bin/env python3
"""Reduce measurements to reviewable facts; never label submitted bytes as heard."""
import json
import gzip
from pathlib import Path
import wave
HERE=Path(__file__).resolve().parent

def audio(path):
 with wave.open(str(path),'rb') as w:return w.readframes(w.getnframes())

def main():
 roots=json.loads((HERE.parents[1]/'.scratch/timing-roots.json').read_text());clock=[];edits=[]
 for filename,root in roots.items():
  path=HERE/'evidence'/filename
  source=json.loads(gzip.decompress(path.with_name(path.name+'.gz').read_bytes())) if path.with_name(path.name+'.gz').exists() else json.loads(path.read_text())
  for c in source['cases']:
   item={'mode':c['mode'],'case':c['name'],'artifact':filename,'timeout':c.get('timeout'),'naturalEnds':sum(e['event']==7 and e.get('reason')==0 and e.get('error')==0 for e in c['events']),
    'negativeClockSamples':sum(r['audio_pts'] is not None and r['audio_pts']<0 for r in c['clocks']),
    'entriesWithPositiveClock':sorted(set(r['entry'] for r in c['clocks'] if r['entry'] is not None and r['coherent'] and r['audio_pts'] is not None and r['audio_pts']>0)),
    'entriesStarted':len([e for e in c['events'] if e['event']==6])}
   if c['mode']=='alsa':
    folder=Path(root)/('alsa-'+c['name']);raw=b''.join(p.read_bytes() for p in sorted(folder.glob('stream-*.pcm')))
    item['acceptedFrames']=len(raw)//4
    item['negotiated']=[{k:e[k] for k in ('pcm_type','format','rate','channels','valid_bits','access')} for e in c['tap'] if e['event']=='negotiated']
    if filename=='clock-traces.json':
     expected=b''.join(audio(folder/(letter+'.wav')) for letter in 'ABC')
     item['fullSubmittedPayloadExact']=raw==expected
    elif c['name']=='early':item['submittedAThenCExact']=raw==audio(folder/'A.wav')+audio(folder/'C.wav')
    else:
     drops=[e for e in c['tap'] if e['event']=='drop' and e['result']==0 and e.get('accepted_total',0)>0]
     if drops:
      drop=drops[0];offset=drop['accepted_total'];after=raw[offset*4:]
      if c['name'] in ('buffered-remove','accepted-remove'):
       barrier=c['barrier'];position=barrier.get('recovery_position',barrier['last_confirmed']['position']);start=round(position*48000)
       expected=audio(folder/'A.wav')[start*4:]+audio(folder/'C.wav')
       actual_start=round(c['lengths'][0]*48000)-(len(after)//4-len(audio(folder/'C.wav'))//4)
       actual_expected=audio(folder/'A.wav')[actual_start*4:]+audio(folder/'C.wav')
       item.update(observedRecoverySourceFrame=actual_start,requestedRoundedRecoveryFrame=start,recoveryQuantizationFrames=actual_start-start,postDropPayloadExactAtObservedSourceFrame=after==actual_expected)
       previous=[e for e in c['tap'] if e['event']=='delay' and e['mono_ns']<=barrier['mono_ns']]
       state=previous[-1] if previous else None
       item.update(recoveryPositionMs=round(position*1000,3),removedBBeforeNominalCurrent=barrier['clock']['audio_pts']<0,
        acceptedBFramesBeforeBarrier=max(0,offset-round(c['lengths'][0]*48000)),
        reportedPresentedCursorBeforeBarrier=state['accepted_total']-state['delay'] if state else None,
        previousAEndFrame=round(c['lengths'][0]*48000),flushLatencyMs=(drop['mono_ns']-barrier['mono_ns'])/1e6)
      elif c['name']=='late-remove':expected=audio(folder/'C.wav');item['confirmedBPositionMs']=round(c['barrier']['clock']['audio_pts']*1000,3)
      elif c['name']=='seek':expected=audio(folder/'A.wav')[48000*4:]+audio(folder/'B.wav')+audio(folder/'C.wav')
      else:expected=None
      item.update(postFirstDropFrames=len(after)//4,expectedPostDropFrames=len(expected)//4 if expected is not None else None,postDropPayloadExact=after==expected if expected is not None else None)
   (clock if filename=='clock-traces.json' else edits).append(item)
 summary={'boundary':'Nominal mpv output-delay clock plus scoped ALSA status/accepted-write observations; no physical audibility/DAC claim',
  'clockCases':clock,'editExecutions':edits,'findings':[
   'Engine playlist/file events can lead nominal presentation; negative audio-pts is useful evidence, not a value to clamp to zero.',
   'Positive public clock witnesses can be absent for an entire tiny-file chain. Exact predecessor spans can map a negative witness, but cannot create a missing clock.',
   'One native short-start attempt stalled after accepting 4800 frames and was bounded by the observer timeout; this is not a successful continuity case.',
   'The early-edit observer originally expected three ends after removing one of three files; its wait timed out after two normal ends. Corrected early runs are retained separately.',
   'Accepted future B frames can be discarded before the nominal boundary; the post-drop payload comparison is distinct from the old accepted stream.',
   'An earlier rebuild started one sample before the rounded checkpoint. Its strict requested-offset comparison remains false; the exact suffix at the observed source offset is retained separately, not called sample-exact seeking.',
   'All reports in the portable model are mock semantics, not live Plex requests or verified scrobbles.'
  ]}
 (HERE/'evidence/summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 print(json.dumps({'clockCases':len(clock),'editExecutions':len(edits),'nativePayloadComparisons':[{'case':r['case'],'exact':r.get('postDropPayloadExact',r.get('submittedAThenCExact'))} for r in edits if r['mode']=='alsa']}))
if __name__=='__main__':main()
