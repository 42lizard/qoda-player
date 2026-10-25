#!/usr/bin/env python3
"""Additional bounded acceptance using real private units, files and SQLite."""
import json
from contextlib import closing
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
from qoda import atomic
from study import Study,wait,HERE,EVIDENCE,BINARY,digest
from package_study import OwnedFiles,backup,require_rollback_compatibility,restore_backup,purge_fixture_credentials


def main():
    s=Study();error=None;retained=[];keyring=None
    print('Extended study root: '+str(s.root),flush=True)
    try:
        s.setup();c=s.controller();root=s.root
        missing=subprocess.run([sys.executable,str(HERE/'qoda.py'),'--root',str(root),'show'],env=dict(os.environ,PATH='/qoda-no-dependencies'),capture_output=True,text=True,timeout=4)
        s.check('missing dependencies fail before starting either service',missing.returncode!=0 and 'missing-dependencies:systemctl,quickshell' in missing.stderr and not c.active('backend') and not c.active('ui'))
        keyring=subprocess.Popen([sys.executable,str(HERE/'keyring_fixture.py'),'--address',s.config['serviceEnv']['DBUS_SESSION_BUS_ADDRESS']],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        wait(lambda:c.keyring_state()=='locked')
        s.check('private locked Secret Service availability is reported',json.loads(s.qoda('status').stdout)['keyring']=='locked')
        s.qoda('show');s.check('locked credential store does not block fictional session UI',s.ui()['compatible'] and not s.state()['playing'])
        wait(lambda:len(s.ui()['mediaPlayers'])==2)
        names={p['name'] for p in s.ui()['mediaPlayers']}
        s.check('stock Quickshell MPRIS client discovers two independent players',names=={'org.mpris.MediaPlayer2.qoda','org.mpris.MediaPlayer2.fixture_peer'})
        c.ui('media','play','org.mpris.MediaPlayer2.fixture_peer')
        wait(lambda:'Playing' in s.buscall('get-property','org.mpris.MediaPlayer2.fixture_peer','/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','PlaybackStatus').stdout)
        s.check('selected peer media command leaves Qoda paused',not s.state()['playing'])
        c.ui('media','play','org.mpris.MediaPlayer2.qoda');wait(lambda:s.state()['playing'])
        c.ui('media','pause','org.mpris.MediaPlayer2.fixture_peer')
        wait(lambda:'Paused' in s.buscall('get-property','org.mpris.MediaPlayer2.fixture_peer','/org/mpris/MediaPlayer2','org.mpris.MediaPlayer2.Player','PlaybackStatus').stdout)
        s.check('selected Qoda media command preserves independent peer state',s.state()['playing'])
        c.ui('media','pause','org.mpris.MediaPlayer2.qoda');wait(lambda:not s.state()['playing'])
        # Start from an actual installed desktop entry without installing it in the real menu.
        s.qoda('hide')
        launch=subprocess.run(['gio','launch',str(root/'prefix/share/applications/qoda-player.desktop')],capture_output=True,text=True,timeout=6)
        wait(lambda:s.ui()['form']=='full')
        s.check('isolated desktop file launches canonical command',launch.returncode==0 and s.ui()['form']=='full')
        s.check('installed icon is discoverable by stock Quickshell',s.ui()['iconAvailable'])
        (root/'runtime/quitting').touch()
        blocked=False
        try:s.rpc('play')
        except RuntimeError as e:blocked=str(e)=='shutdown-in-progress'
        (root/'runtime/quitting').unlink()
        s.check('Rust rejects new playback commands while shutdown is pending',blocked and not s.state()['playing'])
        s.rpc('append',trackId='fixture:track:extended-1');s.rpc('play');wait(lambda:s.state()['playing'])
        instance=s.state()['instance'];position=s.state()['position'];before=s.ui()
        qml=root/'prefix/lib/qoda-player/releases/0.1.0-study/shell.qml';original=qml.read_text()
        qml.write_text(original+'\n// deliberate reload fixture\n')
        wait(lambda:s.ui()['loadedAt']!=before['loadedAt'] and s.ui()['connected'])
        s.check('real QML file reload preserves Rust epoch and null playback',s.ui()['pid']==before['pid'] and s.state()['instance']==instance and s.state()['playing'] and s.state()['position']>=position)
        qml.write_text(original);wait(lambda:s.ui()['compatible'])
        # Mismatch is a real different QML release; its public presentation method refuses activation.
        s.ctl('stop',c.unit('ui'))
        qml.write_text(original.replace('property string release: "0.1.0-study"','property string release: "wrong-ui-release"'))
        started=time.monotonic();r=s.qoda('show',check=False);elapsed=time.monotonic()-started
        s.check('UI/backend release mismatch fails explicitly without stopping audio',r.returncode!=0 and 'ui-backend-version-mismatch' in r.stderr and elapsed<5 and s.state()['instance']==instance and s.state()['playing'],{'seconds':elapsed})
        refused=json.loads(c.ui('present','show'))
        s.check('incompatible UI refuses presentation command',not refused['compatible'] and refused['form']=='hidden')
        s.ctl('stop',c.unit('ui'));qml.write_text(original);s.qoda('show')
        s.rpc('pause');wait(lambda:not s.state()['playing']);s.rpc('checkpoint');s.qoda('quit')
        # Relocation is measured by executing the binary and QML from another release path.
        moved=root/'prefix/lib/qoda-player/releases/relocated release'
        shutil.copytree(root/'prefix/lib/qoda-player/releases/0.1.0-study',moved)
        text=(moved/'shell.qml').read_text().replace('property string release: "0.1.0-study"','property string release: "relocated release"');(moved/'shell.qml').write_text(text)
        meta=json.loads((moved/'release.json').read_text());meta['version']='relocated release';(moved/'release.json').write_text(json.dumps(meta))
        for path in moved.iterdir():s.manifest[str(path)]=digest(path)
        s.switch('relocated release');s.qoda('show')
        s.check('release relocation including spaces executes without source paths',s.state()['release']==s.ui()['release']=='relocated release' and len(s.state()['queue'])==1 and not s.state()['playing'])
        s.qoda('quit');s.switch('0.1.0-study')
        # An actual failed new UI, followed by explicit rollback of code only.
        s.qoda('show');broken=s.release('broken-study')
        (broken/'shell.qml').write_text('import QtQuick\nMissingStudyType {}\n')
        s.manifest[str(broken/'shell.qml')]=digest(broken/'shell.qml')
        failed=False
        try:s.qualified_update('broken-study')
        except subprocess.CalledProcessError as e:
            failed=True;atomic(root/'state/lifecycle-error.json',{'action':'update','error':'new-ui-activation-failed','recoverableRelease':'0.1.0-study'})
        s.check('actual new-release activation failure remains visible in status',failed and json.loads(s.qoda('status').stdout)['lifecycleError']['recoverableRelease']=='0.1.0-study' and s.props('ui')['ActiveState']=='failed')
        s.qoda('quit');s.switch('0.1.0-study');s.qoda('show')
        s.check('explicit rollback after failed activation keeps UI backend and data consistent',s.ui()['release']==s.state()['release']=='0.1.0-study' and len(s.state()['queue'])==1 and not s.state()['playing'])
        s.qoda('quit')
        # Older code must refuse a newer schema and allow an explicit compatible backup recovery.
        database=root/'data/qoda/session.sqlite3';backups=root/'data/qoda/backups';backups.mkdir()
        snapshot=backups/'pre-migration.sqlite3';backup(database,snapshot)
        with closing(sqlite3.connect(database)) as db,db:
            body=json.loads(db.execute('SELECT body FROM session WHERE id=1').fetchone()[0]);body['queue'].append({'entryId':'fixture:post-backup','trackId':'fixture:new'})
            db.execute('UPDATE session SET body=? WHERE id=1',(json.dumps(body),));db.execute('PRAGMA user_version=2')
        old_release=root/'prefix/lib/qoda-player/releases/0.1.0-study';before_pointer=os.readlink(root/'prefix/lib/qoda-player/current')
        refused=False
        try:require_rollback_compatibility(database,old_release)
        except RuntimeError as e:
            refused=str(e)=='rollback-incompatible-schema';atomic(root/'state/lifecycle-error.json',{'action':'rollback','error':str(e)})
        with closing(sqlite3.connect(database)) as db,db:after_queue=json.loads(db.execute('SELECT body FROM session WHERE id=1').fetchone()[0])['queue']
        s.check('incompatible rollback refuses without replacing pointer or data',refused and os.readlink(root/'prefix/lib/qoda-player/current')==before_pointer and len(after_queue)==2 and json.loads(s.qoda('status').stdout)['lifecycleError']['error']=='rollback-incompatible-schema')
        report=restore_backup(database,snapshot,old_release)
        s.qoda('show')
        s.check('explicit backup recovery restores compatible session paused',len(s.state()['queue'])==1 and not s.state()['playing'] and report['restoredSchema']==1)
        s.check('backup recovery reports exact discarded occurrence',report['lostOccurrences']==['fixture:post-backup'] and report['beforeCount']==2 and report['afterCount']==1,report)
        # Deliberate command timeout, then restore last successful checkpoint.
        s.rpc('checkpoint');pid=int(s.props('backend')['MainPID']);os.kill(pid,signal.SIGSTOP)
        started=time.monotonic();r=s.qoda('quit',check=False);elapsed=time.monotonic()-started
        s.check('unresponsive backend Quit is bounded and exposes persistence timeout',r.returncode!=0 and elapsed<10 and not c.active('backend') and not c.active('ui') and 'timed out' in r.stderr,{'seconds':elapsed})
        s.qoda('show');s.check('timeout recovery retains last checkpoint without autoplay',len(s.state()['queue'])==1 and not s.state()['playing'])
        s.qoda('quit')
        # Backend deliberately remains pre-ready longer than the public readiness limit.
        s.config['serviceEnv']['QODA_STUDY_DELAY']='12000';atomic(root/'study.json',s.config)
        started=time.monotonic();r=s.qoda('show',check=False);elapsed=time.monotonic()-started
        s.check('startup readiness expires with actionable error',r.returncode!=0 and 'readiness-timeout' in r.stderr and 9.5<=elapsed<12,{'seconds':elapsed})
        s.qoda('quit');s.config['serviceEnv'].pop('QODA_STUDY_DELAY');atomic(root/'study.json',s.config)
        s.check('timed-out startup cancels without service resurrection',not c.active('backend') and not c.active('ui'))
        # Real ownership manifest: existing unowned and modified integration files survive removal.
        prefix=root/'prefix'
        parent=prefix/'share/owned-parent';parent.mkdir();(parent/'sentinel').write_text('owned-fixture')
        owned=OwnedFiles(prefix,[p for p in prefix.rglob('*') if p.is_file() or p.is_symlink()])
        (parent/'sentinel').unlink();parent.rmdir()
        outside_parent=root/'foreign-container';outside_parent.mkdir();(outside_parent/'sentinel').write_text('outside-fixture')
        parent.symlink_to(outside_parent)
        original_icon=prefix/'share/icons/hicolor/scalable/apps/qoda-player.svg';icon_bytes=original_icon.read_bytes();original_icon.write_text('user-modified-fixture')
        foreign=prefix/'share/applications/other-player.desktop';foreign.write_text('unrelated-fixture')
        collision=False
        try:s.own(foreign,'must-not-overwrite')
        except RuntimeError:collision=True
        s.check('installation ownership conflict preserves preexisting bytes',collision and foreign.read_text()=='unrelated-fixture')
        unit_root=Path(os.environ['XDG_RUNTIME_DIR'])/'systemd/user';runtime_owned=OwnedFiles(unit_root,s.units)
        malicious=root/'outside-owned-root';malicious.write_text('must-stay')
        owned.entries.append({'path':'../outside-owned-root','kind':'file','sha256':digest(malicious)})
        config_dir=root/'config/systemd/user'
        config_owned=OwnedFiles(config_dir,list(config_dir.glob('*.service')))
        config_result=config_owned.remove()
        result=owned.remove();runtime_result=runtime_owned.remove();s.ctl('daemon-reload')
        s.check('uninstall removes manifest-owned release and runtime integrations',not (prefix/'bin/qoda').exists() and not (prefix/'lib/qoda-player/current').is_symlink() and all(not p.exists() for p in s.units) and not runtime_result['retained'] and not config_result['retained'] and not list(config_dir.glob('*.service')))
        s.check('uninstall preserves unowned changed and out-of-root files',foreign.read_text()=='unrelated-fixture' and original_icon.read_text()=='user-modified-fixture' and malicious.read_text()=='must-stay' and len(result['retained'])==3 and (outside_parent/'sentinel').read_text()=='outside-fixture')
        s.check('default uninstall retains session and account-bearing backup',database.exists() and snapshot.exists())
        # Credentials are fake private-bus items found by scoped attributes.
        locked_purge=purge_fixture_credentials(s.config['serviceEnv']['DBUS_SESSION_BUS_ADDRESS'],root/'runtime')
        s.check('locked credential purge reports incomplete cleanup without unlocking',locked_purge['incomplete'] and locked_purge['deleted']==0 and c.keyring_state()=='locked')
        s.buscall('call','org.freedesktop.secrets','/org/freedesktop/secrets','org.freedesktop.Secret.Service','Unlock','ao','1','/org/freedesktop/secrets/collection/fixture/qoda')
        complete_purge=purge_fixture_credentials(s.config['serviceEnv']['DBUS_SESSION_BUS_ADDRESS'],root/'runtime')
        other=s.buscall('get-property','org.freedesktop.secrets','/org/freedesktop/secrets/collection/fixture/other','org.freedesktop.Secret.Item','Label')
        s.check('explicit unlocked purge deletes only scoped private credential item',not complete_purge['incomplete'] and complete_purge['deleted']==1 and 'Other fixture' in other.stdout)
        s.check('credential purge can be retried after completion',purge_fixture_credentials(s.config['serviceEnv']['DBUS_SESSION_BUS_ADDRESS'],root/'runtime')['deleted']==0)
        # Purge synthetic durable data including account-bearing backups.
        report={'retainedModifiedFiles':result['retained'],'retainedData':['session.sqlite3','backups/pre-migration.sqlite3'],'prerequisitesRemain':['quickshell','libmpv','systemd'],'keyring':complete_purge,'priorLockedPurge':locked_purge}
        atomic(root/'uninstall-report.json',report)
        s.check('remaining dependency data and conflict report is available',len(report['retainedModifiedFiles'])==3 and report['prerequisitesRemain']==['quickshell','libmpv','systemd'])
        shutil.rmtree(root/'data/qoda')
        s.check('explicit fixture data purge removes session and account-bearing backups',not database.exists() and not snapshot.exists() and foreign.exists())
        # Expected modified file is retained, then remove it only as this study's synthetic content.
        original_icon.unlink();parent.unlink();s.save_manifest()
    except BaseException as e:
        error={'type':type(e).__name__,'message':str(e),'stderr':getattr(e,'stderr',None)};print('ERROR '+str(error),flush=True)
    finally:
        if keyring:
            keyring.terminate();keyring.wait(timeout=4)
        try:retained=s.cleanup()
        except Exception as e:retained=['cleanup-error:'+str(e)]
        result=s.redact({'scope':'extended','checks':s.results,'error':error,'retainedOwnedFiles':retained,'binarySha256':digest(BINARY)})
        atomic(EVIDENCE/('extended-'+str(time.time_ns())+'.json'),result);atomic(EVIDENCE/'extended.json',result)
        print(json.dumps({'passed':sum(c['pass'] for c in s.results),'total':len(s.results),'error':error,'retained':retained}),flush=True)
    return 1 if error or retained or any(not c['pass'] for c in s.results) else 0

if __name__=='__main__':sys.exit(main())
