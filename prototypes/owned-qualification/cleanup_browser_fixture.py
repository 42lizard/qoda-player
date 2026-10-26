#!/usr/bin/env python3
"""Remove only the uniquely reidentified fresh browser fixture from this run.

This is cleanup, not evidence of the lost in-memory child token's revocation.
"""
import base64
import datetime
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET
import requests

HERE = Path(__file__).resolve().parent
PRODUCT = 'Qoda Disposable Auth Qualification'


def main():
    initial = json.loads((HERE / 'fresh-auth-evidence.json').read_text())
    stopped = initial['observations'][-1]
    assert stopped['name'] == 'stopped_safely' and stopped['fresh_credential_acquired']
    assert stopped.get('cleanup_identity_established') and not stopped['fresh_device_removed']
    control = json.loads((HERE / 'short-auth-evidence.json').read_text())
    assert control['completed'] and control['observations'][-1]['fresh_device_removed']
    began = datetime.datetime.fromisoformat(initial['recorded_utc']).timestamp()
    payload = subprocess.run(['/usr/bin/secret-tool', 'lookup', 'application', 'qoda-plex-legacy-validation'],
                             capture_output=True, timeout=10, check=True).stdout.decode().strip()
    assert payload.startswith('qoda-legacy-credential-v1:')
    parent = json.loads(base64.b64decode(payload.split(':', 1)[1], validate=True))
    session = requests.Session()
    session.trust_env = False
    headers = {'X-Plex-Token': parent['token'], 'X-Plex-Client-Identifier': parent['client'], 'Accept': 'application/xml'}
    def call(method, path):
        return session.request(method, 'https://plex.tv' + path, headers=headers,
                               timeout=(5, 10), allow_redirects=False)
    def devices():
        response = call('GET', '/devices.xml')
        assert response.status_code == 200 and len(response.content) <= 2_000_000
        return [dict(row.attrib) for row in ET.fromstring(response.content).findall('Device')]
    rows = devices()
    # The first run established exactly one new client/id before its token-field
    # check failed. No other browser run used this product; the short control's
    # device has separately been verified absent. Reidentify only that fixture.
    candidates = [row for row in rows if row.get('product') == PRODUCT]
    assert len(candidates) == 1
    candidate = candidates[0]
    assert candidate.get('clientIdentifier', '').startswith('qoda-disposable-auth-')
    assert candidate.get('id', '').isdigit() and 'server' not in candidate.get('provides', '').lower()
    assert began - 5 <= int(candidate['createdAt']) <= began + 600
    parents = [row for row in rows if row.get('clientIdentifier') == parent['client']]
    assert len(parents) == 1 and parents[0]['id'] != candidate['id']
    response = call('DELETE', '/devices/' + candidate['id'] + '.xml')
    assert response.status_code == 200
    rows = devices()
    absent = not any(row.get('id') == candidate['id'] for row in rows)
    parent_present = any(row.get('id') == parents[0]['id'] for row in rows)
    parent_status = call('GET', '/api/v2/user').status_code
    result = {'completed': absent and parent_present and parent_status == 200,
              'selection': 'one fresh same-product client within first-run creation window; distinct from original device',
              'targeted_removal_http': response.status_code, 'fixture_device_absent': absent,
              'original_device_present': parent_present, 'original_cloud_http': parent_status,
              'remaining_qualification_devices': sum(row.get('product') == PRODUCT for row in rows),
              'limits': ['Cleanup only; original child token was not retained after safe stop',
                         'No claim of PMS revocation from device disappearance'],
              'no_music_playlist_or_timeline_mutations': True, 'original_keyring_untouched': True}
    assert result['completed'] and result['remaining_qualification_devices'] == 0
    (HERE / 'browser-fixture-cleanup-evidence.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
    session.close()


if __name__ == '__main__':
    main()
