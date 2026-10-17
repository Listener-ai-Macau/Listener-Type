"""Check new source binding without touching a product checkout or process."""
import importlib.util
import json
from pathlib import Path
import tempfile

spec = importlib.util.spec_from_file_location('history', Path(__file__).with_name('regression-history.py'))
history = importlib.util.module_from_spec(spec)
spec.loader.exec_module(history)


def init(root):
    root.mkdir()
    history.git(root, 'init', '-q')
    history.git(root, 'config', 'user.name', 'Provenance Fixture')
    history.git(root, 'config', 'user.email', 'fixture@example.invalid')
    (root / 'src').mkdir()
    (root / 'src/main.rs').write_text('original\n')
    history.git(root, 'add', '.')
    history.git(root, 'commit', '-qm', 'accepted fixture')


with tempfile.TemporaryDirectory(prefix='listener-provenance-test-') as directory:
    root = Path(directory) / 'repo'
    init(root)
    child = root / 'third_party/core'
    child.parent.mkdir()
    init(child)
    history.git(root, 'add', 'third_party/core')
    history.git(root, 'commit', '-qm', 'pin dependency')
    before_head = history.git(root, 'rev-parse', 'HEAD')
    index = root / '.git/index'
    # A user's staged version and unstaged compiled bytes are distinct.
    (root / 'src/main.rs').write_text('staged\n')
    history.git(root, 'add', 'src/main.rs')
    before_index = index.read_bytes()
    (root / 'src/main.rs').write_bytes(b'compiled\r\n')
    (root / 'src/new.rs').write_bytes(b'new compiled file\n')
    (child / 'src/main.rs').write_bytes(b'dirty dependency\n')
    before_child_head = history.git(child, 'rev-parse', 'HEAD')
    bound = history.source_state(root, 'type', True)
    assert history.git(root, 'rev-parse', 'HEAD') == before_head
    assert index.read_bytes() == before_index
    assert (root / 'src/main.rs').read_bytes() == b'compiled\r\n'
    assert history.git(child, 'rev-parse', 'HEAD') == before_child_head
    assert history.git(root, 'show', bound['sourceCommit'] + ':src/main.rs') == b'compiled\r\n'
    assert history.git(root, 'show', bound['sourceCommit'] + ':src/new.rs') == b'new compiled file\n'
    nested = next(row for row in bound['files'] if row['path'] == 'third_party/core')
    assert history.git(child, 'show', nested['commit'] + ':src/main.rs') == b'dirty dependency\n'
    assert history.source_state(root, 'type', True)['sourceCommit'] == bound['sourceCommit']
    assert history.source_state(root, 'type', False)['sourceFingerprint'] == bound['sourceFingerprint']
    receipt = Path(directory) / 'receipt.json'
    args = type('Args', (), {'repo': str(root), 'role': 'type', 'output': str(receipt)})()
    history.begin(args)
    artifact = Path(directory) / 'fixture.exe'
    artifact.write_bytes(b'compiled fixture')
    args = type('Args', (), {'receipt': str(receipt), 'artifact': [str(artifact)]})()
    history.finish(args)
    result = json.loads(receipt.read_text())
    assert result['status'] == 'built_not_accepted' and result['humanAccepted'] is False
    (root / 'src/main.rs').write_text('concurrently changed')
    try:
        history.finish(args)
        raise AssertionError('Changed source was falsely attributed to the old commit')
    except RuntimeError as error:
        assert 'changed after capture' in str(error)
    assert index.read_bytes() == before_index
print('PASS: exact Git source recovery, dirty dependency, staged/index isolation, stable snapshot, artifact binding, source-drift rejection; no automatic acceptance')
