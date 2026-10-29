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

    def test_date_identifiers_and_positive_numbers_are_not_negative(self):
        for source in ['2026-10-29', 'A-102', '3-5', '1.5', '2026-10-29 and A-102']:
            with self.subTest(source=source):
                require_signed_numbers(source, 'translation with no numeric signs')

    def test_date_does_not_supply_missing_negative(self):
        with self.assertRaises(ValueError):
            require_signed_numbers('2026-10-29: -10%', '2026-10-29: 10%')
        require_signed_numbers('2026-10-29: -10%', '2026-10-29: -10%')


if __name__ == '__main__':
    unittest.main()
