"""Run intermittent input checks against one already-built Rust test binary.

Build the library tests once with the normal platform toolchain, then pass that
executable through --test-binary. No microphone, editor, clipboard, BLE or cloud
is opened by these selections. Synthetic speech states test scheduling policy;
they do not certify the acoustic detector or an actual capture continuation.
"""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    binary = args.test_binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    listed = subprocess.run([str(binary), '--list'], capture_output=True, text=True, check=True).stdout
    required = (
        'intermittent_tail_fault_matrix_holds_continuous_speech_then_releases_real_quiet',
        'intermittent_tail_fault_matrix_rejects_resumed_and_out_of_order_speech',
    )
    if not all(name + ': test' in listed for name in required):
        parser.error('The supplied executable does not contain the current intermittent fault checks')
    results = []
    for selection in ('endpoint_clock_submission_tests', 'ended_ensure_predecessor', 'wake_recovery_task_tests'):
        result = subprocess.run([str(binary), selection, '--test-threads=1'], capture_output=True, text=True)
        (output / (selection + '.log')).write_text(result.stdout + result.stderr, encoding='utf8')
        summary = next((line for line in result.stdout.splitlines() if line.startswith('test result:')), None)
        success = result.returncode == 0 and summary and not summary.startswith('test result: ok. 0 passed;')
        results.append({'selection': selection, 'exitCode': result.returncode, 'summary': summary})
        if not success:
            break
    unchanged = hashlib.sha256(binary.read_bytes()).hexdigest() == digest
    complete = unchanged and len(results) == 3 and all(row['exitCode'] == 0 and row['summary'] and not row['summary'].startswith('test result: ok. 0 passed;') for row in results)
    report = {'complete': complete, 'testBinarySha256': digest, 'results': results,
              'scope': 'Offline reducer/task scheduling; no real audio, editor, clipboard, BLE or cloud',
              'fieldAcceptance': False}
    (output / 'verification.json').write_text(json.dumps(report, indent=2), encoding='utf8')
    print(json.dumps(report))
    return 0 if complete else 1


if __name__ == '__main__':
    raise SystemExit(main())
