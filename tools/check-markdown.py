#!/usr/bin/env python3
"""Run Markdown lint and Marksman diagnostics on Git-visible documentation."""
import argparse
import json
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent


def check_marksman(root, files):
    """Wait for two diagnostic publications instead of guessing a sleep duration."""
    messages = queue.Queue()
    diagnostics = {}
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(
            ['marksman', 'server'], cwd=root, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=errors,
        )

        def read_messages():
            try:
                while True:
                    headers = {}
                    while True:
                        line = process.stdout.readline()
                        if not line:
                            raise RuntimeError('Marksman closed its output unexpectedly')
                        if line == b'\r\n':
                            break
                        key, value = line.decode().split(':', 1)
                        headers[key.lower()] = value.strip()
                    data = process.stdout.read(int(headers['content-length']))
                    messages.put(json.loads(data))
            except Exception as error:
                messages.put(error)

        reader = threading.Thread(target=read_messages, daemon=True)
        reader.start()

        def send(method, params, request_id=None):
            message = {'jsonrpc': '2.0', 'method': method, 'params': params}
            if request_id is not None:
                message['id'] = request_id
            data = json.dumps(message).encode()
            process.stdin.write(f'Content-Length: {len(data)}\r\n\r\n'.encode() + data)
            process.stdin.flush()

        def wait_for(predicate):
            deadline = time.monotonic() + 30
            while True:
                try:
                    message = messages.get(timeout=max(0, deadline - time.monotonic()))
                except queue.Empty as error:
                    raise RuntimeError('Timed out waiting for Marksman diagnostics') from error
                if isinstance(message, Exception):
                    raise message
                if 'error' in message:
                    raise RuntimeError(f'Marksman request failed: {message["error"]}')
                if message.get('method') == 'textDocument/publishDiagnostics':
                    params = message['params']
                    diagnostics[params['uri']] = params['diagnostics']
                if predicate(message):
                    return

        try:
            send('initialize', {
                'processId': None, 'rootUri': root.as_uri(), 'capabilities': {},
                'workspaceFolders': [{'uri': root.as_uri(), 'name': root.name}],
            }, 1)
            wait_for(lambda message: message.get('id') == 1)
            send('initialized', {})
            for path in files:
                send('textDocument/didOpen', {'textDocument': {
                    'uri': path.as_uri(), 'languageId': 'markdown', 'version': 1,
                    'text': path.read_text(encoding='utf-8'),
                }})

            # Marksman pushes only changed diagnostics, including no publication
            # for clean files. An in-memory broken link proves it processed the
            # opens above. Clearing it confirms that publication batch finished.
            token = uuid.uuid4().hex
            probe = (root / f'qa-probe-{token}.md').as_uri()
            send('textDocument/didOpen', {'textDocument': {
                'uri': probe, 'languageId': 'markdown', 'version': 1,
                'text': f'# QA probe\n\n[Missing](./qa-missing-{token}.md)\n',
            }})
            wait_for(lambda _: bool(diagnostics.get(probe)))
            send('textDocument/didChange', {
                'textDocument': {'uri': probe, 'version': 2},
                'contentChanges': [{'text': '# QA probe\n'}],
            })
            wait_for(lambda _: probe in diagnostics and not diagnostics[probe])
            send('shutdown', None, 2)
            wait_for(lambda message: message.get('id') == 2)
            send('exit', None)
            process.wait(timeout=5)
            if process.returncode != 0:
                raise RuntimeError(f'Marksman exited with status {process.returncode}')
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            reader.join(timeout=5)
            process.stdin.close()
            process.stdout.close()

    count = 0
    for path in files:
        for diagnostic in diagnostics.get(path.as_uri(), []):
            position = diagnostic['range']['start']
            print(f'{path.relative_to(root)}:{position["line"] + 1}:'
                  f'{position["character"] + 1}: {diagnostic["message"]}')
            count += 1
    print(f'Marksman: {count} diagnostics in {len(files)} Markdown files')
    return count == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT, help='Git workspace to check')
    root = parser.parse_args().root.resolve()
    for tool in ('markdownlint-cli2', 'marksman'):
        if shutil.which(tool) is None:
            raise RuntimeError(f'{tool} is required on PATH; see docs/development.md')
    names = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', '*.md'],
        cwd=root,
    ).decode().split('\0')
    files = sorted({root / name for name in names if name and (root / name).is_file()})
    if not files:
        raise RuntimeError('No Markdown files found')
    lint = subprocess.run(['markdownlint-cli2', *[str(path) for path in files]], cwd=root)
    links_ok = check_marksman(root, files)
    return 0 if lint.returncode == 0 and links_ok else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f'Markdown check failed: {error}', file=sys.stderr)
        sys.exit(1)
