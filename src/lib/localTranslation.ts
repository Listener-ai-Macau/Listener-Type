export type TranslationEngine = 'local' | 'cloud';

export function localTargetCode(language: string): string {
  if (language === 'English' || language === 'en') return 'en';
  if (['简体中文', '繁体中文', 'zh'].includes(language)) return 'zh';
  return '';
}

export function localTranslationKey(source: string, target: string): string {
  return JSON.stringify([source, target]);
}

export type LocalTranslationReadiness = { key: string; ready: boolean };

export function localTranslationErrorKey(error: unknown): string {
  const value = String(error instanceof Error ? error.message : error);
  if (value.includes('cancelled')) return 'cancelled';
  if (value.includes('unsupported_pair')) return 'unsupported';
  if (value.includes('not_ready') || value.includes('checksum') || value.includes('incomplete_model')) return 'notReady';
  if (value.includes('download')) return 'downloadFailed';
  if (value.includes('busy') || value.includes('preparing')) return 'busy';
  if (value.includes('timeout')) return 'timeout';
  if (value.includes('invalid_text')) return 'invalidText';
  return 'failed';
}
