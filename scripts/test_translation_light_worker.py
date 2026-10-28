"""Execute the same bundled light-worker protocol with deterministic model IO.

This tests source/time/date handling and error delivery, not model quality.
The actual saved CPU models are checked separately.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

resources = Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation'
bundle = '\n'.join((resources / name).read_text(encoding='utf8') for name in
                   ['date_fidelity.py', 'time_fidelity.py', 'worker.py'])
fixture = '''
import sys, types
class Tokenizer:
    def __init__(self, **kwargs): pass
    def encode(self, value, **kwargs): return [value] if value.strip() else []
    def decode(self, value): return value[0]
class Engine:
    def __init__(self, *args, **kwargs):
        assert kwargs['device'] == 'cpu' and kwargs['compute_type'] == 'int8'
    def translate_batch(self, chunks, **kwargs):
        return [types.SimpleNamespace(hypotheses=[[REPLIES[chunk[0].strip()]]]) for chunk in chunks]
sys.modules['sentencepiece'] = types.SimpleNamespace(SentencePieceProcessor=Tokenizer)
sys.modules['ctranslate2'] = types.SimpleNamespace(Translator=Engine)
'''


class LightWorker(unittest.TestCase):
    def run_worker(self, source, replies, language='zh'):
        with tempfile.TemporaryDirectory() as root:
            request = {'source': language, 'text': source, 'model': str(Path(root) / 'model'),
                       'tokenizer': str(Path(root) / 'sentencepiece.model')}
            code = 'REPLIES=' + repr(replies) + '\n' + fixture + '\n' + bundle
            process = subprocess.run([sys.executable, '-I', '-S', '-X', 'utf8', '-c', code],
                                     input=json.dumps(request), text=True, encoding='utf8', capture_output=True, timeout=10)
            self.assertNotIn('local_translation_runtime_failed', process.stdout, process.stderr)
            return process.returncode, json.loads(process.stdout)

    def test_light_route_corrects_only_source_backed_clock_facts(self):
        self.assertEqual(self.run_worker('上午十点到十一点开会。',
                         {'上午十点到十一点开会。': 'Meet between 10 and 11 p.m.'}),
                         (0, {'text': 'Meet between 10 a.m. and 11 a.m.'}))

    def test_missing_time_rejects_entire_result_with_structured_error(self):
        self.assertEqual(self.run_worker('下午三点开会。', {'下午三点开会。': 'Meet tomorrow.'}),
                         (1, {'error': 'local_translation_time_fidelity'}))

    def test_negation_scope_cannot_be_changed_by_clock_repair(self):
        self.assertEqual(self.run_worker('Do not arrive before 3 p.m.',
                         {'Do not arrive before 3 p.m.': '下午3点之后到。'}, 'en'),
                         (1, {'error': 'local_translation_time_fidelity'}))

    def test_light_route_restores_years_and_exact_source_layout(self):
        self.assertEqual(self.run_worker('  2026年10月29日开会。  \r\n\r\n谢谢。\n',
                         {'2026年10月29日开会。': 'Meet October 29.', '谢谢。': 'Thank you.'}),
                         (0, {'text': '  Meet October 29, 2026.  \r\n\r\nThank you.\n'}))

    def test_wrong_date_is_structured_and_never_accepted_as_partial(self):
        self.assertEqual(self.run_worker('2026年10月29日开会。', {'2026年10月29日开会。': 'Meet October 30.'}),
                         (1, {'error': 'local_translation_date_fidelity'}))

    def test_input_and_unsupported_language_errors_stay_explicit(self):
        self.assertEqual(self.run_worker('', {}), (1, {'error': 'local_translation_invalid_text'}))
        self.assertEqual(self.run_worker('hello', {}, 'ja'), (1, {'error': 'local_translation_unsupported_pair'}))

    def test_bundled_script_fits_windows_command_line(self):
        command = subprocess.list2cmdline(['C:/example/runtime/python.exe', '-I', '-S', '-X', 'utf8', '-c', bundle])
        self.assertLess(len(command.encode('utf-16-le')) // 2, 32767)


if __name__ == '__main__':
    unittest.main()
