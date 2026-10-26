#!/usr/bin/env python3
"""Rebuild original synthetic-tone fixtures; optional tool dependency mutagen==1.47.0.
Tests consume checked-in outputs and need no Mutagen installation or network.
"""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import mutagen
from mutagen.id3 import ID3, TXXX
from mutagen.apev2 import APEv2
from mutagen.mp4 import MP4, MP4FreeForm
from mutagen.oggvorbis import OggVorbis
from mutagen.oggopus import OggOpus
from mutagen.wave import WAVE

ROOT = Path(__file__).resolve().parent
TAGS = {'REPLAYGAIN_TRACK_GAIN': '-6.0 dB', 'REPLAYGAIN_TRACK_PEAK': '1.0',
        'REPLAYGAIN_ALBUM_GAIN': '-12.0 dB', 'REPLAYGAIN_ALBUM_PEAK': '1.0'}

def encode(path, codec):
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y',
        '-f', 'lavfi', '-i', 'sine=frequency=1000:sample_rate=48000:duration=0.25',
        '-ac', '2', '-map_metadata', '-1', '-fflags', '+bitexact', '-flags:a', '+bitexact',
        '-c:a', codec, str(path)], check=True)

def main():
    with tempfile.TemporaryDirectory(prefix='qoda-synthetic-tags-') as work:
        work = Path(work)
        base = work/'base.mp3'; encode(base, 'libmp3lame')
        for version in (3, 4):
            dest = ROOT/f'id3v2{version}.mp3'; shutil.copyfile(base, dest)
            tags = ID3()
            for key, value in TAGS.items(): tags.add(TXXX(encoding=3, desc=key, text=[value]))
            tags.save(dest, v2_version=version, padding=lambda _: 0)
        dest = ROOT/'ape.mp3'; shutil.copyfile(base, dest)
        tags = APEv2()
        for key, value in TAGS.items(): tags[key] = value
        tags.save(dest)
        dest = ROOT/'gain.m4a'; encode(dest, 'aac')
        file = MP4(dest)
        if file.tags is None: file.add_tags()
        for key, value in TAGS.items(): file.tags['----:com.apple.iTunes:'+key] = [MP4FreeForm(value.encode())]
        file.save(padding=lambda _: 0)
        for name, codec, kind in [('gain.ogg', 'libvorbis', OggVorbis), ('gain.opus', 'libopus', OggOpus)]:
            dest = ROOT/name; encode(dest, codec)
            if name == 'gain.opus': shutil.copyfile(dest, ROOT/'opus-base.opus')
            file = kind(dest)
            for key, value in TAGS.items(): file.tags[key] = [value]
            file.save(padding=lambda _: 0)
        shutil.copyfile(ROOT/'gain.opus', ROOT/'opus-r128.opus')
        conflict = OggOpus(ROOT/'opus-r128.opus'); conflict.tags['R128_TRACK_GAIN'] = ['0']; conflict.save(padding=lambda _: 0)
        dest = ROOT/'gain.wav'; encode(dest, 'pcm_s16le'); file = WAVE(dest)
        if file.tags is None: file.add_tags()
        for key, value in TAGS.items(): file.tags.add(TXXX(encoding=3, desc=key, text=[value]))
        file.save(padding=lambda _: 0)
    names = ['id3v23.mp3','id3v24.mp3','ape.mp3','gain.m4a','gain.ogg','gain.opus','gain.wav','opus-base.opus','opus-r128.opus']
    manifest = {'source':'Original 0.25-second 1 kHz mathematical tone; no recorded or user-library audio',
        'license':'MIT (repository original synthetic fixtures)',
        'ffmpeg':subprocess.check_output(['ffmpeg','-version'],text=True).splitlines()[0],
        'tag_writer':'Mutagen '+mutagen.version_string,'tags':TAGS,
        'files':{name:{'bytes':(ROOT/name).stat().st_size,'sha256':hashlib.sha256((ROOT/name).read_bytes()).hexdigest()} for name in names}}
    (ROOT/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print(json.dumps(manifest['files'],indent=2))

if __name__ == '__main__': main()
