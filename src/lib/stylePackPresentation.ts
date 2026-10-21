import type { TFunction } from 'i18next';
import { zhCN } from '../i18n/zh-CN.ts';
import type { StylePack } from './types';

/** Localize only unchanged factory metadata, never the saved pack or its prompt. */
export function stylePackPresentation(pack: StylePack, t: TFunction) {
  const original = { name: pack.name, description: pack.description, tags: [...pack.tags] };
  if (pack.kind !== 'builtin' || pack.id !== `builtin.${pack.baseMode}`) return original;
  const canonical = zhCN.style.builtinPacks[pack.baseMode];
  if (!canonical) return original;
  const prefix = `style.builtinPacks.${pack.baseMode}`;
  return {
    name: pack.name === canonical.name ? t(`${prefix}.name`) : pack.name,
    description: pack.description === canonical.description ? t(`${prefix}.description`) : pack.description,
    tags: pack.tags.map(tag => {
      if (tag === canonical.tag0) return t(`${prefix}.tag0`);
      if (tag === canonical.tag1) return t(`${prefix}.tag1`);
      return tag;
    }),
  };
}
