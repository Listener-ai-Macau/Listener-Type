import assert from 'node:assert/strict';
import { applyPoliteEdits, reviewPoliteEdits } from './politeEdits.ts';

const source = "Tomorrow's design review at 3 won't work for me. Could we move it to 4? If not, send me your notes and I'll reply by Friday. Thanks!";
const edits = [{ old: 'Could we move it to 4?', new: 'Could we please move it to 4?' }, { old: 'send me your notes', new: 'please send me your notes' }];
const result = reviewPoliteEdits(source, JSON.stringify({ edits }));
assert.deepEqual(result, { edits, skipped: 0 });
assert.equal(applyPoliteEdits(source, [edits[1]]), source.replace('send me your notes', 'please send me your notes'));
assert.equal(applyPoliteEdits(source, edits), "Tomorrow's design review at 3 won't work for me. Could we please move it to 4? If not, please send me your notes and I'll reply by Friday. Thanks!");

// Actual118 cloud counterexample: preserve must, while keeping valid courtesy proposals.
const mandatory = 'You must send the log by 18:00. Do not share the API Key. If validation fails, do not publish.';
const proposed = [{ old: 'You must send the log by 18:00.', new: 'Please send the log by 18:00.' }, { old: 'Do not share the API Key.', new: 'Please do not share the API Key.' }, { old: 'If validation fails, do not publish.', new: 'If validation fails, please do not publish.' }];
const guarded = reviewPoliteEdits(mandatory, JSON.stringify({ edits: proposed }));
assert.equal(guarded.skipped, 1); assert.deepEqual(guarded.edits, proposed.slice(1));
assert.ok(applyPoliteEdits(mandatory, guarded.edits).startsWith('You must send the log by 18:00.'));
assert.throws(() => applyPoliteEdits(mandatory, proposed));
for (const edit of [{ old: '3', new: '2' }, { old: 'Friday', new: 'Monday' }, { old: "won't", new: 'will' }]) {
  assert.equal(reviewPoliteEdits(source, JSON.stringify({ edits: [edit] })).skipped, 1);
}
assert.equal(reviewPoliteEdits('Please use https://example.com/log and tell me.', JSON.stringify({ edits: [{ old: 'example', new: 'other' }] })).skipped, 1);
assert.equal(reviewPoliteEdits('Use `API_KEY` and ask me.', JSON.stringify({ edits: [{ old: 'API_KEY', new: 'OTHER_KEY' }] })).skipped, 1);
for (const invalid of [{ edits: [{ old: 'me', new: 'you' }] }, { edits: [{ old: 'missing', new: 'please' }] }, { edits: [{ old: 'send me your notes', new: 'please send me your notes' }, { old: 'your notes', new: 'the notes' }] }, { edits: [{ old: source, new: 'Whole rewrite' }] }, { edits: [{ old: 'send me your notes', new: '' }] }, { edits: null }]) {
  assert.throws(() => reviewPoliteEdits(source, JSON.stringify(invalid)));
}
const multilingual = '小李，条件😀不变；请查看日志。田中さん、変えてもらえる？';
const jp = { old: '変えてもらえる？', new: '変えていただけますか？' };
assert.equal(applyPoliteEdits(multilingual, [jp]), multilingual.replace(jp.old, jp.new));
assert.equal(reviewPoliteEdits(multilingual, JSON.stringify({ edits: [{ old: '条件😀不变', new: '条件😀改变' }] })).skipped, 1);
assert.throws(() => applyPoliteEdits(source.replace('your notes', 'the notes'), edits));
console.log('Polite edit tests passed: actual courtesy gain, mandatory counterexample, numbers/dates/negation, protected partial spans, overlap/ambiguity, Unicode and stale input. These checks are limited, not a semantic guarantee.');
