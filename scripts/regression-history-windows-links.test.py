"""Reproduce core.symlinks=false in the real Windows source snapshot path."""
from pathlib import Path
import tempfile,importlib.util
spec=importlib.util.spec_from_file_location('history',Path(__file__).with_name('regression-history.py'))
h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
with tempfile.TemporaryDirectory(prefix='listener-source-link-') as folder:
    root=Path(folder)
    h.git(root,'init','-q');h.git(root,'config','user.name','Source Link Fixture')
    h.git(root,'config','user.email','fixture@example.invalid');h.git(root,'config','core.symlinks','false')
    (root/'src').mkdir();(root/'src/target.md').write_bytes(b'content\n')
    link=root/'src/link.md';link.write_bytes(b'target.md')
    h.git(root,'add','.');blob=h.decode(h.git(root,'hash-object','-w','--stdin',data=b'target.md')).strip()
    h.git(root,'update-index','--cacheinfo',f'120000,{blob},src/link.md')
    h.git(root,'commit','-qm','Windows regular-file symlink checkout')
    head=h.git(root,'rev-parse','HEAD');index=(root/'.git/index').read_bytes()
    bound=h.source_state(root,'type',True)
    assert h.git(root,'show',bound['sourceCommit']+':src/link.md')==b'target.md'
    assert h.git(root,'ls-tree',bound['sourceCommit'],'src/link.md').startswith(b'120000 ')
    assert h.git(root,'rev-parse','HEAD')==head and (root/'.git/index').read_bytes()==index
    assert link.read_bytes()==b'target.md' and not link.is_symlink()
    # A user edit to the checked-out link text must affect the fingerprint.
    link.write_bytes(b'other-target.md')
    changed=h.source_state(root,'type',True)
    assert changed['sourceFingerprint']!=bound['sourceFingerprint']
    assert h.git(root,'show',changed['sourceCommit']+':src/link.md')==b'other-target.md'
print('PASS Windows Git symlink stub, mode/bytes recovery, edit detection, real index/HEAD preserved')
