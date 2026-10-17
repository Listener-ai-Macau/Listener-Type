"""Run the actual reachability state/publisher tests without rebuilding Tauri/media."""
from pathlib import Path
import argparse
import json
import os
import shutil
import subprocess
import tempfile


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target-dir', type=Path, default=(repo / 'src-tauri/target').resolve() / 'network-health-focused')
    args = parser.parse_args()
    cargo = shutil.which('cargo') or str(Path.home() / '.cargo/bin/cargo.exe')
    with tempfile.TemporaryDirectory(prefix='listener-network-health-') as folder:
        root = Path(folder)
        (root / 'src').mkdir()
        (root / 'Cargo.toml').write_text('[package]\nname="listener-network-health-replay"\nversion="0.0.0"\nedition="2021"\n[dependencies]\nserde={version="1",features=["derive"]}\nlog="0.4"\n', encoding='utf8')
        modules = ['network_health_state', 'net_health']
        (root / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + ''.join(
            f'#[path={json.dumps((repo / "src-tauri/src" / (name + ".rs")).as_posix())}]\nmod {name};\n'
            for name in modules), encoding='utf8')
        result = subprocess.run([cargo, 'test', '--offline', '--manifest-path', str(root / 'Cargo.toml'), '--target-dir', str(args.target_dir)])
        raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
