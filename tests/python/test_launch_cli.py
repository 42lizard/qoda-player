"""Both installation adapters cross the real CLI; only systemctl is substituted."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]


class LaunchCLI(unittest.TestCase):
    @contextmanager
    def installation(self, packaged):
        with tempfile.TemporaryDirectory(prefix='qoda-launch-cli-') as temporary:
            base=Path(temporary)
            root=base/'usr/lib/qoda-player'
            release=root/'releases/fixture'
            release.mkdir(parents=True)
            (root/'current').symlink_to('releases/fixture')
            shutil.copy2(ROOT/'tools/launch.py',release/'launch.py')
            if packaged:
                shutil.copy2(ROOT/'packaging/arch/entry.py',root/'entry.py')
            entry=root/'entry.py' if packaged else release/'launch.py'
            runtime=base/'runtime';runtime.mkdir(mode=0o700)
            config=base/'config'
            commands=base/'commands';commands.mkdir()
            log=base/'calls'
            manager=commands/'systemctl'
            manager.write_text('''#!/usr/bin/python3
import os,sys
from pathlib import Path
a=sys.argv[1:]
with open(os.environ['CALL_LOG'],'a') as f: f.write(' '.join(a)+'\\n')
if '--property=FragmentPath' in a:
    print('FragmentPath='+os.environ['UNIT_DIR']+'/'+a[-1]+'\\nDropInPaths='+os.environ.get('DROP_IN',''))
elif '--property=ActiveState' in a: print(os.environ.get('ACTIVE','inactive'))
else: print('LoadState=loaded\\nResult=success\\nExecMainCode=1\\nExecMainStatus='+os.environ.get('EXIT_STATUS','0'))
''')
            manager.chmod(0o755)
            units=base/'usr/lib/systemd/user' if packaged else config/'systemd/user'
            env=dict(os.environ,PATH=str(commands)+':'+os.environ['PATH'],XDG_RUNTIME_DIR=str(runtime),
                     XDG_CONFIG_HOME=str(config),CALL_LOG=str(log),UNIT_DIR=str(units))
            def run(*args):
                return subprocess.run(['python3',str(entry),*args],env=env,capture_output=True,text=True,timeout=5)
            yield base,env,log,run

    def test_both_adapters_status_hide_deadline_and_checkpoint_failure(self):
        for packaged in [False,True]:
            with self.subTest(packaged=packaged), self.installation(packaged) as (_,env,log,run):
                status=run('status');self.assertEqual(status.returncode,0,status.stderr)
                stem='qoda-packaged' if packaged else 'qoda'
                self.assertEqual(set(json.loads(status.stdout)['services']),{stem+'-backend.service',stem+'-ui.service'})
                hidden=run('hide');self.assertEqual(hidden.returncode,0,hidden.stderr)
                expired=run('show','--deadline','0');self.assertNotEqual(expired.returncode,0)
                self.assertIn('expired',expired.stderr)
                env['EXIT_STATUS']='74'
                failed=run('quit');self.assertNotEqual(failed.returncode,0)
                self.assertIn('checkpoint failed',failed.stderr)
                self.assertIn('journalctl --user -u '+stem+'-backend.service',failed.stderr)
                self.assertNotIn('start',log.read_text())

    def test_both_adapters_refuse_overridden_definitions_before_starting(self):
        for packaged in [False,True]:
            with self.subTest(packaged=packaged), self.installation(packaged) as (base,env,log,run):
                for changed in ['fragment','drop-in']:
                    original=env['UNIT_DIR']
                    if changed=='fragment': env['UNIT_DIR']=str(base/'foreign')
                    else: env['DROP_IN']=str(base/'override.conf')
                    refused=run('show');self.assertNotEqual(refused.returncode,0)
                    self.assertIn('overridden or unowned',refused.stderr)
                    self.assertNotIn('start',log.read_text())
                    env['UNIT_DIR']=original

    def test_both_adapters_refuse_a_replaced_backend_instance(self):
        for packaged in [False,True]:
            with self.subTest(packaged=packaged), self.installation(packaged) as (base,env,log,run):
                runtime=base/'runtime/qoda';runtime.mkdir(mode=0o700)
                with socket.socket(socket.AF_UNIX) as listener:
                    listener.bind(str(runtime/'backend.sock'));listener.listen();listener.settimeout(5)
                    def backend():
                        with listener.accept()[0] as peer:
                            peer.settimeout(2)
                            peer.recv(1024)
                            peer.sendall(b'{"protocol":1,"instance":"replacement"}\n')
                    thread=threading.Thread(target=backend,daemon=True);thread.start()
                    env['ACTIVE']='active'
                    refused=run('show','--expected-instance','old')
                    thread.join(timeout=5)
                    self.assertFalse(thread.is_alive())
                self.assertNotEqual(refused.returncode,0)
                self.assertIn('instance changed',refused.stderr)
                self.assertNotIn('start',log.read_text())
