"""Run the actual optional worker against explicit, already prepared assets.

No downloads, Type profile, provider credentials, microphone or UI are used.
These are bounded content-preservation cases, not a general quality metric.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--zh-en', type=Path, required=True)
    parser.add_argument('--en-zh', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--worker', type=Path, help='Explicit research candidate; never changes the installed worker')
    args = parser.parse_args()
    worker = args.worker or Path(__file__).resolve().parents[1] / 'src-tauri/resources/translation/worker.py'
    code = worker.read_text(encoding='utf8')
    cases = [
        ('zh', '明天下午三点开会，请带上产品样品。', [('3:00',), ('sample',)]),
        ('en', 'Do not send the email. Save the draft only.', [('不',), ('存',), ('草',)]),
        ('en', 'Please send a quotation for 500 samples by 3 PM on Friday. List the shipping cost separately.', [('500',), ('周五', '星期五'), ('运费',)]),
        ('zh', '你好，明天下午三点提醒我给澳门的客户回邮件，并把新的报价表发给他。语气友好一点，但不要太正式。', [('3:00', '3 p.m.', '3 pm'), ('macao', 'macau'), ('quotation', 'quote'), ('formal',)]),
        ('en', 'Could you send me the updated price list before Friday? Thanks!', [('周五', '星期五'), ('价目', '报价'), ('谢谢', '感谢')]),
        ('en', 'The order is A-102. Please send 500 samples, not 50. Thank you!', [('a-102',), ('500',), ('50',), ('不',), ('谢谢', '感谢')]),
        ('zh', '订单编号是 A-102。需要五百件样品，不是五十件。请单独列出运费。', [('a-102',), ('500', 'five hundred'), ('50', 'fifty'), ('freight', 'shipping')]),
        ('zh', '请确认订单 A-102 的发货时间。\n\n不要发送邮件，只保存草稿。', [('a-102',), ('\n\n',), ('do not send',), ('draft',)]),
        ('zh', '请确认五百件样品的报价，运费单独列出。' * 30 + '最后，请不要取消订单。', [('do not cancel the order',)]),
        # Original customer167 factual omissions. This checks the restored
        # event and both dayparts, not the still-inaccurate availability clause.
        ('zh', '你好，我们可以把周二下午三点的会议改到周三上午十点吗？如果这个时间不方便，请告诉我你有空的时间。谢谢！', [('meeting',), ('tuesday 3 p.m.', 'tuesday 3 pm'), ('wednesday 10 am', 'wednesday 10 a.m.'), ('thank',)]),
        # Coverage-penalty research dropped this already-correct closing.
        ('zh', '请在明天下午三点前把修改后的报价单发给我，谢谢。', [('revised quotation',), ('3 p.m.', '3 pm'), ('tomorrow',), ('thank',)]),
    ]
    rows = []
    for language, text, meaning_groups in cases:
        pair_root = args.zh_en if language == 'zh' else args.en_zh
        request = {'text': text, 'model': str(pair_root / 'model'),
                   'tokenizer': str(pair_root / 'sentencepiece.model')}
        started = time.perf_counter()
        process = subprocess.run([str(args.runtime / 'python.exe'), '-I', '-S', '-X', 'utf8', '-c', code],
                                 input=json.dumps(request, ensure_ascii=False), text=True, encoding='utf8',
                                 capture_output=True, timeout=45)
        output = json.loads(process.stdout)['text'] if process.returncode == 0 else ''
        missing = [group for group in meaning_groups if not any(term in output.lower() for term in group)]
        repeated_count_ok = len(text) < 500 or output.count('500') == 30
        rows.append({'source': language, 'input': text, 'output': output,
                     'seconds': round(time.perf_counter() - started, 4), 'exitCode': process.returncode,
                     'missingMeaningGroups': missing, 'repeatedCountOk': repeated_count_ok,
                     'passed': process.returncode == 0 and not missing and repeated_count_ok})
    result = {'workerSha256': hashlib.sha256(worker.read_bytes()).hexdigest(), 'rows': rows,
              'passed': all(row['passed'] for row in rows),
              'scope': 'Actual worker and saved assets: retained customer cases and bounded factual preservation. Customer167 event/dayparts restored, but its availability request remains inaccurate; these marker checks are not a general semantic-quality score.'}
    args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf8')
    print(json.dumps({'passed': result['passed'], 'cases': len(rows), 'workerSha256': result['workerSha256']}))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
