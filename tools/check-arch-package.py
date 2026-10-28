#!/usr/bin/env python3
"""Native pacman transaction and empty-profile backend checks in a disposable root.

Run as root in CI's disposable container, or under tools/check-arch-package.sh.
No package scripts execute. The process guard is tested separately against a
process-filesystem fixture; its hook is masked in this interpreter-free root.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import shlex
import subprocess
import sys
import tempfile
import time


def run(*args):
    result=subprocess.run(list(map(str,args)),text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(result.stdout)
    return result.stdout


def observe_backend(command, env, base, identity=None):
    with (base/'backend.log').open('w') as log:
        options = {} if identity is None else {'user': identity, 'group': identity, 'extra_groups': []}
        child=subprocess.Popen(command,env=env,stdout=log,stderr=log,**options)
        try:
            deadline=time.monotonic()+15
            # Backend IPC accepts only the same UID. Probe from the same
            # identity as the child, including the ordinary-user CI case.
            probe_code = '\n'.join([
                'import socket,sys',
                'with socket.socket(socket.AF_UNIX) as connection:',
                ' connection.settimeout(1)',
                ' connection.connect(sys.argv[1])',
                " connection.sendall(b'{\"op\":\"hello\",\"protocol\":1}\\n')",
                ' with connection.makefile("rb") as stream: sys.stdout.buffer.write(stream.readline(16*1024*1024))',
            ])
            while True:
                assert child.poll() is None,'Packaged backend exited: '+(base/'backend.log').read_text()
                assert time.monotonic()<deadline, 'Packaged backend IPC did not become ready'
                probe=subprocess.run([sys.executable,'-c',probe_code,str(Path(env['XDG_RUNTIME_DIR'])/'qoda/backend.sock')],env=env,capture_output=True,timeout=2,**options)
                if probe.returncode==0 and probe.stdout:
                    state=json.loads(probe.stdout)
                    assert state['intent']=='paused' and not state['sign_in']['connected'] and not state['queue']['entries']
                    break
                time.sleep(.05)
            child.send_signal(signal.SIGINT);assert child.wait(timeout=12)==0
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=3)


def check(archive):
    if os.geteuid() != 0:
        raise SystemExit('Use tools/check-arch-package.sh for an unprivileged sandbox')
    with tempfile.TemporaryDirectory(prefix='qoda-pacman-check-') as temporary:
        base=Path(temporary);root=base/'root';root.mkdir()
        database=root/'var/lib/pacman';database.mkdir(parents=True)
        hooks=base/'hooks';hooks.mkdir();(hooks/'qoda-player-stopped.hook').symlink_to('/dev/null')
        config=base/'pacman.conf';config.write_text('[options]\nSigLevel = Never\n')
        name,version=run('pacman','-Qp',archive).split()
        assert name=='qoda-player'
        listing=run('pacman','-Qlp','-q',archive).splitlines()
        assert all(path.startswith('/usr/') and '..' not in Path(path).parts for path in listing)
        for suffix in ['bin/qoda','share/applications/qoda-player.desktop','lib/systemd/user/qoda-packaged-backend.service','lib/qoda-player/entry.py','share/doc/qoda-player/docs/user-guide.md']:
            assert '/usr/'+suffix in listing,suffix
        metadata=run('bsdtar','-xOf',archive,'.PKGINFO')
        for dependency in ['python','ffmpeg','mpv','quickshell','alsa-lib','sqlite']:
            assert 'depend = '+dependency+'\n' in metadata
        sentinel=root/'home/demo/.local/state/qoda/sentinel';sentinel.parent.mkdir(parents=True);sentinel.write_text('retained user data')
        pacman=['pacman','--root',root,'--dbpath',database,'--logfile',base/'pacman.log','--config',config,'--hookdir',hooks,'--noconfirm','--noscriptlet']
        run(*pacman,'-U','--nodeps','--nodeps',archive)
        release=(root/'usr/lib/qoda-player/current').resolve()
        manifest=json.loads((release/'release.json').read_text())
        assert len(manifest['commit'])==40 and manifest['version']==version.rsplit('-',1)[0]
        assert (release/'ui/shell.qml').is_file()
        for asset in ['I18n.qml', 'i18n/Catalog.js', 'i18n/German.js', 'i18n/de.json']:
            assert (release/'ui'/asset).is_file(), 'Missing localization asset: '+asset
        unit=(root/'usr/lib/systemd/user/qoda-packaged-backend.service').read_text()
        command=next(line.removeprefix('ExecStart=') for line in unit.splitlines() if line.startswith('ExecStart='))
        assert shlex.split(command)==['/usr/bin/python3','/usr/lib/qoda-player/entry.py','run-backend']
        assert '[Install]' not in unit
        # Start the actual packaged backend with an empty private profile. No UI,
        # audio stream, session bus, credential, network source or user data.
        runtime=base/'runtime';runtime.mkdir(mode=0o700)
        env=dict(os.environ, HOME=str(base/'home'), XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'),XDG_CACHE_HOME=str(base/'cache'),XDG_DATA_HOME=str(base/'data'),DBUS_SESSION_BUS_ADDRESS='unix:path='+str(base/'absent-bus'),WAYLAND_DISPLAY='')
        observe_backend(['python3',str(root/'usr/lib/qoda-player/entry.py'),'run-backend'],env,base)
        # A user namespace used for local verification maps only UID 0. CI's
        # container has a full UID map and must exercise root-owned activation
        # as an ordinary user too, without launching the configured helper.
        mapped = any(int(line.split()[2]) > 65534 for line in Path('/proc/self/uid_map').read_text().splitlines())
        if mapped:
            base.chmod(0o755)
            user_env=dict(env)
            for variable in ['XDG_RUNTIME_DIR','XDG_CONFIG_HOME','XDG_STATE_HOME','XDG_CACHE_HOME','XDG_DATA_HOME']:
                directory=base/(variable.lower()+'-user');directory.mkdir(mode=0o700);os.chown(directory,65534,65534);user_env[variable]=str(directory)
            backend=str(release/'qoda-backend')
            unsafe=subprocess.run([backend,'--ui-launcher',str(root/'usr/bin/qoda')],env=user_env,user=65534,group=65534,extra_groups=[],capture_output=True,timeout=5)
            assert unsafe.returncode==78, 'Root-owned launcher below writable /tmp must be refused'
            observe_backend([backend,'--ui-launcher','/usr/bin/true'],user_env,base,identity=65534)
            print('Non-root packaged binary accepted protected root-owned system activation and refused writable ancestors')
        elif os.environ.get('QODA_CI_REQUIRE_NONROOT'):
            raise AssertionError('CI requires a full UID map for non-root system activation verification')
        settings=base/'config/qoda/audio.json'
        settings.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
        settings.write_text('{"version":999}');settings.chmod(0o600)
        refused=subprocess.run(['python3',str(root/'usr/lib/qoda-player/entry.py'),'run-backend'],env=env,capture_output=True,text=True,timeout=5)
        assert refused.returncode==78 and 'Unsupported settings' in refused.stderr
        assert settings.read_text()=='{"version":999}'
        # Exercise real upgrade transactions with a private version-bump fixture
        # and changed documentation payload, retaining the actual package layout.
        upgraded=base/'upgraded';upgraded.mkdir();run('bsdtar','-xf',archive,'-C',upgraded)
        info=upgraded/'.PKGINFO';info.write_text(info.read_text().replace('pkgver = '+version+'\n','pkgver = '+version+'.1\n'))
        (upgraded/'.MTREE').unlink(missing_ok=True)
        readme=upgraded/'usr/share/doc/qoda-player/README.md';readme.write_text(readme.read_text()+'\nSynthetic upgrade fixture\n')
        candidate=base/'upgrade.pkg.tar.zst';run('bsdtar','--zstd','-cf',candidate,'-C',upgraded,'.PKGINFO','.BUILDINFO','usr')
        run(*pacman,'-U','--nodeps','--nodeps',candidate)
        assert 'Synthetic upgrade fixture' in (root/'usr/share/doc/qoda-player/README.md').read_text()
        run(*pacman,'-R','--nodeps','--nodeps','qoda-player')
        assert sentinel.read_text()=='retained user data'
        assert not (root/'usr/bin/qoda').exists()
        print('Package metadata/layout, empty-profile native startup, pacman install/upgrade/remove and retained user data passed')


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('archive',type=Path)
    check(parser.parse_args().archive.resolve(strict=True))
