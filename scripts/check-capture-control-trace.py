"""Replay saved capture-control outcomes using the actual session trace type.

Only the trace implementation is compiled; this does not open BLE, ASR, UI,
recording or clipboard. Pass --output to retain the bounded compiler evidence.
"""
from pathlib import Path
import argparse
import json
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
source = repo / 'src-tauri/src/coordinator/dictation_capture_control_trace.rs'
with tempfile.TemporaryDirectory(prefix='listener-control-trace-') as temporary:
    root = args.output.resolve() if args.output else Path(temporary)
    root.mkdir(parents=True, exist_ok=True)
    (root / 'Cargo.toml').write_text(
        '[package]\nname="listener-capture-control-replay"\nversion="0.1.0"\n'
        'edition="2021"\n[dependencies]\nserde_json="1"\n'
        '[lib]\npath="replay.rs"\n', encoding='utf-8')
    (root / 'replay.rs').write_text(
        'use std::{sync::Arc,time::Instant};\ninclude!('
        + json.dumps(source.as_posix()) + ');\n', encoding='utf-8')
    result = subprocess.run([
        'cargo', 'test', '--offline', '--manifest-path', str(root / 'Cargo.toml'),
        '-j', '2'])
    raise SystemExit(result.returncode)
