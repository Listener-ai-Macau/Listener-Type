export interface PoliteEdit { old: string; new: string }
export interface PoliteEditReview { edits: PoliteEdit[]; skipped: number }
export interface LocalPoliteOption extends PoliteEdit { tone: 'courteous' | 'softer' }

/** A deliberately narrow, local English option. Only the request prefix and final
 * punctuation change; the request body is copied exactly. This is not a general
 * grammar or multilingual rewriter. Applying recomputes the option for current text.
 */
export function localPoliteOptions(source: string): LocalPoliteOption[] {
  if (!source.trim() || source.length > 4000) return [];
  const Segmenter = (Intl as unknown as { Segmenter?: new (locale: string, options: { granularity: string }) => {
    segment(text: string): Iterable<{ segment: string }>;
  } }).Segmenter;
  if (!Segmenter) return [];
  const edits: LocalPoliteOption[] = [];
  for (const { segment } of new Segmenter('en', { granularity: 'sentence' }).segment(source)) {
    const sentence = segment.trim();
    if (sentence.length > 240 || /[\r\n`]|https?:|www\.|[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}/iu.test(sentence)) continue;
    // Find the request span rather than requiring the whole sentence to begin
    // with it. Greetings, addressees and preceding conditions stay outside it.
    const question = /\b(?:could|can|would|will)[ \t]+(you|I|we)[ \t]+(?:please[ \t]+)?(send|share|check|review|confirm|provide|bring|update|let|tell|give|take|return|submit|forward|attach|move|change|reschedule|postpone|meet|join|schedule|use|book|cancel)[ \t]+([^?!]+?)\?$/iu.exec(sentence);
    const imperative = /(?:^|,[ \t]+)((?:(?:please|kindly)[ \t]+)?(send|share|check|review|confirm|provide|bring|update|let|tell|give|take|return|submit|forward|attach)[ \t]+([^?!]+?)[.!]?)$/iu.exec(sentence);
    if (!question && !imperative) continue;
    const old = question?.[0] ?? imperative![1];
    const subject = question?.[1].toLowerCase() === 'i' ? 'I' : question?.[1].toLowerCase() ?? 'you';
    const verb = question?.[2] ?? imperative![2];
    const object = question?.[3] ?? imperative![3];
    // Turning an imperative into a question is safe only within this narrow
    // simple-request grammar. A comma or coordination can start a promise,
    // condition or another speaker's clause; copying it byte-for-byte and
    // changing the final punctuation still changes its grammatical role.
    // Decline ambiguous compounds rather than pretending this is a parser.
    // Existing questions keep their original question scope.
    if (!question && (/[,;—–]|:(?!\d{2}\b)/u.test(object)
      || /\b(?:and|or|but|so|yet|then)[ \t]+(?:I|you|we|he|she|it|they|there)\b/iu.test(object))) continue;
    if (/\b(?:Mr|Mrs|Ms|Dr|Prof|Sr|Jr|etc|[A-Z])\.$/u.test(old)) continue;
    if (source.indexOf(old) !== source.lastIndexOf(old)) continue;
    const body = `${verb.toLowerCase()} ${object}`;
    const choices: { tone: LocalPoliteOption['tone']; prefix: string }[] = [
      { tone: 'courteous', prefix: `Could ${subject} please` },
      { tone: 'softer', prefix: `Would ${subject} be able to` },
    ];
    for (const { tone, prefix } of choices) {
      const caseMatchedPrefix = /^[a-z]/u.test(old) ? prefix[0].toLowerCase() + prefix.slice(1) : prefix;
      const replacement = `${caseMatchedPrefix} ${body}?`;
      if (old === replacement) continue;
      const proposed = source.replace(old, () => replacement);
      if (same(anchors(source), anchors(proposed)) && same(markers(source), markers(proposed))) edits.push({ old, new: replacement, tone });
    }
  }
  return edits;
}

/** The whole single-sentence draft is permitted only for a recomputed local option,
 * never by relaxing the checks on untrusted service proposals. */
export function applyLocalPoliteOption(source: string, selected: PoliteEdit): string {
  const valid = localPoliteOptions(source).some(edit => edit.old === selected.old && edit.new === selected.new);
  if (!valid) throw new Error('polite_edits_invalid_response');
  return source.replace(selected.old, () => selected.new);
}

function markers(text: string): string[] {
  return (text.toLowerCase().match(/\b(?:must|required|mandatory|shall|not|never|cannot|can't|won't|may|might|possibly|perhaps|if|unless|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b|必须|务必|不得|禁止|不能|不要|别|可能|也许|或许|如果|除非|不|(?:星期|周)[一二三四五六日天]|必ず|必要|してはいけ|かもしれ|できない|ない|なら|[月火水木金土日]曜日?|반드시|필수|금지|않|못|가능|[월화수목금토일]요일/gu) ?? []).sort();
}
function anchors(text: string): string[] {
  return text.match(/https?:\/\/[^\s<>]+|[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}|`[^`]+`|\p{N}+(?::\p{N}+)?(?:\s?[ap]\.?m\.?)?|\b(?:zero|one|two|three|four|five|six|seven|eight|nine|ten)\b/giu) ?? [];
}
function same(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index]);
}

/** Untrusted service proposals. Exact spans and factual/modal checks precede display.
 * These checks are limited; users still review meaning/names before choosing edits.
 */
export function reviewPoliteEdits(source: string, json: string): PoliteEditReview {
  const value: unknown = JSON.parse(json);
  if (!value || typeof value !== 'object' || !('edits' in value) || !Array.isArray(value.edits) || value.edits.length > 24) {
    throw new Error('polite_edits_invalid_response');
  }
  const spans: { start: number; end: number }[] = [];
  const edits: PoliteEdit[] = [];
  let skipped = 0;
  for (const candidate of value.edits) {
    if (!candidate || typeof candidate.old !== 'string' || typeof candidate.new !== 'string') {
      throw new Error('polite_edits_invalid_response');
    }
    let old: string = candidate.old;
    const replacement: string = candidate.new;
    const start = source.indexOf(old);
    if (!old || Array.from(old).length > 80 || Array.from(replacement).length > 240 || old === source || start < 0 || start !== source.lastIndexOf(old)) {
      throw new Error('polite_edits_invalid_response');
    }
    let end = start + old.length;
    // Providers sometimes omit the original full stop from the selected span
    // while adding a question mark. Include that adjacent sentence terminator
    // in the visible proposed edit, so accepting it cannot produce "?.".
    if (/[?!？！]\s*$/u.test(replacement) && /[.。]/u.test(source[end] ?? '')
      && (end + 1 === source.length || /\s/u.test(source[end + 1]))) {
      const expanded = old + source[end];
      if (expanded === source || Array.from(expanded).length > 80 || source.indexOf(expanded) !== start || source.lastIndexOf(expanded) !== start) {
        skipped += 1; continue;
      }
      old = expanded; end += 1;
    }
    if (spans.some(span => start < span.end && end > span.start)) throw new Error('polite_edits_invalid_response');
    spans.push({ start, end });
    if (!replacement.trim()) throw new Error('polite_edits_invalid_response');
    if (old === replacement) continue;
    const proposed = source.slice(0, start) + replacement + source.slice(end);
    if (!same(anchors(source), anchors(proposed)) || !same(markers(source), markers(proposed))) {
      skipped += 1; continue;
    }
    edits.push({ old, new: replacement });
  }
  return { edits, skipped };
}

/** Apply only the chosen exact edits to the same current text, in one local update. */
export function applyPoliteEdits(source: string, selected: PoliteEdit[]): string {
  const reviewed = reviewPoliteEdits(source, JSON.stringify({ edits: selected }));
  if (reviewed.skipped || reviewed.edits.length !== selected.length) throw new Error('polite_edits_invalid_response');
  return reviewed.edits.map(edit => ({ ...edit, start: source.indexOf(edit.old) }))
    .sort((a, b) => b.start - a.start)
    .reduce((text, edit) => text.slice(0, edit.start) + edit.new + text.slice(edit.start + edit.old.length), source);
}
