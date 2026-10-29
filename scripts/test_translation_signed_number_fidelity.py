"""Regress observed negative sign loss and avoid confusing dates with signs."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation'))
from signed_number_fidelity import require_signed_numbers


class SignedNumberFacts(unittest.TestCase):
    def test_observed_loss_is_rejected_without_exposing_source(self):
        with self.assertRaisesRegex(ValueError, '^local_translation_signed_number_fidelity$'):
            require_signed_numbers('-3.5度以下不要启动。', "Don't start under 3.5 degrees.")

    def test_equivalent_signed_outputs_are_accepted(self):
        for value in ['-3.5', '−3.50', 'minus 3.5', 'negative 3.5', '负3.5', '負3.5', '零下3.5']:
            with self.subTest(value=value):
                require_signed_numbers('-3.5度', value + ' degrees')

    def test_missing_or_wrong_negative_is_rejected(self):
        for translated in ['3.5', '-5', 'minus 35', 'no number']:
            with self.subTest(translated=translated):
                with self.assertRaises(ValueError):
                    require_signed_numbers('-3.5', translated)

    def test_repeated_values_require_the_same_number_of_signs(self):
        require_signed_numbers('-5 and -5', '-5 and minus 5')
        with self.assertRaises(ValueError):
            require_signed_numbers('-5 and -5', '-5 and 5')
        with self.assertRaises(ValueError):
            require_signed_numbers('-5 and -5', 'negative 5 degrees below zero')

    def test_date_identifiers_and_positive_numbers_are_not_negative(self):
        for source in ['2026-10-29', 'A-102', '3-5', '1.5', '2026-10-29 and A-102']:
            with self.subTest(source=source):
                require_signed_numbers(source, 'translation with no numeric signs')

    def test_date_does_not_supply_missing_negative(self):
        with self.assertRaises(ValueError):
            require_signed_numbers('2026-10-29: -10%', '2026-10-29: 10%')
        require_signed_numbers('2026-10-29: -10%', '2026-10-29: -10%')

    def test_observed_negative_word_loss_is_rejected(self):
        with self.assertRaisesRegex(ValueError, '^local_translation_signed_number_fidelity$'):
            require_signed_numbers('本月亏损负100元。', "This month's loss was less than $100.")
        for source in ['负100元', '負100元', '零下100度', 'negative 100 dollars', 'minus 100 degrees']:
            with self.subTest(source=source):
                with self.assertRaises(ValueError):
                    require_signed_numbers(source, '100')

    def test_natural_negative_expressions_and_currency(self):
        pairs = [
            ('零下5度', '5 degrees below zero'),
            ('到了零下10度要关闭设备。', 'Ten degrees below zero to shut down the equipment.'),
            ('5 degrees below zero', '零下5度'),
            ('温度是零下2度，余额为负100元。', 'Temperature is 2 degrees below zero and balance is negative $100.'),
            ('余额是-100元', 'The balance is negative $100.'),
            ('余额是负100元，请不要付款。', 'The balance is negative $100, and please do not pay.'),
            ('balance is negative $100', '余额为负100元'),
            ('负3.5度', 'minus 3.50 degrees'),
        ]
        for source, output in pairs:
            with self.subTest(source=source, output=output):
                require_signed_numbers(source, output)

    def test_negative_word_does_not_create_substring_false_positives(self):
        for source in ['订单A-102', '2026-10-29', 'negative feedback 100', '负责人100位', '正100元']:
            with self.subTest(source=source):
                require_signed_numbers(source, 'ordinary translation')
        with self.assertRaises(ValueError):
            require_signed_numbers('零下5度', 'twenty-five degrees below zero')


if __name__ == '__main__':
    unittest.main()
