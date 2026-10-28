export type TranslationEngine = 'local' | 'cloud';
export type LocalTranslationQuality = 'standard' | 'enhanced';

// Stored/native values remain unchanged; only presentation follows the UI locale.
export function translationLanguageLabel(language: string, locale: string): string {
  const codes: Record<string, string> = {
    '简体中文': 'zh-Hans', '繁体中文': 'zh-Hant', English: 'en', '日本語': 'ja', '한국어': 'ko',
    Français: 'fr', Deutsch: 'de', Español: 'es', Italiano: 'it', Português: 'pt',
    Русский: 'ru', 'العربية': 'ar', 'Tiếng Việt': 'vi', 'ไทย': 'th', 'हिन्दी': 'hi',
  };
  const code = codes[language] ?? (language === 'zh' || language === 'en' ? language : undefined);
  if (!code) return language;
  try { return new Intl.DisplayNames([locale], { type: 'language' }).of(code) ?? language; }
  catch { return language; }
}

export function localTargetCode(language: string): string {
  if (language === 'English' || language === 'en') return 'en';
  if (['简体中文', '繁体中文', 'zh'].includes(language)) return 'zh';
  return '';
}

// This is a script hint, not language identification: Han also occurs in
// Japanese. Never change the selected source or start/download a task here.
export function canSuggestChineseSource(text: string, target: string): boolean {
  return localTargetCode(target) === 'en' && /\p{Script=Han}/u.test(text)
    && !/[\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(text);
}

export function localTranslationKey(source: string, target: string, quality: LocalTranslationQuality = 'standard'): string {
  return JSON.stringify([source, target, quality]);
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
  if (value.includes('memory_low')) return 'memoryLow';
  if (value.includes('context_limit') || value.includes('output_limit')) return 'capacity';
  if (value.includes('_fidelity')) return 'fidelity';
  if (value.includes('invalid_text')) return 'invalidText';
  return 'failed';
}
