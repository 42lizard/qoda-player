#!/usr/bin/env python3
"""Classify recorded observations by their intended oracle, without playback."""
import json,pathlib
HERE=pathlib.Path(__file__).resolve().parent;E=HERE/'evidence'
def read(name):return json.loads((E/name).read_text())
def main():
 core=read('results.json')
 if (E/'edge-boundary.json').exists():
  extra_case=read('edge-boundary.json')['case']
  if not any(c['name']==extra_case['name'] for c in core['cases']):core['cases'].append(extra_case)
 extra=read('followups.json');confirm=read('confirmation.json');checks=[]
 def ck(name,ok,scope):checks.append({'name':name,'pass':bool(ok),'scope':scope})
 for c in core['cases']:
  if c['name'].startswith('unsupported'):
   ck(c['name'],c.get('blocked_without_pcm') and not c['actual_samples'],'expected unsupported-input rejection, not a decoded-output comparison')
  else:
   params=[e for s in c['streams'] for e in s['events'] if e['event']=='negotiated']
   ok=c['same_count'] and c['max_error_lsb'] is not None and c['max_error_lsb']<=1 and params and all(e['rate']==48000 and e['format']=='S16_LE' and e['channels']==2 and e['pcm_type']==0 for e in params) and not c['worker'].get('timeout') and not c['worker'].get('harness_error')
   ck(c['name'],ok,'native accepted PCM, selected profile')
 replacements={}
 if not extra.get('positive_control_expected') and (E/'guard-final.json').exists():replacements['volume-and-late-intent']=read('guard-final.json')['case']
 for original in extra['cases']:
  c=replacements.get(original['name'],original);n=c['name']
  if c.get('restored_existing_capture'):
   c['worker_exit']=None;c['stderr']=None;c['restoration_note']='Capture/event files reused; original exit/stderr were not separately retained. No new playback for this analysis.'
  if n in ['positive-gain','loop-file-gain']:ck(n,c['same_count'] and c['max_error_lsb']<=1,'native PCM; original restored-process exit is not inferred')
  if n=='seek-gain':
   c['global_comparison_applicable']=False;c['native_quantized_error_lsb']=None;epochs=c.get('seek_epochs',{});ck(n,epochs and all(epochs[x]['same_count'] and epochs[x]['max_error_lsb']<=1 for x in ['before','after']),'separate drop-delimited epochs; discarded buffers not claimed as heard')
  if n=='accepted-remove-prefetch':c['classification']='nonqualifying short fixture: edit issued after engine advance; confirmation.json targets the early state'
  if n=='busy-before-playback':ck(n,c.get('blocked_without_pcm') and c['worker']['preflight'][0].get('error')==-16,'own held handle; no other client stopped')
  if n=='volume-and-late-intent':
   r=c['responses'];denied=[x for x in r if x.get('mpris_value') in ['0.5','0.0','2.0']];one=next(x for x in r if x.get('mpris_value')=='1.0');readback=next(x for x in r if 'mpris_reported' in x);stale=next(x for x in r if 'stale_completion' in x);app=next(x for x in r if 'app_volume' in x);valid=next((x for x in r if 'valid_prepare' in x),{})
   ck('app/MPRIS unity volume',not app['app_volume']['accepted'] and len(denied)==3 and all(x['exit']!=0 and x['unsupported_error'] for x in denied) and one['exit']==0 and readback['mpris_reported']=='d 1' and readback['native_after']['volume']==100,'real DBus, disposable owner-thread guard; production Rust integration separate')
   ck('newer pause beats stale completion',valid.get('valid_prepare',{}).get('accepted') and stale['stale_completion']['stale'] and not stale['stale_completion']['accepted'] and stale['after_pause']['pause'] and not stale['after_pause']['desired_playing'],'positive current-generation completion and rejected stale completion')
  if n in ['mode-switch-pauses','missing-new-device','injected-device-loss']:
   ok=c['worker'].get('session_restore',{}).get('paused') and c['process_exit']==0 and c['prefix_comparison']['max_error_lsb']<=1 and all(r['route']=='hw:CARD=avsmax98927,DEV=1' for r in c['routes'])
   if n=='missing-new-device':ok=ok and c['worker'].get('new_device_preflight',{}).get('error')==-19
   if n=='injected-device-loss':ok=ok and c['worker'].get('stop_reason')=='injected-device-loss'
   ck(n,ok,'pause/release; ENODEV injection is not physical unplug; Desktop output not exercised')
 ck('early accepted upcoming removal',confirm['case']['exact'],'long current source; removed B and accepted A then C')
 ck('hardware mixer unchanged',confirm['mixer_unchanged'],'targeted before/after run only; no mixer values exported')
 ck('no XRUN before final submission in confirmation',not confirm['xruns_before_all_frames_submitted'],'post-content XRUN retained; no downstream continuity claim')
 extra['analysis_corrections']=['Restored child exit/stderr not guessed','Seek uses drop-delimited epochs','Short edit fixture did not reach intended state and is retained'];(E/'followups.json').write_text(json.dumps(extra,indent=2)+'\n')
 if (E/'source-guard.json').exists():
  for c in read('source-guard.json')['cases']:
   rejects='rejects' in c['name']
   ok=c.get('blocked_without_pcm') and not c['routes'] if rejects else c['exact'] and bool(c['worker'].get('source_formats'))
   ck(c['name'],ok,'authoritative local source descriptor checked before ALSA open; supplied hints are not trusted')
 review={'profile':{'device':'hw:CARD=avsmax98927,DEV=1','rate':48000,'channels':2,'format':'S16_LE','valid_bits':16,'pcm_type':'HW','access':'RW_INTERLEAVED','boundary':'PCM frames accepted by real snd_pcm_writei on selected hw PCM','qualified_transfer':'writei','unqualified_transfers':['writen','mmap']},'checks':checks,'qualified_scope':'prototype digital handoff; deterministic low-level WAV/FLAC and measured gain/transition conditions','not_qualified':['speaker/DSP/transport/DAC/analog fidelity','44.1/96k and 24-bit sources on this device','other devices, formats, drivers or package versions','physical hot-unplug and Desktop route playback on mode switch','production Rust application/MPRIS integration','current/history/Plex reporting from early decoder events','unconditional real-time network/device gaplessness'],'warnings':{'post_content_xrun_observed':True,'short_fixture_edit_not_qualifying':True,'initial_tap_loader_abort_before_pcm':True,'repeated_tag_analysis_corrected':True}}
 (E/'review.json').write_text(json.dumps(review,indent=2)+'\n');print(json.dumps({'checks':len(checks),'passed':sum(x['pass'] for x in checks),'failed':[x['name'] for x in checks if not x['pass']]}))
if __name__=='__main__':main()
