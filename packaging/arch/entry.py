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
    service = module('qoda_launch', release / 'launch.py').Launch(release, packaged=True)
    if sys.argv[1:] == ['run-backend']:
        compatibility = module('qoda_compatibility', release / 'release_state.py')
        compatibility.data_compatible(compatibility.release_evidence(release))
        os.execv(str(release / 'qoda-backend'), [str(release / 'qoda-backend'), '--ui-launcher', str(ROOT.parent.parent / 'bin/qoda')])
    service.main()


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(78)
