import type { StylePack } from './types';

/** A session-only workspace descriptor, never a saved or active style pack. */
export function translationTrialPack(language: string, name: string, description: string): StylePack {
  return { id: `translation:${language}`, name, description, version: '1', kind: 'imported',
    baseMode: 'formal', prompt: language, examples: [], tags: [], enabled: true, active: false };
}
