"""Source-boundary regressions; these do not certify general NMT accuracy."""
import importlib.util
from pathlib import Path
import unittest

WORKER = Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation/worker.py'
spec = importlib.util.spec_from_file_location('translation_sentence_worker', WORKER)
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class TranslationSentenceBoundaries(unittest.TestCase):
    def assert_units(self, text, expected):
        units = worker.translation_sentences(text)
        self.assertEqual(''.join(units), text, 'No source character may be rewritten or dropped')
        self.assertEqual([unit.strip() for unit in units if unit.strip()], expected)

    def test_title_stays_with_name(self):
        for title in ['Mr.', 'Mrs.', 'Ms.', 'Dr.', 'Prof.']:
            with self.subTest(title=title):
                self.assert_units(f'Ask {title} Lee. Save the draft only.',
                                  [f'Ask {title} Lee.', 'Save the draft only.'])

    def test_time_keeps_its_deadline_continuation(self):
        for continuation in ['tomorrow', 'on Friday', 'Friday', 'today', 'UTC+8']:
            with self.subTest(continuation=continuation):
                self.assert_units(f'Call at 3 p.m. {continuation}. Thank you.',
                                  [f'Call at 3 p.m. {continuation}.', 'Thank you.'])

    def test_morning_time_and_title(self):
        self.assert_units('Dr. Lee will call at 9 a.m. on Friday. Do not cancel.',
                          ['Dr. Lee will call at 9 a.m. on Friday.', 'Do not cancel.'])

    def test_time_at_real_sentence_end_stays_separate(self):
        self.assert_units('The meeting ends at 3 p.m. Do not cancel the order.',
                          ['The meeting ends at 3 p.m.', 'Do not cancel the order.'])

    def test_may_question_after_time_is_a_new_sentence(self):
        self.assert_units('The meeting ends at 3 p.m. May I call you tomorrow?',
                          ['The meeting ends at 3 p.m.', 'May I call you tomorrow?'])

    def test_parenthesized_timezone_stays_with_time(self):
        self.assert_units('Meet at 3 p.m. (UTC+8). Save the draft.',
                          ['Meet at 3 p.m. (UTC+8).', 'Save the draft.'])

    def test_distinct_instructions_never_merge(self):
        self.assert_units('Do not send the email. Save the draft only.',
                          ['Do not send the email.', 'Save the draft only.'])

    def test_chinese_and_question_boundaries_remain(self):
        self.assert_units('First! Next?第三。第四。', ['First!', 'Next?', '第三。', '第四。'])

    def test_decimal_and_ascii_identifier_are_unchanged(self):
        self.assert_units('The ratio is 3.14. Keep A_102 intact.',
                          ['The ratio is 3.14.', 'Keep A_102 intact.'])

    def test_numbered_item_prefix_stays_outside_model_without_touching_decimals(self):
        for source, prefix, body in [
            ('1. 测试账号名单', '1. ', '测试账号名单'),
            ('  12)\t不要发送旧报价。', '  12)\t', '不要发送旧报价。'),
            ('1.5 度', '', '1.5 度'),
            ('2026. 10. 29', '', '2026. 10. 29'),
            ('编号 1. 列表', '', '编号 1. 列表'),
            ('1.', '', '1.'),
        ]:
            with self.subTest(source=source):
                self.assertEqual(worker.split_numbered_list_prefix(source), (prefix, body))
                self.assertEqual(prefix + body, source)

    def test_terminal_abbreviation_is_not_empty_translation(self):
        self.assert_units('Call at 3 p.m.', ['Call at 3 p.m.'])


if __name__ == '__main__':
    unittest.main()
