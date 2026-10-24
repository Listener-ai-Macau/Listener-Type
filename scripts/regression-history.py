"""Trace existing feature guards and bind build bytes to a recoverable Git tree.

Snapshot commits live under refs/listener/build-sources; HEAD, the real index,
the working tree, installed software and accepted baselines are never changed.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

TYPE = Path(__file__).resolve().parent.parent
MANIFEST = TYPE / 'scripts/regression-guard-commits.json'
ROOTS = {
    'type': ('src', 'src-tauri', 'windows-ime', 'scripts', 'third_party', 'index.html', 'package.json', 'package-lock.json', 'tsconfig.json', 'vite.config.ts'),
    'firmware': ('main', 'components', 'ports', 'protocols', 'tools', 'third_party', 'CMakeLists.txt', 'sdkconfig', 'sdkconfig.defaults', 'sdkconfig.defaults.esp32s3', 'dependencies.lock', 'VERSION', 'partitions.csv'),
}
OMIT = {'.git', 'target', 'build', 'node_modules', '.artifacts', '__pycache__'}


def git(repo, *args, data=None, env=None):
    p = subprocess.run(['git', '-C', str(repo), *args], input=data,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    if p.returncode:
        raise RuntimeError(p.stderr.decode('utf-8', 'replace').strip())
    return p.stdout


def decode(raw):
    return raw.decode('utf-8', 'surrogateescape')


def selected(path, role):
    parts = Path(path).parts
    if any(part in OMIT for part in parts):
        return False
    roots = ROOTS.get(role)
    return roots is None or any(path == root or path.startswith(root + '/') for root in roots)


def source_state(repo, role, persist):
    repo = repo.resolve()
    head = decode(git(repo, 'rev-parse', 'HEAD')).strip()
    entries = {}
    for raw in git(repo, 'ls-files', '--stage', '-z').split(b'\0'):
        if not raw:
            continue
        info, path = raw.split(b'\t', 1)
        mode, blob, stage = info.split()
        if stage != b'0':
            raise RuntimeError('Resolve the existing merge before binding a build')
        name = decode(path)
        if selected(name, role):
            entries[name] = (decode(mode), decode(blob))
    for raw in git(repo, 'ls-files', '--others', '--exclude-standard', '-z').split(b'\0'):
        if raw:
            name = decode(raw)
            if selected(name, role):
                entries.setdefault(name, ('100644', None))
    # Effective ESP-IDF configuration can be ignored by Git but affects the binary.
    if role == 'firmware' and (repo / 'sdkconfig').is_file():
        entries.setdefault('sdkconfig', ('100644', None))
    rows, updates = [], []
    for name, (mode, old_blob) in sorted(entries.items()):
        path = repo / name
        if mode == '160000':
            if not path.is_dir():
                raise RuntimeError('Missing build dependency: ' + name)
            child = source_state(path, 'dependency', persist)
            rows.append({'path': name, 'sourceFingerprint': child['sourceFingerprint'],
                         'commit': child.get('sourceCommit', child['head'])})
            if persist:
                updates.append(f"160000 {child['sourceCommit']}\t{name}\0".encode())
        elif not path.exists() and not path.is_symlink():
            rows.append({'path': name, 'deleted': True})
            updates.append(f"0 {'0'*40}\t{name}\0".encode())
        else:
            # core.symlinks=false checks a Git symlink out as a regular file
            # containing the target. Preserve those bytes and Git's link mode;
            # Windows readlink() rejects this valid checkout representation.
            contents = os.fsencode(os.readlink(path)) if mode == '120000' and path.is_symlink() else path.read_bytes()
            digest = hashlib.sha256(contents).hexdigest()
            rows.append({'path': name, 'sha256': digest, 'bytes': len(contents)})
            if persist:
                blob = decode(git(repo, 'hash-object', '-w', '--stdin', data=contents)).strip()
                updates.append(f'{mode} {blob}\t{name}\0'.encode('utf-8', 'surrogateescape'))
    # A dependency's recorded commit may be a private snapshot; its stable
    # content fingerprint, rather than commit metadata, establishes equality.
    fingerprint_rows = [{k: v for k, v in row.items() if k != 'commit'} for row in rows]
    fingerprint = hashlib.sha256(json.dumps(fingerprint_rows, sort_keys=True).encode()).hexdigest()
    state = {'repository': str(repo), 'role': role, 'head': head,
             'sourceFingerprint': fingerprint, 'files': rows}
    if persist:
        with tempfile.TemporaryDirectory(prefix='listener-source-index-') as directory:
            env = dict(os.environ, GIT_INDEX_FILE=str(Path(directory) / 'index'))
            git(repo, 'read-tree', head, env=env)
            if updates:
                git(repo, 'update-index', '-z', '--index-info', data=b''.join(updates), env=env)
            tree = decode(git(repo, 'write-tree', env=env)).strip()
            ref = 'refs/listener/build-sources/' + tree
            probe = subprocess.run(['git', '-C', str(repo), 'rev-parse', '--verify', ref],
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            if probe.returncode == 0:
                commit = decode(probe.stdout).strip()
            else:
                message = ('Listener build source snapshot\n\nContent receipt only; '
                           'not human acceptance or a release.\n').encode()
                commit = decode(git(repo, 'commit-tree', tree, '-p', head, data=message)).strip()
                git(repo, 'update-ref', ref, commit)
            state.update(sourceCommit=commit, sourceTree=tree, sourceRef=ref)
    return state


def load(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def begin(args):
    state = source_state(Path(args.repo), args.role, True)
    state.update(schema='listener.build-source.v1', capturedAt=datetime.datetime.now().astimezone().isoformat(),
                 status='candidate_source_only', humanAccepted=False)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(state, indent=2), encoding='utf-8')
    print(json.dumps({key: state[key] for key in ('sourceCommit', 'sourceFingerprint', 'status')}))


def finish(args):
    state = load(args.receipt)
    observed = source_state(Path(state['repository']), state['role'], False)
    if observed['sourceFingerprint'] != state['sourceFingerprint']:
        raise RuntimeError('Build source changed after capture; do not attribute these artifacts to the saved commit')
    artifacts = []
    for name in args.artifact:
        path = Path(name).resolve()
        artifacts.append({'path': str(path), 'bytes': path.stat().st_size,
                          'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
    if not artifacts:
        raise RuntimeError('No artifact supplied; a source snapshot is not a completed build')
    state.update(status='built_not_accepted', artifacts=artifacts,
                 completedAt=datetime.datetime.now().astimezone().isoformat())
    Path(args.receipt).write_text(json.dumps(state, indent=2), encoding='utf-8')
    print(json.dumps({'sourceCommit': state['sourceCommit'], 'artifacts': artifacts, 'humanAccepted': False}))


def trace(args):
    manifest = load(MANIFEST)
    routes = manifest.get('feature_routes', [])
    if not args.feature:
        print('\n'.join(route['id'] for route in routes))
        return
    route = next((route for route in routes if route['id'] == args.feature), None)
    if route is None:
        raise RuntimeError('Unknown feature; run trace without --feature to list existing routes')
    guards = {entry['id']: entry for entry in manifest['entries']}
    anchors = [guards[key] for key in route.get('guard_entries', [])]
    # Routes retain their registration-time description. Delivery updates the
    # referenced guards, so that historical description is not current status.
    states = list(dict.fromkeys(entry['state'] for entry in anchors))
    baseline_state = (states[0] if len(states) == 1 else
                      'multiple_scoped_baselines_see_current_anchors' if states else
                      route.get('baseline_state'))
    print(json.dumps({'feature': route['id'], 'acceptedScope': route.get('accepted_scope'),
                      'baselineState': baseline_state,
                      'registeredBaselineState': route.get('baseline_state'),
                      'currentAnchors': [
                          {key: entry.get(key) for key in ('id', 'repo', 'commit', 'state', 'humanAccepted')}
                          for entry in anchors
                      ],
                      'knownFailures': route.get('known_failures', [])}, ensure_ascii=False, indent=2))
    for repo_id, paths in route['paths'].items():
        repo = (TYPE.parent / manifest['repositories'][repo_id]['path']).resolve()
        anchors = [guards[key] for key in route.get('guard_entries', []) if guards[key]['repo'] == repo_id]
        print('\nREPOSITORY', repo_id, 'HEAD', decode(git(repo, 'rev-parse', 'HEAD')).strip())
        for entry in anchors:
            print('GUARD ANCHOR', entry['commit'], entry['state'], entry['id'])
            print(decode(git(repo, 'log', '--format=%h %ad %s', '--date=iso-strict',
                             entry['commit'] + '..HEAD', '--', *paths)).strip())
            print('CHECK ENTRY POINTS', json.dumps(entry['guards'], ensure_ascii=False))
        if not anchors:
            print('No human-accepted commit is registered for this exact repository/scenario.')
            print(decode(git(repo, 'log', '-8', '--format=%h %ad %s', '--date=iso-strict', '--', *paths)).strip())
        print('UNCOMMITTED', decode(git(repo, 'status', '--short', '--', *paths)).strip() or '(none)')
        print('CURRENT DIFF', decode(git(repo, 'diff', '--stat', 'HEAD', '--', *paths)).strip() or '(none)')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('begin')
    p.add_argument('--repo', required=True)
    p.add_argument('--role', choices=('type', 'firmware'), required=True)
    p.add_argument('--output', required=True)
    p.set_defaults(run=begin)
    p = sub.add_parser('finish')
    p.add_argument('--receipt', required=True)
    p.add_argument('--artifact', action='append', default=[])
    p.set_defaults(run=finish)
    p = sub.add_parser('trace')
    p.add_argument('--feature')
    p.set_defaults(run=trace)
    args = parser.parse_args()
    args.run(args)


if __name__ == '__main__':
    main()
