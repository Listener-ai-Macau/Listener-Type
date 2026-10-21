import assert from 'node:assert/strict';
import { applyLocalPoliteOption, applyPoliteEdits, localPoliteOptions, reviewPoliteEdits } from './politeEdits.ts';

// FIRST_USE120 rejected Please kindly and manually chose a courteous question.
const customer120 = "Please send me the revised draft by Thursday. I need time to review it before Friday's meeting. Thanks.";
const local = localPoliteOptions(customer120);
assert.deepEqual(local, [{ old: 'Please send me the revised draft by Thursday.', new: 'Could you please send me the revised draft by Thursday?', tone: 'courteous' }, { old: 'Please send me the revised draft by Thursday.', new: 'Would you be able to send me the revised draft by Thursday?', tone: 'softer' }]);
assert.equal(applyLocalPoliteOption(customer120, local[0]), "Could you please send me the revised draft by Thursday? I need time to review it before Friday's meeting. Thanks.");
assert.equal(applyLocalPoliteOption('Send me the log by 18:00.', localPoliteOptions('Send me the log by 18:00.')[0]), 'Could you please send me the log by 18:00?');
assert.equal(applyLocalPoliteOption('  Share the notes with Zoë on Friday!  ', localPoliteOptions('  Share the notes with Zoë on Friday!  ')[0]), '  Could you please share the notes with Zoë on Friday?  ');
for (const unsupported of ['You must send the log by 18:00.', 'Please do not send the log.', 'Could you not send the log?', 'Could you please not send the log?', 'Send me `API_KEY`.', 'Send me https://example.com/log.', 'Send Dr.', 'Send me the log. Send me the log.', '请发日志。', 'Send\nme the log.']) assert.deepEqual(localPoliteOptions(unsupported), [], unsupported);
assert.throws(() => applyLocalPoliteOption(customer120.replace('Thursday', 'Monday'), local[0]));
assert.throws(() => applyLocalPoliteOption(customer120, { ...local[0], new: 'Could you please send me the draft by Monday?' }));
const literalTokens = 'Send $& and $\' and $` as literal text.';
// Backticks deliberately exclude code-like text. Dollar replacement tokens alone
// must remain literal rather than duplicating the old request or its surroundings.
assert.deepEqual(localPoliteOptions(literalTokens), []);
const dollars = 'Send $& and $$ as literal text. Thanks.';
assert.equal(applyLocalPoliteOption(dollars, localPoliteOptions(dollars)[0]), 'Could you please send $& and $$ as literal text? Thanks.');
// FIRST_USE121's natural question fell outside121's imperative-only grammar.
const customer121 = "Could you send me the revised agenda by 3 pm today? I need to prepare the handout. If that won't work, just let me know when you can get it over. Thanks.";
const questionOptions = localPoliteOptions(customer121);
assert.equal(questionOptions.length, 2);
assert.equal(applyLocalPoliteOption(customer121, questionOptions[0]), customer121.replace('Could you send', 'Could you please send'));
assert.equal(applyLocalPoliteOption(customer121, questionOptions[1]), customer121.replace('Could you send', 'Would you be able to send'));
for (const modal of ['Can', 'Could', 'Would', 'Will']) assert.equal(localPoliteOptions(`${modal} you share the notes by 3 pm?`).length, 2);
assert.deepEqual(localPoliteOptions('Could you please send the log?').map(edit => edit.tone), ['softer']);
assert.throws(() => applyLocalPoliteOption(customer121.replace('3 pm', '4 pm'), questionOptions[1]));
const customer122 = 'Hi, could you send me the updated schedule by Friday? I need it to plan the handover. Thanks!';
const greetingOptions = localPoliteOptions(customer122);
assert.equal(greetingOptions.length, 2);
assert.equal(greetingOptions[0].old, 'could you send me the updated schedule by Friday?');
assert.equal(applyLocalPoliteOption(customer122, greetingOptions[1]), 'Hi, would you be able to send me the updated schedule by Friday? I need it to plan the handover. Thanks!');
assert.equal(applyLocalPoliteOption('Hello Zoë, please send the draft by Friday.', localPoliteOptions('Hello Zoë, please send the draft by Friday.')[0]), 'Hello Zoë, could you please send the draft by Friday?');
const conditionalRequest = 'If the draft is ready, could you share it by 18:00? Otherwise, keep it private.';
assert.equal(applyLocalPoliteOption(conditionalRequest, localPoliteOptions(conditionalRequest)[0]), conditionalRequest.replace('could you share', 'could you please share'));
assert.deepEqual(localPoliteOptions('Hi, you must send the draft by Friday.'), []);
assert.deepEqual(localPoliteOptions('He will send the draft by Friday.'), []);

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
