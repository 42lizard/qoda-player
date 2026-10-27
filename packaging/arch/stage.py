#!/usr/bin/env python3
"""Stage only package-owned files; never inspect user profiles or run services."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def stage(source, destination, version):
    spec = importlib.util.spec_from_file_location('qoda_installer', source / 'tools/install.py')
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    prefix = Path('/usr')
    release = destination / 'usr/lib/qoda-player/releases' / version
    release.mkdir(parents=True)
    target = Path(os.environ['CARGO_TARGET_DIR'])
    shutil.copy2(target / 'release/qoda-backend', release / 'qoda-backend')
    shutil.copytree(source / 'ui', release / 'ui')
    for name in ['launch.py', 'release_state.py']:
        shutil.copy2(source / 'tools' / name, release / name)
    commit = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    (release / 'release.json').write_text(json.dumps({'version': version, 'commit': commit, 'protocol': 1, 'storage_compatibility': installer.storage_evidence()}, indent=2) + '\n')
    (release.parent.parent / 'current').symlink_to('releases/' + version)
    shutil.copy2(source / 'packaging/arch/entry.py', release.parent.parent / 'entry.py')
    integrations = installer.integrations(prefix, packaged=True)
    for name, (content, mode) in integrations['prefix'].items():
        path = destination / 'usr' / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(mode)
    for name, (content, mode) in integrations['config'].items():
        path = destination / 'usr/lib' / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(mode)
    for name, relative in [('LICENSE', 'usr/share/licenses/qoda-player/LICENSE'), ('README.md', 'usr/share/doc/qoda-player/README.md')]:
        path = destination / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / name, path)
    shutil.copytree(source / 'docs', destination / 'usr/share/doc/qoda-player/docs')
    hook = destination / 'usr/share/libalpm/hooks/qoda-player-stopped.hook'
    hook.parent.mkdir(parents=True, exist_ok=True)
    hook.write_text('[Trigger]\nOperation = Upgrade\nOperation = Remove\nType = Package\nTarget = qoda-player\n\n[Action]\nDescription = Require packaged Qoda to be stopped\nWhen = PreTransaction\nExec = /usr/bin/python3 /usr/lib/qoda-player/guard.py\nAbortOnFail\n')
    shutil.copy2(source / 'packaging/arch/guard.py', release.parent.parent / 'guard.py')


if __name__ == '__main__':
    stage(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), sys.argv[3])
