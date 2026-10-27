#!/usr/bin/env python3
"""Regenerate user-guide images from production QML and fictional local fixtures."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parent.parent
OUTPUT=ROOT/'docs/assets/user-guide'


def main():
    spec=importlib.util.spec_from_file_location('qoda_layout',ROOT/'tools/check-layout.py')
    layout=importlib.util.module_from_spec(spec);spec.loader.exec_module(layout)
    OUTPUT.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='qoda-guide-images-') as temporary:
        base=Path(temporary)
        layout.check(base/'captures', guide=True)
        for source,name in [('wide','main-window'),('queue-guide','queue-drawer'),('account-guide','account'),('sign-in-small','sign-in'),('library-choices-small','libraries'),('playlist-drag-wide','playlists'),('hq-conversion-small','audio-settings'),('cache-small','cache'),('archive-small','listening-archive')]:
            shutil.copyfile(base/'captures'/f'{source}.png',OUTPUT/f'{name}.png')
        shutil.copytree(ROOT/'ui',base/'ui')
        snapshot=layout.fixture()
        qml='''import QtQuick
import QtQuick.Window
import "ui" as Qoda
Window {
    width: 444; height: 350; visible: true; color: Qoda.Theme.background
    Rectangle { anchors.fill: parent; color: Qoda.Theme.background }
    Qoda.CompactControls {
        id: controls; anchors.fill: parent; anchors.margins: 16
        connected: true
        snapshot: SNAPSHOT
        selectedTrack: snapshot.selection
    }
    Timer { interval: 750; running: true; onTriggered: controls.Window.window.contentItem.grabToImage(result => { Qt.exit(result.saveToFile(OUTPUT) ? 0 : 1); }); }
}
'''.replace('SNAPSHOT',json.dumps(snapshot)).replace('OUTPUT',json.dumps(str(OUTPUT/'compact-controls.png')))
        (base/'compact.qml').write_text(qml)
        env=dict(os.environ,QT_QPA_PLATFORM='offscreen',QT_QUICK_BACKEND='software',XDG_CONFIG_HOME=str(base/'config'),XDG_RUNTIME_DIR=str(base/'runtime'))
        Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
        subprocess.run(['/usr/lib/qt6/bin/qml',str(base/'compact.qml')],env=env,check=True,timeout=15)
    print('Guide screenshots use fictional fixtures only; inspect all PNGs before committing.')


if __name__=='__main__':main()
