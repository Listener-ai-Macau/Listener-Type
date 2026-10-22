"""Upload and verify a staged release asset before replacing the download name.

GitHub renames assets through PATCH /repos/{repo}/releases/assets/{id}:
https://docs.github.com/en/rest/releases/assets#update-a-release-asset
Promotion needs two renames; it is not a zero-gap server-side transaction.
An upload failure never deletes or renames the previous canonical download.
"""
import argparse
import hashlib
import json
import shutil
import subprocess
import urllib.parse
import urllib.request
from pathlib import Path


def matches(asset, size, sha):
    return bool(asset and asset.get('state') == 'uploaded' and asset.get('size') == size
                and asset.get('digest') == 'sha256:' + sha)


def promote_verified_asset(api, canonical, size, sha, upload):
    """Small observable state machine; never dispose of the old unverified copy."""
    assets = api.list()
    old = next((x for x in assets if x['name'] == canonical), None)
    if matches(old, size, sha):
        return {'complete': True, 'assetId': old['id'], 'alreadyCurrent': True}
    path = Path(canonical)
    staging = path.stem + '.pending-' + sha[:12] + path.suffix
    candidate = next((x for x in assets if x['name'] == staging), None)
    if candidate and not matches(candidate, size, sha):
        raise RuntimeError('Existing staging asset is incomplete or different; inspect before retry')
    if not candidate:
        candidate = upload(staging)
    if not matches(candidate, size, sha) or candidate['name'] != staging:
        raise RuntimeError('Staged upload did not match the local file; previous download retained')
    # No deletion or canonical rename has happened before this point.
    backup = path.stem + '.previous-' + str(old['id']) + path.suffix if old else None
    if backup and any(x['name'] == backup for x in api.list()):
        raise RuntimeError('Previous backup name already exists; inspect before promotion')
    def restore_previous():
        current = api.list()
        named = next((x for x in current if x['name'] == canonical), None)
        if named and named['id'] == candidate['id']:
            api.rename(candidate['id'], staging)
            named = None
        if old and not named and any(x['id'] == old['id'] for x in current):
            api.rename(old['id'], canonical)

    try:
        if old:
            api.rename(old['id'], backup)
        try:
            api.rename(candidate['id'], canonical)
        except Exception:
            # A response can be lost after a successful mutation. Re-read first.
            named = next((x for x in api.list() if x['name'] == canonical), None)
            if not (named and named['id'] == candidate['id'] and matches(named, size, sha)):
                raise
        verified = api.get(candidate['id'])
        if verified['name'] != canonical or not matches(verified, size, sha):
            raise RuntimeError('Promoted asset identity or digest did not verify')
    except Exception:
        restore_previous()
        raise
    # Preserve the old content until the canonical new object is independently read.
    if old:
        api.delete(old['id'])
    return {'complete': True, 'assetId': candidate['id'], 'alreadyCurrent': False,
            'size': size, 'digest': 'sha256:' + sha}


class GitHubAssets:
    def __init__(self, repo, release):
        self.repo, self.release = repo, release

    @staticmethod
    def command(args):
        result = subprocess.run(args, capture_output=True, text=True, encoding='utf-8', timeout=120)
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or 'GitHub command failed')
        return result.stdout.strip()

    def endpoint(self, asset_id):
        return f'repos/{self.repo}/releases/assets/{asset_id}'

    def list(self):
        return json.loads(self.command(['gh', 'api', f'repos/{self.repo}/releases/{self.release["id"]}/assets?per_page=100']))

    def get(self, asset_id):
        return json.loads(self.command(['gh', 'api', self.endpoint(asset_id)]))

    def rename(self, asset_id, name):
        return json.loads(self.command(['gh', 'api', '--method', 'PATCH', self.endpoint(asset_id), '-f', 'name=' + name]))

    def delete(self, asset_id):
        self.command(['gh', 'api', '--method', 'DELETE', self.endpoint(asset_id)])

    def upload(self, file, name, reply):
        curl = shutil.which('curl.exe') or shutil.which('curl')
        if not curl:
            raise RuntimeError('curl is required for this developer release command')
        token = self.command(['gh', 'auth', 'token', '--hostname', 'github.com'])
        if not token or any(ch in token for ch in '\r\n"\\'):
            raise RuntimeError('Unexpected credential format')
        url = self.release['upload_url'].split('{', 1)[0]
        if urllib.parse.urlsplit(url).hostname != 'uploads.github.com':
            raise RuntimeError('Unexpected release upload host')
        url += '?' + urllib.parse.urlencode({'name': name})
        config = 'header = "Authorization: Bearer ' + token + '"\n'
        proxy = urllib.request.getproxies().get('https')
        if proxy:
            config += 'proxy = ' + json.dumps(proxy) + '\n'
        try:
            result = subprocess.run([curl, '--config', '-', '--http1.1', '--request', 'POST', '--url', url,
                '--header', 'Accept: application/vnd.github+json', '--header', 'Content-Type: application/octet-stream',
                '--data-binary', '@' + str(file), '--output', str(reply), '--connect-timeout', '20', '--max-time', '300',
                '--speed-limit', '65536', '--speed-time', '30', '--silent', '--show-error'],
                input=config, text=True, encoding='utf-8', capture_output=True, timeout=330)
        finally:
            token = config = None
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or 'Staged upload failed')
        return json.loads(reply.read_text(encoding='utf-8'))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repo', required=True)
    p.add_argument('--tag', required=True)
    p.add_argument('--file', required=True, type=Path)
    p.add_argument('--receipt', required=True, type=Path)
    a = p.parse_args()
    file = a.file.resolve(strict=True)
    a.receipt.parent.mkdir(parents=True, exist_ok=True)
    with file.open('rb') as f:
        sha = hashlib.file_digest(f, 'sha256').hexdigest()
    release = json.loads(GitHubAssets.command(['gh', 'api', f'repos/{a.repo}/releases/tags/{urllib.parse.quote(a.tag, safe="")}']))
    api = GitHubAssets(a.repo, release)
    report = {'repo': a.repo, 'tag': a.tag, 'file': file.name, 'size': file.stat().st_size, 'sha256': sha}
    reply = a.receipt.with_name(a.receipt.stem + '-upload-response.json')
    try:
        report.update(promote_verified_asset(api, file.name, file.stat().st_size, sha,
            lambda name: api.upload(file, name, reply)))
    except Exception as error:
        report.update(complete=False, error=str(error))
        raise
    finally:
        a.receipt.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
