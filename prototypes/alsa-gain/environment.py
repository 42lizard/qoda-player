#!/usr/bin/env python3
"""Record public package/device context, never mixer values or microphone data."""
import json,pathlib,subprocess
HERE=pathlib.Path(__file__).resolve().parent
v={}
for key,cmd in [('kernel',['uname','-r']),('packages',['pacman','-Q','mpv','ffmpeg','alsa-lib','alsa-utils','pipewire','wireplumber','rust']),('client_api',['pkg-config','--modversion','mpv']),('desktop_sinks_after',['pactl','list','short','sinks'])]:
 p=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True);v[key]=p.stdout.strip()
v['card_id']=pathlib.Path('/proc/asound/card1/id').read_text().strip();v['physical_path']=str(pathlib.Path('/sys/class/sound/card1/device').resolve());v['scope']='No microphone capture; no mixer/service/config write; per-child LD_PRELOAD only'
(HERE/'evidence/environment.json').write_text(json.dumps(v,indent=2)+'\n')
