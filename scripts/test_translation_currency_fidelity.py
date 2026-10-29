"""Focused regression for an ungrounded dollar on generic 元 amounts."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation'))
from currency_fidelity import require_no_invented_dollar


class CurrencyFacts(unittest.TestCase):
    def test_observed_currency_invention_is_rejected(self):
        for source, target in [('本月预算为100元。', 'The budget for this month is $100.'),
                               ('余额是负100元，请不要付款。', 'The balance is negative $100, and please do not pay.'),
                               ('报价为人民币100元。', 'The offer is USD 100.'),
                               ('报价一百元。', 'The price is 100 dollars.')]:
            with self.subTest(source=source):
                with self.assertRaisesRegex(ValueError, '^local_translation_currency_fidelity$'):
                    require_no_invented_dollar(source, target, 'zh')

    def test_explicit_and_non_dollar_currencies_pass(self):
        for source, target in [('报价为人民币100元。','The offer is 100 yuan.'),
                               ('报价为100美元。','The offer is $100.'),
                               ('报价为100港元。','The offer is HK$100.'),
                               ('报价为100澳门元。','The offer is MOP 100.'),
                               ('报价为100日元。','The offer is 100 yen.'),
                               ('报价为100欧元。','The offer is Euro100.'),
                               ('预算100元，另有50美元。','Budget 100 yuan and $50.')]:
            with self.subTest(source=source):
                require_no_invented_dollar(source, target, 'zh')

    def test_no_amount_or_reverse_translation_are_not_reinterpreted(self):
        for source, target, language in [('元旦预算是100。','New Year budget is $100.', 'zh'),
                                         ('测试100个元素。','Test $100 elements.', 'zh'),
                                         ('The price is $100.','价格为100美元。','en')]:
            require_no_invented_dollar(source, target, language)

    def test_only_currency_context_not_general_amount_correctness(self):
        # Multiple currencies are deliberately not guessed or assigned per amount.
        require_no_invented_dollar('100元加50美元。', '$100 and $50.', 'zh')


if __name__ == '__main__':
    unittest.main()
