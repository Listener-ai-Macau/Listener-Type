"""Source-backed time facts, ambiguity and false numeric-anchor regressions."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation'))
from time_fidelity import TimeFidelityError, preserve_explicit_times


class ClockFacts(unittest.TestCase):
    def check(self, source, translated, expected, language='zh'):
        actual, edits = preserve_explicit_times(source, translated, language)
        self.assertEqual(actual, expected)
        self.assertEqual(preserve_explicit_times(source, actual, language), (actual, []))
        return edits

    def test_customer_range_has_no_written_daypart(self):
        self.check('师傅上午十点到。我们十点到十一点有会，改到下午两点以后。',
                   'He arrives at 10 a.m. We meet between 10 and 11 p.m.; move to 2 p.m.',
                   'He arrives at 10 a.m. We meet between 10 and 11; move after 2 p.m.')

    def test_explicit_morning_range_and_reordered_points(self):
        self.check('上午十点到十一点开会，下午三点回电。',
                   'Call at 3 a.m. Meet between 10 and 11 p.m.',
                   'Call at 3 p.m. Meet between 10 a.m. and 11 a.m.')

    def test_source_before_and_minutes_are_retained(self):
        self.check('下午两点三十分之前提交。', 'Submit at 2:30 a.m.', 'Submit before 2:30 p.m.')
        self.check('上午九点半以后到。', 'Arrive before 9:30 p.m.', 'Arrive after 9:30 a.m.')
        self.check('上午九点半以后到。', 'Arrive after 9.30 a.m.', 'Arrive after 9.30 a.m.')

    def test_no_inferred_daypart_for_english_point(self):
        self.check('Meet at 3 on Thursday.', '周四下午3点见。', '周四3点见。', 'en')
        self.check('Meet at 3 on Thursday.', '星期四三点见。', '星期四三点见。', 'en')

    def test_reverse_direction_and_deadline(self):
        self.check('Call after 10 a.m. Submit by 3 p.m.',
                   '上午10点之前回电，上午3点之前提交。',
                   '上午10点之后回电，不晚于下午3点提交。', 'en')

    def test_equivalent_24_hour_output_stays_unchanged(self):
        self.check('下午三点开会。', 'Meet at 15:00.', 'Meet at 15:00.')
        self.check('Meet at 00:30.', '凌晨12点30分见。', '凌晨12点30分见。', 'en')
        self.check('19:30开会。', 'Meet at 7:30 a.m.', 'Meet at 7:30 p.m.')

    def test_removing_invented_period_keeps_sentence_boundary(self):
        self.check('十点到十一点开会。', 'Meet between 10 and 11 p.m.', 'Meet between 10 and 11.')
        self.check('十点到十一点开会。', 'Meet between 10 and 11 p.m. tomorrow.', 'Meet between 10 and 11 tomorrow.')

    def test_wrong_time_or_missing_range_is_rejected(self):
        for output in ['Meet at 4 p.m.', 'Meet tomorrow.', 'Meet at 3 p.m. Leave at 4 p.m.']:
            with self.assertRaises(TimeFidelityError):
                preserve_explicit_times('下午三点开会。', output, 'zh')
        with self.assertRaises(TimeFidelityError):
            preserve_explicit_times('上午十点到十一点开会。', 'Meet at 10 a.m.', 'zh')

    def test_repeated_different_dayparts_are_not_guessed(self):
        source = '上午十点来，下午十点走。'
        self.check(source, 'Arrive at 10 a.m. Leave at 10 p.m.', 'Arrive at 10 a.m. Leave at 10 p.m.')
        with self.assertRaises(TimeFidelityError):
            preserve_explicit_times(source, 'Arrive at 10 a.m. Leave at 10 a.m.', 'zh')

    def test_repeated_identical_facts_can_be_restored(self):
        self.check('上午十点来。上午十点来。', 'Come at 10 p.m. Come at 10 p.m.',
                   'Come at 10 a.m. Come at 10 a.m.')

    def test_quantities_dates_ids_and_vague_period_are_not_clock_guesses(self):
        for source, translated, language in [
            ('给我十点建议和十一件样品。', 'Give me 10 suggestions and 11 samples.', 'zh'),
            ('Send between 10 and 11 samples.', '发送10到11件样品。', 'en'),
            ('订单A-102，2026年5月10日，预算50美元。', 'Order A-102, May 10, 2026, budget 50 dollars.', 'zh'),
            ('下午见。', 'See you tomorrow.', 'zh'),
            ('The ratio is 3.14.', '比例是3.14。', 'en'),
            ('Please keep the budget at 12 dollars.', '请把预算保持在12美元。', 'en'),
            ('Keep the budget at 12.', '预算保持在12。', 'en'),
            ('Call before 3 days have passed.', '请在三天内来电。', 'en'),
        ]:
            self.check(source, translated, translated, language)

    def test_explicit_cross_midnight_range(self):
        self.check('晚上十一点到凌晨一点工作。', 'Work between 11 a.m. and 1 p.m.',
                   'Work between 11 p.m. and 1 a.m.')


if __name__ == '__main__':
    unittest.main()
