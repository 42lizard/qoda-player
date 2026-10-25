#!/usr/bin/env python3
"""Preserve full captures in gzip, with a readable sampled companion."""
import gzip
import json
from pathlib import Path
HERE=Path(__file__).resolve().parent
for name in ['clock-traces.json','edit-traces.json','corrected-edits.json','initial-device-busy.json']:
 path=HERE/'evidence'/name
 if not path.exists():continue
 full=path.with_name(name+'.gz')
 raw=gzip.decompress(full.read_bytes()) if full.exists() else path.read_bytes()
 with gzip.GzipFile(str(full),'wb',mtime=0) as f:f.write(raw)
 source=json.loads(raw)
 for case in source['cases']:
  for key in ('clocks','tap'):
   rows=case.get(key,[]);keep=[];last=-1;last_entry=None
   for i,row in enumerate(rows):
    t=row.get('mono_ns',0);entry=row.get('entry');barrier=case.get('barrier',{}).get('mono_ns',-10**18)
    important=(key=='tap' and row.get('event')!='delay') or (key=='clocks' and entry!=last_entry)
    if important or i in (0,len(rows)-1) or t-last>=25_000_000 or abs(t-barrier)<20_000_000:keep.append(row);last=t
    last_entry=entry
   case[key]=keep;case[key+'_full_count']=len(rows)
 source.update(full_capture=name+'.gz',readable_sampling='25ms plus transitions, all non-delay tap events and barrier-nearby samples; complete gzip is authoritative')
 path.write_text(json.dumps(source,indent=2)+'\n')
