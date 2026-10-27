"""Package entry CLI contract, using a private runtime and fake systemctl."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]

class PackageEntry(unittest.TestCase):
    def test_status_uses_distinct_packaged_units_without_starting_services(self):
        with tempfile.TemporaryDirectory(prefix='qoda-pacman-cli-') as temporary:
            base=Path(temporary)
            package=base/'usr/lib/qoda-player'
            release=package/'releases/test'
            release.mkdir(parents=True)
            (package/'current').symlink_to('releases/test')
            shutil.copy(ROOT/'packaging/arch/entry.py',package/'entry.py')
            shutil.copy(ROOT/'tools/launch.py',release/'launch.py')
            runtime=base/'runtime';runtime.mkdir(mode=0o700)
            commands=base/'commands';commands.mkdir()
            log=base/'calls'
            manager=commands/'systemctl'
            manager.write_text('#!/usr/bin/python3\nimport sys,os\nfrom pathlib import Path\na=sys.argv[1:]\nwith open(os.environ["CALL_LOG"],"a") as f:f.write(" ".join(a)+"\\n")\nif "--property=FragmentPath" in a: print("FragmentPath="+os.environ["UNIT_DIR"]+"/"+a[-1]+"\\nDropInPaths=")\nelif "--property=ActiveState" in a: print("inactive")\nelse: print("LoadState=loaded\\nResult=success\\nExecMainCode=0\\nExecMainStatus=0")\n')
            manager.chmod(0o755)
            env=dict(os.environ,PATH=str(commands)+':'+os.environ['PATH'],XDG_RUNTIME_DIR=str(runtime),CALL_LOG=str(log),UNIT_DIR=str(base/'usr/lib/systemd/user'))
            result=subprocess.run(['python3',str(package/'entry.py'),'status'],env=env,capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            states=json.loads(result.stdout)['services']
            self.assertEqual(set(states),{'qoda-packaged-backend.service','qoda-packaged-ui.service'})
            self.assertNotIn('start',log.read_text())
            env['UNIT_DIR']=str(base/'overridden')
            refused=subprocess.run(['python3',str(package/'entry.py'),'show'],env=env,capture_output=True,text=True)
            self.assertNotEqual(refused.returncode,0)
            self.assertIn('units are overridden',refused.stderr)
            self.assertNotIn('start',log.read_text())

    def test_transaction_guard_accepts_idle_and_refuses_a_packaged_process(self):
        with tempfile.TemporaryDirectory(prefix='qoda-proc-fixture-') as temporary:
            proc=Path(temporary)
            guard=ROOT/'packaging/arch/guard.py'
            command=['python3',str(guard),'--proc-root',str(proc)]
            self.assertEqual(subprocess.run(command,capture_output=True).returncode,0)
            process=proc/'234567';process.mkdir()
            (process/'exe').symlink_to('/usr/bin/python3')
            (process/'cmdline').write_bytes(b'/usr/bin/python3\0/usr/lib/qoda-player/entry.py\0')
            refused=subprocess.run(command,capture_output=True,text=True)
            self.assertNotEqual(refused.returncode,0)
            self.assertIn('Quit packaged Qoda',refused.stderr)
