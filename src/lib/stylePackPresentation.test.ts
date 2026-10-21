import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createInstance } from 'i18next';
import { stylePackExamplePresentation, stylePackPresentation } from './stylePackPresentation.ts';
import { zhCN } from '../i18n/zh-CN.ts';
import { zhTW } from '../i18n/zh-TW.ts';
import { en } from '../i18n/en.ts';
import { ja } from '../i18n/ja.ts';
import { ko } from '../i18n/ko.ts';
import type { StylePack, PolishMode } from './types';

const locales = { 'zh-CN': zhCN, 'zh-TW': zhTW, en, ja, ko };
const modes: PolishMode[] = ['raw', 'light', 'structured', 'formal'];
const rust = readFileSync(new URL('../../src-tauri/src/types.rs', import.meta.url), 'utf8');

// Protect the actual persisted factory boundary, rather than assuming all builtin
// names are untouched or translating user content just because its ID matches.
function factory(mode: PolishMode): StylePack {
  const variant = mode[0].toUpperCase() + mode.slice(1);
  const body = rust.split(`PolishMode::${variant} => StylePack {`)[1].split('\n        },')[0];
  const canonical = zhCN.style.builtinPacks[mode];
  assert.equal(body.match(/\n\s*name: "([^"]*)"/)?.[1], canonical.name);
  assert.equal(body.match(/\n\s*description: "([^"]*)"/)?.[1], canonical.description);
  const tags = [...body.match(/tags: vec!\[([^\]]*)\]/)![1].matchAll(/"([^"]*)"\.into\(\)/g)].map(m => m[1]);
  assert.deepEqual(tags, [canonical.tag0, canonical.tag1]);
  const example = {
    title: JSON.parse('"' + body.match(/title: Some\("((?:\\.|[^"\\])*)"/)![1] + '"'),
    input: JSON.parse('"' + body.match(/\n\s*input: "((?:\\.|[^"\\])*)"/)![1] + '"'),
    output: JSON.parse('"' + body.match(/\n\s*output: "((?:\\.|[^"\\])*)"/)![1] + '"'),
  };
  assert.deepEqual(example, canonical.example, 'display examples must match actual saved factory examples');
  return {
    id: `builtin.${mode}`, name: canonical.name, description: canonical.description,
    kind: 'builtin', baseMode: mode, prompt: 'Saved prompt — 不改写提示词',
    examples: [example],
    tags, enabled: true, active: mode === 'raw', version: '1.0.0',
  };
}

for (const [locale, resource] of Object.entries(locales)) {
  const i18n = createInstance();
  await i18n.init({ lng: locale, fallbackLng: 'zh-CN', initAsync: false, resources: { [locale]: { translation: resource } } });
  for (const mode of modes) {
    const pack = factory(mode);
    const saved = JSON.stringify(pack);
    const view = stylePackPresentation(pack, i18n.t);
    assert.deepEqual(view, {
      name: mode === 'structured' ? resource.style.builtinDisplayNames.structured : resource.style.builtinPacks[mode].name,
      description: resource.style.builtinPacks[mode].description,
      tags: [resource.style.builtinPacks[mode].tag0, resource.style.builtinPacks[mode].tag1],
    });
    assert.equal(JSON.stringify(pack), saved, 'presentation must not mutate stored fields or prompt');
    assert.deepEqual(stylePackExamplePresentation(pack, pack.examples[0], i18n.t), resource.style.builtinPacks[mode].example);
    for (const changed of [
      { ...pack.examples[0], input: 'My own edited input' },
      { ...pack.examples[0], output: 'My own edited output' },
      { ...pack.examples[0], title: 'My own title' },
    ]) {
      assert.deepEqual(stylePackExamplePresentation(pack, changed, i18n.t), changed, 'edited pairs stay together without mixing translated factory output');
    }
    assert.equal(JSON.stringify(pack), saved, 'viewing examples must not mutate a saved prompt or pair');
  }

  const custom = factory('light');
  custom.name = 'My custom name'; custom.description = '客户自己写的说明';
  custom.tags = ['Personal tag', zhCN.style.builtinPacks.light.tag1];
  const customSaved = JSON.stringify(custom);
  assert.deepEqual(stylePackPresentation(custom, i18n.t), {
    name: custom.name, description: custom.description,
    tags: ['Personal tag', resource.style.builtinPacks.light.tag1],
  });
  assert.equal(JSON.stringify(custom), customSaved);

  const technical = factory('structured');
  assert.equal(technical.name, '清晰结构', 'legacy stored factory name remains unchanged');
  technical.name = 'My technical workflow';
  const technicalSaved = JSON.stringify(technical);
  assert.equal(stylePackPresentation(technical, i18n.t).name, technical.name, 'a customer-renamed builtin remains theirs');
  assert.equal(JSON.stringify(technical), technicalSaved);
  const imported = { ...factory('structured'), kind: 'imported' as const };
  assert.equal(stylePackPresentation(imported, i18n.t).name, imported.name, 'an imported canonical-looking name is not relabelled');

  for (const pack of [
    { ...factory('light'), kind: 'imported' as const },
    { ...factory('light'), id: 'my-local-style' },
    { ...factory('light'), id: 'builtin.formal' },
  ]) {
    assert.deepEqual(stylePackPresentation(pack, i18n.t), { name: pack.name, description: pack.description, tags: pack.tags });
    assert.deepEqual(stylePackExamplePresentation(pack, pack.examples[0], i18n.t), pack.examples[0]);
  }
}
console.log('PASS: five-locale factory metadata/examples match Rust defaults; edited/imported pairs stay intact and saved prompts/examples are unchanged.');
