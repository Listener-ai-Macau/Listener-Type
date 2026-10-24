import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createInstance } from 'i18next';
import { styleEditorMessages, vocabPresetMessages } from '../i18n/styleEditor.ts';
import { newStylePackDraft, styleEditorCopy } from './styleEditorCopy.ts';
import { vocabPresetLabel } from './vocabPresetPresentation.ts';
import defaults from './vocab-presets.json' with { type: 'json' };

const expectedHashes = {
  en: '9e478eb8df3ba4454dd3ae67510f0053d22c7ed2efbf1c3635d29ae446a811de',
  'zh-CN': 'd0c6e6d539369192106a55435af0b0e434d580bd4aa82f54feccc540236809cb',
};
const expectedTitles = { en: 'Style Packs', 'zh-CN': '风格包', 'zh-TW': '風格包', ja: 'スタイルパック', ko: '스타일 팩' };
const keys = Object.keys(styleEditorMessages.en).sort();
const placeholders = (text: string) => [...text.matchAll(/{{(.*?)}}/g)].map(m => m[1]).sort();
const dataBefore = JSON.stringify(defaults);
for (const locale of Object.keys(styleEditorMessages) as (keyof typeof styleEditorMessages)[]) {
  const messages = styleEditorMessages[locale];
  assert.deepEqual(Object.keys(messages).sort(), keys, `${locale}: complete editor resource coverage`);
  for (const key of keys as (keyof typeof messages)[]) {
    assert.ok(messages[key].trim(), `${locale}/${key}: nonempty`);
    assert.deepEqual(placeholders(messages[key]), placeholders(styleEditorMessages.en[key]), `${locale}/${key}: dynamic values preserved`);
  }
  const i18n = createInstance();
  i18n.init({ lng: locale, initAsync: false, interpolation: { escapeValue: false },
    resources: { [locale]: { translation: { styleEditor: messages, vocabPresetLabels: vocabPresetMessages[locale] } } } });
  const copy = styleEditorCopy(i18n.t);
  assert.equal(copy.title, expectedTitles[locale]);
  const ownedName = 'My {{HOTWORDS}} ${name} — $&';
  assert.ok(copy.builtinPackEditLabel(ownedName).includes(ownedName), 'authored name stays literal in messages');
  assert.ok(copy.exportSuccess('E:/My 文件/한글 日本語.zip').includes('E:/My 文件/한글 日本語.zip'));
  assert.ok(copy.loadFailed('ERR_42 {{name}}').includes('ERR_42 {{name}}'));
  assert.ok(copy.listCount(7).includes('7'));
  const draft = newStylePackDraft(i18n.t);
  assert.equal(draft.prompt.match(/{{HOTWORDS}}/g)?.length, 1, 'literal runtime hotword slot preserved');
  assert.ok(!draft.prompt.includes('[[') && !draft.name.includes('styleEditor.'));
  if (locale in expectedHashes) {
    assert.equal(createHash('sha256').update(draft.prompt).digest('hex'), expectedHashes[locale as keyof typeof expectedHashes], 'existing EN/CN new-prompt behavior unchanged');
  }
  for (const preset of defaults) {
    const label = vocabPresetLabel(preset, i18n.t);
    assert.equal(label, vocabPresetMessages[locale][preset.id as 'programmer' | 'chef' | 'civil-servant']);
    for (const changed of [
      { ...preset, id: 'user-custom' },
      { ...preset, name: ownedName },
      { ...preset, phrases: [...preset.phrases, 'User term'] },
      { ...preset, phrases: [...preset.phrases].reverse() },
    ]) assert.equal(vocabPresetLabel(changed, i18n.t), changed.name, 'authored/imported/overridden preset is never translated');
  }
  assert.equal(JSON.stringify(defaults), dataBefore, 'presentation never mutates stored vocabulary content');
}
console.log('style-editor localization: five locales, literal slots, legacy prompt hashes and authored preset preservation passed');
