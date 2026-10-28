from pathlib import Path
import runpy
import sys
import unittest

sys.dont_write_bytecode = True
root = Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation'
sys.path.insert(0, str(root))
import result_fidelity as guard
splitter = runpy.run_path(str(root / 'worker.py'))['translation_sentences']


class ResultFidelityTests(unittest.TestCase):
    def test_whole_nonrepetitive_context_is_preserved(self):
        source = 'Dr. Lee will call at 3 p.m. on Friday. Do not cancel the meeting.\n\nPlease reply.'
        self.assertEqual(guard.translation_units(source, splitter), [(source, 1)])

    def test_repeated_cardinality_and_distinct_final_instruction(self):
        sentence = '请确认五百件样品的报价，运费单独列出。'
        source = sentence * 30 + '最后，请不要取消订单。'
        pieces = guard.translation_units(source, splitter)
        self.assertEqual(pieces, [(sentence, 30), ('最后，请不要取消订单。', 1)])
        self.assertEqual(''.join(value * count for value, count in pieces), source)

    def test_english_whitespace_remains_in_original_units(self):
        source = 'Do not send. ' * 30 + 'Save the draft.'
        pieces = guard.translation_units(source, splitter)
        self.assertEqual(''.join(value * count for value, count in pieces), source)
        self.assertEqual(sum(count for _, count in pieces), 31)

    def test_year_alignment_is_per_original_line(self):
        source = '2026年10月29日开会。\n\n请在10月29日回复。'
        result, changes = guard.preserve_layout_and_years(source, 'Meeting October 29.\n\nReply October 29.', 'zh')
        self.assertEqual(result, 'Meeting October 29, 2026.\n\nReply October 29.')
        self.assertEqual(len(changes), 1)

    def test_original_newlines_blank_lines_and_trailing_line_are_restored(self):
        source = '标题\r\n \r\n2028年2月29日14:30前回复。\r\n'
        result, _ = guard.preserve_layout_and_years(source, '\nTitle\n\n\nReply by February 29 at 14:30.\n', 'zh')
        self.assertEqual(result, 'Title\r\n \r\nReply by February 29, 2028 at 14:30.\r\n')

    def test_changed_nonblank_line_count_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'structure_fidelity'):
            guard.preserve_layout_and_years('第一项\n第二项', 'Merged result', 'zh')

    def test_join_does_not_insert_spaces_at_existing_newlines(self):
        self.assertEqual(guard.assemble_translations(['Header\n\n', 'Repeated.', 'Finally stop.']),
                         'Header\n\nRepeated. Finally stop.')

    def test_output_capacity_counts_utf8_and_inserted_separators(self):
        self.assertEqual(len(guard.assemble_translations(['a' * 30000, 'b' * 29999]).encode('utf8')), 60000)
        with self.assertRaisesRegex(ValueError, 'output_limit'):
            guard.assemble_translations(['😀' * 10000, 'b' * 20000])

    def test_repetition_expansion_is_bounded_before_large_string_assembly(self):
        with self.assertRaisesRegex(ValueError, 'output_limit'):
            guard.assemble_translations(['a' * 40000] * 4000)


if __name__ == '__main__':
    unittest.main()
