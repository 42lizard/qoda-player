#!/usr/bin/python3
"""Pacman-managed entry point; reuse the source launcher's process ownership."""
import importlib.util
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def main():
    release = (ROOT / 'current').resolve(strict=True)
    service = module('qoda_launch', release / 'launch.py')
    service.UNITS = ('qoda-packaged-backend.service', 'qoda-packaged-ui.service')
    if sys.argv[1:] == ['run-backend']:
        compatibility = module('qoda_compatibility', release / 'release_state.py')
        compatibility.data_compatible(compatibility.release_evidence(release))
        os.execv(str(release / 'qoda-backend'), [str(release / 'qoda-backend'), '--ui-launcher', str(ROOT.parent.parent / 'bin/qoda')])
    for unit in service.UNITS:
        result = service.manager('show', '--property=FragmentPath', '--property=DropInPaths', unit)
        fields = dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)
        if fields.get('FragmentPath') != str(ROOT.parent / 'systemd/user' / unit) or fields.get('DropInPaths'):
            raise ValueError('Packaged Qoda units are overridden; inspect systemctl --user cat ' + unit)
    service.main()


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(78)
