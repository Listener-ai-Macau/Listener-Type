/** Explicit user-provided names/terms stay local and use their chosen spelling.
 * A missing, duplicated or altered token fails closed instead of silently
 * claiming that the name survived translation. */
export interface ProtectedTranslation { input: string; restore: (output: string) => string }
export function protectTranslationTerms(source: string, list: string, sourceLanguage: string = 'zh', markerOffset = 0): ProtectedTranslation {
  const mappings = new Map<string, string>();
  for (const line of list.split(/\r?\n/u).map(x => x.trim()).filter(Boolean)) {
    const match = line.match(/^(.*?)\s*(?:->|→)\s*(.*?)$/u);
    const term = (match ? match[1] : line).trim(), replacement = (match ? match[2] : line).trim();
    if (!term || !replacement || (mappings.has(term) && mappings.get(term) !== replacement))
      throw new Error('local_translation_protected_terms_failed');
    mappings.set(term, replacement);
  }
  const terms = [...mappings.keys()].sort((a, b) => b.length - a.length);
  if (!terms.length) return { input: source, restore: output => output };
  // The shipped en→zh tokenizer can turn rare Latin marker characters into
  // unknown glyphs; its tested numeric tokens survive. The opposite direction
  // may normalize digits, so it uses the tested Latin form instead.
  const numeric = sourceLanguage === 'en';
  const markerPattern = numeric ? /(?<!\d)98765\d{7}(?!\d)/gu : /ZXQKEEP\d+ZXQ/giu;
  if (markerPattern.test(source)) throw new Error('local_translation_protected_terms_failed');
  const entries: Array<{ marker: string; term: string; replacement: string }> = [];
  let input = '';
  for (let i = 0; i < source.length;) {
    const term = terms.find(value => source.startsWith(value, i)
      && (!/^[A-Za-z0-9_]/u.test(value) || i === 0 || !/[A-Za-z0-9_]/u.test(source[i - 1]))
      && (!/[A-Za-z0-9_]$/u.test(value) || i + value.length === source.length || !/[A-Za-z0-9_]/u.test(source[i + value.length])));
    if (!term) { input += source[i++]; continue; }
    // An odd, non-zero suffix avoids the tested model's removal of trailing
    // zeroes while remaining unique for inputs within the existing 4k limit.
    const marker = numeric ? `98765${1234567 + entries.length * 2}` : `ZXQKEEP${entries.length + markerOffset}ZXQ`;
    entries.push({ marker, term, replacement: mappings.get(term)! }); input += marker; i += term.length;
  }
  return { input, restore: output => {
    const seen = output.match(markerPattern) ?? [];
    if (seen.length !== entries.length || entries.some(({ marker }) => seen.filter(value => value === marker).length !== 1))
      throw new Error('local_translation_protected_terms_failed');
    // A model can preserve a numeric token but treat it as an ordinal (e.g.
    // adding 号 to a person's name). Do not silently attach that classifier.
    if (numeric && entries.some(({ marker, term }) => !/^\d+$/u.test(term)
      && /^[ \t]*(?:号|號|个|個|元|项|項|位|名|台|次|年|月|日|岁|歲)/u.test(output.slice(output.indexOf(marker) + marker.length))))
      throw new Error('local_translation_protected_terms_failed');
    const replacements = new Map(entries.map(({ marker, replacement }) => [marker, replacement]));
    return output.replace(markerPattern, marker => replacements.get(marker)!);
  } };
}
