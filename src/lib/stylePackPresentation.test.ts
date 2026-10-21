import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createInstance } from 'i18next';
import { stylePackPresentation } from './stylePackPresentation.ts';
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
  return {
    id: `builtin.${mode}`, name: canonical.name, description: canonical.description,
    kind: 'builtin', baseMode: mode, prompt: 'Saved prompt — 不改写提示词',
    examples: [{ input: '保留原始示例', output: 'Stored example' }],
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
      name: resource.style.builtinPacks[mode].name,
      description: resource.style.builtinPacks[mode].description,
      tags: [resource.style.builtinPacks[mode].tag0, resource.style.builtinPacks[mode].tag1],
    });
    assert.equal(JSON.stringify(pack), saved, 'presentation must not mutate stored fields or prompt');
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

  for (const pack of [
    { ...factory('light'), kind: 'imported' as const },
    { ...factory('light'), id: 'my-local-style' },
    { ...factory('light'), id: 'builtin.formal' },
  ]) {
    assert.deepEqual(stylePackPresentation(pack, i18n.t), { name: pack.name, description: pack.description, tags: pack.tags });
  }
}
console.log('PASS: five-locale factory metadata, custom/imported identity boundaries, and unchanged stored prompts/examples.');
