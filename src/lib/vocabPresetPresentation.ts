import type { TFunction } from 'i18next';
import type { VocabPreset } from './types';
import defaultPresetsJson from './vocab-presets.json' with { type: 'json' };

/** Display only: an edited name or phrase list belongs to its author. */
export function vocabPresetLabel(preset: VocabPreset, t: TFunction): string {
  const factory = (defaultPresetsJson as VocabPreset[]).find(item => item.id === preset.id);
  if (!factory || factory.name !== preset.name || factory.phrases.length !== preset.phrases.length
    || !factory.phrases.every((phrase, index) => phrase === preset.phrases[index])) return preset.name;
  return t(`vocabPresetLabels.${factory.id}`);
}
