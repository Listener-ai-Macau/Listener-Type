import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
path = Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation/date_fidelity.py'
spec = importlib.util.spec_from_file_location('listener_date_fidelity', path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class DateFidelityTests(unittest.TestCase):
    def test_source_year_restored_only_to_its_matching_date(self):
        source = '2026年10月29日19:30的说明会改到10月30日19:30。'
        output, edits = module.preserve_explicit_years(source, 'Move from October 29 at 19:30 to October 30 at 19:30.', 'zh')
        self.assertEqual(output, 'Move from October 29, 2026 at 19:30 to October 30 at 19:30.')
        self.assertEqual(len(edits), 1)
        self.assertEqual(source, '2026年10月29日19:30的说明会改到10月30日19:30。')

    def test_already_explicit_year_is_unchanged(self):
        text = 'The deadline is 29 October 2026 at 12:00.'
        self.assertEqual(module.preserve_explicit_years('2026年10月29日12:00前回复。', text, 'zh'), (text, []))

    def test_reordered_distinct_dates_keep_each_source_year(self):
        text, _ = module.preserve_explicit_years('2026年12月31日改到2027年1月2日。', 'New: January 2. Old: December 31.', 'zh')
        self.assertEqual(text, 'New: January 2, 2027. Old: December 31, 2026.')

    def test_conflicting_year_is_rejected(self):
        with self.assertRaises(module.DateFidelityError):
            module.preserve_explicit_years('2026年10月29日', 'October 29, 2025', 'zh')

    def test_wrong_day_is_rejected_without_rewriting_it(self):
        with self.assertRaises(module.DateFidelityError):
            module.preserve_explicit_years('2026年10月29日', 'October 30', 'zh')

    def test_same_month_day_different_years_must_not_be_guessed(self):
        with self.assertRaises(module.DateFidelityError):
            module.preserve_explicit_years('2026年10月29日和2027年10月29日。', 'October 29 and October 29.', 'zh')

    def test_ambiguous_repeated_reference_must_not_be_guessed(self):
        with self.assertRaises(module.DateFidelityError):
            module.preserve_explicit_years('2026年10月29日开会。10月29日回复。', 'Meeting October 29; reply October 29.', 'zh')

    def test_reverse_direction_and_leap_day(self):
        text, edits = module.preserve_explicit_years('By February 29, 2028 at 14:30.', '请在2月29日14:30前发送。', 'en')
        self.assertEqual(text, '请在2028年2月29日14:30前发送。')
        self.assertEqual(len(edits), 1)

    def test_non_date_numbers_do_not_supply_a_year(self):
        text = 'Order 2026 needs 500 samples.'
        self.assertEqual(module.preserve_explicit_years('订单2026需要500件样品。', text, 'zh'), (text, []))

    def test_iso_and_invalid_source_calendar_are_not_guessed(self):
        text = '2028-02-29 14:30'
        self.assertEqual(module.preserve_explicit_years('2028年2月29日14:30', text, 'zh'), (text, []))
        self.assertEqual(module.preserve_explicit_years('2027年2月29日', 'February 29', 'zh'), ('February 29', []))


if __name__ == '__main__':
    unittest.main()
