#!/usr/bin/env python3
"""Fresh browser client lifecycle; no music/playlist/timeline or parent-token writes."""
import base64
import datetime
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.parse
import uuid
import xml.etree.ElementTree as ET
import requests

HERE = Path(__file__).resolve().parent
PRODUCT = 'Qoda Disposable Auth Qualification'
PREFIX = 'qoda-legacy-credential-v1:'


def main():
    short_control = '--short-control' in sys.argv
    evidence_path = HERE / ('short-auth-evidence.json' if short_control else 'fresh-auth-evidence.json')
    evidence = {'auth_path': 'SDK short-PIN control' if short_control else 'strong browser PIN', 'recorded_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'scope': ('fresh SDK short-PIN control client only' if short_control else 'fresh strong browser PIN client only') + '; no music or metadata mutations',
                'observations': [], 'completed': False}
    def record(name, **facts):
        evidence['observations'].append({'name': name, **facts})
        print(json.dumps({'step': name, **facts}), flush=True)
        evidence_path.write_text(json.dumps(evidence, indent=2) + '\n')
    session = requests.Session()
    session.trust_env = False
    client = 'qoda-disposable-auth-' + str(uuid.uuid4())
    parent = child = candidate = None
    removed = False
    def call(method, url, token=None, identity=client, **kwargs):
        headers = {'Accept': 'application/json', 'X-Plex-Product': PRODUCT,
                   'X-Plex-Version': '0.0.0-validation', 'X-Plex-Platform': 'Linux',
                   'X-Plex-Client-Identifier': identity}
        if token:
            headers['X-Plex-Token'] = token
        headers.update(kwargs.pop('headers', {}))
        return session.request(method, url, headers=headers, timeout=(5, 10), allow_redirects=False, **kwargs)
    def devices():
        response = call('GET', 'https://plex.tv/devices.xml', parent['token'], parent['client'],
                        headers={'Accept': 'application/xml'})
        assert response.status_code == 200
        assert len(response.content) <= 2_000_000
        return [dict(row.attrib) for row in ET.fromstring(response.content).findall('Device')]
    try:
        result = subprocess.run(['/usr/bin/secret-tool', 'lookup', 'application', 'qoda-plex-legacy-validation'],
                                capture_output=True, timeout=10, check=True)
        payload = result.stdout.decode().strip()
        assert payload.startswith(PREFIX)
        parent = json.loads(base64.b64decode(payload[len(PREFIX):], validate=True))
        assert set(parent) == {'client', 'token'} and parent['client'].startswith('qoda-legacy-validation-')
        original = call('GET', 'https://plex.tv/api/v2/user', parent['token'], parent['client'])
        assert original.status_code == 200
        account = original.json()
        before = devices()
        assert not any(row.get('clientIdentifier') == client for row in before)
        before_ids = {row.get('id') for row in before}
        assert any(row.get('clientIdentifier') == parent['client'] for row in before)
        pin = call('POST', 'https://plex.tv/api/v2/pins', data={} if short_control else {'strong': 'true'})
        assert pin.status_code in (200, 201)
        pin = pin.json()
        assert pin.get('clientIdentifier') == client and pin.get('product') == PRODUCT
        url = 'https://app.plex.tv/auth#?' + urllib.parse.urlencode({
            'clientID': client, 'code': pin['code'], 'context[device][product]': PRODUCT})
        if short_control:
            assert len(pin['code']) == 4
            linked = call('PUT', 'https://plex.tv/api/v2/pins/link', parent['token'], parent['client'],
                          headers={'X-Plex-Product': 'Plex SSO'}, data={'code': pin['code']})
            assert linked.status_code in (200, 201, 204)
            record('short_pin_linked', parent_credential_retained=True, http_status=linked.status_code)
        else:
            subprocess.Popen(['/usr/bin/brave', url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            record('awaiting_browser', opened_in_brave=True, parent_credential_retained=True)
        deadline = time.monotonic() + min(int(pin.get('expiresIn', 600)), 600)
        while time.monotonic() < deadline:
            response = call('GET', 'https://plex.tv/api/v2/pins/' + str(int(pin['id'])), params={'code': pin['code']})
            if response.status_code == 429:
                time.sleep(5)
                continue
            assert response.status_code == 200
            polled = response.json()
            assert polled.get('clientIdentifier') == client and polled.get('id') == pin['id']
            child = polled.get('authToken')
            if child:
                break
            time.sleep(3)
        assert child and child != parent['token']
        current = call('GET', 'https://plex.tv/api/v2/user', child)
        assert current.status_code == 200
        current = current.json()
        assert current.get('id') and current['id'] == account.get('id')
        matches = [row for row in devices() if row.get('clientIdentifier') == client
                   and row.get('product') == PRODUCT and row.get('id') not in before_ids]
        assert len(matches) == 1
        candidate = matches[0]
        assert candidate.get('id', '').isdigit()
        # The tested API omits tokens from devices.xml. The polled PIN binds
        # this token to the fresh client, which in turn identifies the new device.
        assert not candidate.get('token') or candidate['token'] == child
        assert 'server' not in candidate.get('provides', '').lower()
        record('fresh_client_identity', same_account=True, distinct_token=True,
               exact_pin_client_device_association=True, device_token_returned=bool(candidate.get('token')), token_shape='jwt' if child.count('.') == 2 else 'opaque')
        resources = call('GET', 'https://plex.tv/api/v2/resources', child, params={'includeHttps': 1, 'includeRelay': 1})
        assert resources.status_code == 200
        owned = [row for row in resources.json() if row.get('owned') and 'server' in row.get('provides', '')]
        assert owned
        server = owned[0]
        endpoints = []
        for connection in server.get('connections', []):
            uri = urllib.parse.urlsplit(connection.get('uri', ''))
            if uri.scheme == 'https' and uri.hostname and uri.hostname.endswith('.plex.direct') and not uri.username and not uri.password:
                endpoints.append(connection['uri'].rstrip('/'))
        pms = None
        for endpoint in endpoints:
            try:
                response = call('GET', endpoint + '/library/sections', server.get('accessToken', child))
                if response.status_code == 200:
                    pms = endpoint
                    break
            except requests.RequestException:
                pass
        assert pms
        # For this owned path only, require the direct polled child token itself
        # to work before measuring its revocation. A resource token is not silently
        # substituted into the post-removal comparison.
        assert call('GET', pms + '/library/sections', child).status_code == 200
        assert call('GET', pms + '/library/sections', parent['token'], parent['client']).status_code == 200
        record('owned_pms_before_removal', fresh_token_http=200, original_token_http=200)
        response = call('DELETE', 'https://plex.tv/devices/' + candidate['id'] + '.xml', parent['token'], parent['client'])
        assert response.status_code == 200
        removed = True
        absent = not any(row.get('id') == candidate['id'] for row in devices())
        cloud_status = call('GET', 'https://plex.tv/api/v2/user', child).status_code
        started = time.monotonic()
        pms_status = call('GET', pms + '/library/sections', child).status_code
        while pms_status == 200 and time.monotonic() - started < 30:
            time.sleep(3)
            pms_status = call('GET', pms + '/library/sections', child).status_code
        parent_cloud = call('GET', 'https://plex.tv/api/v2/user', parent['token'], parent['client']).status_code
        parent_pms = call('GET', pms + '/library/sections', parent['token'], parent['client']).status_code
        record('targeted_device_removal', http_status=response.status_code, exact_device_absent=absent,
               child_cloud_http=cloud_status, child_pms_http=pms_status,
               observation_seconds=round(time.monotonic() - started, 1),
               original_cloud_http=parent_cloud, original_pms_http=parent_pms)
        assert absent and parent_cloud == parent_pms == 200
        evidence['completed'] = True
        record('finished', fresh_device_removed=True, original_keyring_untouched=True,
               no_music_playlist_or_timeline_mutations=True,
               limits=['No account expiry measurement', 'No universal immediate PMS revocation guarantee'])
    except Exception as error:
        record('stopped_safely', error_class=type(error).__name__,
               fresh_device_removed=removed, fresh_credential_acquired=bool(child),
               new_device_candidate_found=bool(candidate), original_keyring_untouched=True)
        # No broad signout or ambiguous cleanup compensates for a failed assertion.
        return 1
    finally:
        session.close()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
