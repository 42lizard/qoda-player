#!/usr/bin/python3
"""Refuse package replacement/removal while any packaged player is running."""
import argparse
import os
from pathlib import Path
import sys


def check(proc):
    for process in proc.iterdir():
        if not process.name.isdecimal() or int(process.name) == os.getpid():
            continue
        try:
            executable = str((process / 'exe').readlink())
            arguments = (process / 'cmdline').read_bytes().split(b'\0')
        except FileNotFoundError:
            continue
        except PermissionError:
            raise ValueError('Cannot verify running processes; retry with normal pacman privileges') from None
        if executable.startswith('/usr/lib/qoda-player/') or any(arg.startswith(b'/usr/lib/qoda-player/') or arg == b'/usr/bin/qoda' for arg in arguments):
            raise ValueError('Quit packaged Qoda in every user session before upgrading/removing it: /usr/bin/qoda quit')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--proc-root', type=Path, default=Path('/proc'), help='Process filesystem (for isolated verification)')
    try:
        check(parser.parse_args().proc_root)
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
