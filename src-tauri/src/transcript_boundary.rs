//! Shared content coordinates for provisional transcripts and editor delivery.

pub(crate) fn is_decorative(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。' | '、' | '；' | '：' | '？' | '！' | '“' | '”' | '‘' | '’'
                | '（' | '）' | '【' | '】' | '《' | '》' | '…' | '—'
        )
}

pub(crate) fn content_key(text: &str) -> String {
    text.chars()
        .filter(|ch| !is_decorative(*ch))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Map a rendered prefix back through the actual deterministic display policy.
/// Removing control/filler words or expanding a correction must not make the
/// display string a provider coordinate. Only a complete matching rendering can
/// consume source; an output boundary inside an expansion has no raw boundary.
pub(crate) fn source_prefix_for_rendered_key<'a>(
    source: &'a str,
    rendered_key: &str,
    mut render: impl FnMut(&str) -> Option<String>,
) -> Option<&'a str> {
    if rendered_key.is_empty() { return None; }
    let mut matched = None;
    for end in source.char_indices().map(|(at, ch)| at + ch.len_utf8()) {
        if render(&source[..end]).is_some_and(|text| content_key(&text) == rendered_key) {
            // Removed fill or decoration may follow the visible boundary.
            // The furthest exact rendering is its complete source coordinate.
            matched = Some(&source[..end]);
        }
    }
    matched
}

/// Provider word coordinates are evidence about the retained audio, not a
/// speaker verdict. Only a contiguous, text-aligned prefix can be classified
/// as activation context; missing or conflicting coordinates prove nothing.
#[derive(Clone, Debug, Default)]
pub(crate) struct TimedTranscript {
    pub text: String,
    pub words: Vec<TimedTranscriptWord>,
}

#[derive(Clone, Debug)]
pub(crate) struct TimedTranscriptWord {
    pub text: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

fn native_word_needs_space(left: char, right: char) -> bool {
    // Provider words in scripts without ordinary inter-word spaces join
    // directly. Preserve whitespace already present in each native token.
    let unspaced = |ch: char| matches!(ch as u32,
        0x0e00..=0x0eff | 0x1000..=0x109f | 0x1780..=0x17ff |
        0x2e80..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x3134f);
    left.is_alphanumeric() && right.is_alphanumeric() && !unspaced(left) && !unspaced(right)
}

/// Native tokens are a second rendering of the same sealed response. They
/// retain ITN spellings (e.g. 350) even when the response's display text uses
/// spoken number names. Invalid or overlapping occurrences cannot edit text.
impl TimedTranscript {
    pub(crate) fn native_word_view(&self) -> Option<Self> {
        let mut text = String::new();
        let mut previous_end = None;
        for word in &self.words {
            let (start, end) = (word.start_ms?, word.end_ms?);
            if start >= end || previous_end.is_some_and(|prior| start < prior) {
                return None;
            }
            previous_end = Some(end);
            let space = text.chars().next_back().zip(word.text.chars().next())
                .is_some_and(|(a, b)| native_word_needs_space(a, b));
            if space { text.push(' '); }
            text.push_str(&word.text);
        }
        (!text.is_empty()).then(|| Self { text, words: self.words.clone() })
    }

    /// Keep the selected response's punctuation only when every native word
    /// maps to that exact rendering. A divergent ITN/spelling view still uses
    /// native tokens; formatting cannot grant another word source authority.
    pub(crate) fn native_word_rendering_view(&self) -> Option<Self> {
        let mut view = self.native_word_view()?;
        if self.strictly_aligned_word_ranges().is_some() {
            view.text = self.text.clone();
        }
        Some(view)
    }
}

fn uncovered_source_decoration<'a>(source: &'a str, delivered_prefix: &str) -> &'a str {
    let boundary_start = delivered_prefix.char_indices().rev()
        .find(|(_, ch)| !is_decorative(*ch)).map(|(at, ch)| at + ch.len_utf8()).unwrap_or(0);
    let boundary = &delivered_prefix[boundary_start..];
    let covered = source.char_indices().map(|(at, ch)| at + ch.len_utf8())
        .filter(|&end| boundary.ends_with(&source[..end])).max().unwrap_or(0);
    if covered > 0 { return &source[covered..]; }
    // A previously delivered terminator is immutable even if the provider
    // later chooses a comma. Keep new opening quotes/currency/operators.
    let separator = |ch| matches!(ch, '，'|'。'|'、'|'；'|'：'|'？'|'！'|','|'.'|';'|':'|'?'|'!');
    if boundary.chars().any(separator) {
        source.trim_start_matches(separator)
    } else { source }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FinalSourceDelivery {
    /// The same submitted acoustic occurrence covers every sealed native word.
    /// Keep committed bytes; a different provider rendering cannot invent a tail.
    Covered,
    /// No existing byte changes. Submit only unseen words at the current cursor.
    Append { suffix: String },
    /// An internal omission requires a fresh witness of the owned editor range.
    SupplementOwnedRange { body: String },
}

/// A final source can prove that no words remain as well as prove a supplement.
/// The caller binds this boundary to the same capture; spelling alone must not
/// consume a later repetition of an identical word or clause.
pub(crate) fn final_delivery_after_consumption(
    delivered: &str,
    native: &TimedTranscript,
    body: &str,
    boundary: Option<&TimedDeliveryBoundary>,
) -> Option<FinalSourceDelivery> {
    if let (Some(view), Some(boundary)) = (native.native_word_rendering_view(), boundary) {
        let old = content_key(delivered);
        if !old.is_empty() && old == content_key(body)
            && view.body_suffix_after_delivery(body, boundary)
                .is_some_and(|suffix| content_key(suffix).is_empty())
        {
            return Some(FinalSourceDelivery::Covered);
        }
    }
    lossless_final_delivery(delivered, native, body)
}

/// Recognition evidence describes missing source words; it does not require
/// an editor replacement when all of those words follow the committed body.
pub(crate) fn lossless_final_delivery(delivered: &str, native: &TimedTranscript,
    body: &str) -> Option<FinalSourceDelivery> {
    let supplemented = lossless_final_supplement(delivered, native, body)?;
    match supplemented.strip_prefix(delivered) {
        Some(suffix) => Some(FinalSourceDelivery::Append { suffix: suffix.to_string() }),
        None => Some(FinalSourceDelivery::SupplementOwnedRange { body: supplemented }),
    }
}

/// Supplement a sealed source without deleting, substituting, reordering, or
/// reformatting any delivered character. A unique subsequence embedding is
/// required: repeated content cannot guess which occurrence the editor owns.
/// Every added interval must consist of whole, ordered native source words.
/// `body` may remove only a leading activation prefix from `native.text`.
pub(crate) fn lossless_final_supplement(delivered: &str, native: &TimedTranscript, body: &str) -> Option<String> {
    let view = native.native_word_rendering_view()?;
    let all = content_key(&view.text);
    let source = content_key(body);
    let old = content_key(delivered);
    if old.is_empty() || source.len() <= old.len() || !all.ends_with(&source) { return None; }
    let removed = all.len() - source.len();
    let rendered = &view.text;
    // Keys prove word identity; these byte coordinates retain the provider's
    // actual rendering of newly added words, including commas and final marks.
    let mut offsets = Vec::new();
    let mut rendered_key_bytes = 0;
    for (at, ch) in rendered.char_indices().filter(|(_, ch)| !is_decorative(*ch)) {
        for lower in ch.to_lowercase() {
            offsets.push((rendered_key_bytes, at, at + ch.len_utf8()));
            rendered_key_bytes += lower.len_utf8();
        }
    }
    let mut words = Vec::new();
    let mut key_offset = 0;
    let mut prior_word_end = 0;
    for word in &view.words {
        let key = content_key(&word.text);
        if key.is_empty() { continue; }
        let through = key_offset + key.len();
        let first = offsets.iter().position(|offset| offset.0 == key_offset)?;
        let after = offsets.iter().position(|offset| offset.0 == through).unwrap_or(offsets.len());
        if first > 0 && offsets[first - 1].1 == offsets[first].1
            || after < offsets.len() && after > 0 && offsets[after - 1].1 == offsets[after].1
        { return None; }
        let from = offsets[first].1;
        let end = offsets.get(after).map(|offset| offset.1).unwrap_or(rendered.len());
        if key_offset >= removed {
            let leading_from = if key_offset == removed {
                rendered.strip_suffix(body).map(str::len).unwrap_or(from)
            } else { prior_word_end };
            words.push((key, from, end, leading_from));
        }
        if key_offset < removed && through > removed { return None; }
        prior_word_end = offsets.get(after.checked_sub(1)?)?.2;
        key_offset = through;
    }
    // Match complete native tokens, not individual letters. "the file" in
    // "the red file" has ambiguous letter embeddings but one word embedding.
    // Counts saturate at two; immutable path nodes recover only a unique match.
    let mut states = std::collections::BTreeMap::from([(0usize, (1u8, None::<usize>))]);
    let mut nodes: Vec<(Option<usize>, usize)> = Vec::new();
    for (index, (key, _, _, _)) in words.iter().enumerate() {
        let previous: Vec<_> = states.iter().map(|(&offset, &path)| (offset, path)).collect();
        for (offset, (ways, path)) in previous {
            if !old[offset..].starts_with(key) { continue; }
            let next = offset + key.len();
            let entry = states.entry(next).or_insert((0, None));
            if entry.0 == 0 && ways == 1 {
                nodes.push((path, index));
                entry.1 = Some(nodes.len()-1);
            } else { entry.1 = None; }
            entry.0 = (entry.0 + ways).min(2);
        }
        if states.get(&old.len()).is_some_and(|v| v.0 > 1) { return None; }
    }
    let &(1, mut path) = states.get(&old.len())? else { return None; };
    let mut matched = vec![false; words.len()];
    while let Some(node) = path {
        matched[nodes[node].1] = true;
        path = nodes[node].0;
    }
    let mut old_offsets = Vec::new();
    let mut old_key_bytes = 0;
    for (byte, ch) in delivered.char_indices().filter(|(_,c)| !is_decorative(*c)) {
        for lower in ch.to_lowercase() {
            old_offsets.push((old_key_bytes, byte));
            old_key_bytes += lower.len_utf8();
        }
    }
    let mut insertions = Vec::new();
    let mut old_offset = 0;
    let mut run = None;
    for index in 0..=words.len() {
        if index < words.len() && !matched[index] {
            run.get_or_insert(index);
            continue;
        }
        if let Some(from) = run.take() {
            let at = if old_offset == old.len() { delivered.len() } else {
                let i = old_offsets.iter().position(|v| v.0 == old_offset)?;
                if i > 0 && old_offsets[i-1].1 == old_offsets[i].1 { return None; }
                old_offsets[i].1
            };
            let leading = uncovered_source_decoration(
                &rendered[words[from].3..words[from].1], &delivered[..at]);
            let addition = format!("{leading}{}", &rendered[words[from].1..words[index-1].2]);
            insertions.push((at,addition));
        }
        if index < words.len() { old_offset += words[index].0.len(); }
    }
    let mut result = delivered.to_string();
    for (at, mut addition) in insertions.into_iter().rev() {
        if result[..at].chars().next_back().zip(addition.chars().next())
            .is_some_and(|(a,b)| native_word_needs_space(a,b)) { addition.insert(0,' '); }
        if addition.chars().next_back().zip(result[at..].chars().next())
            .is_some_and(|(a,b)| native_word_needs_space(a,b)) { addition.push(' '); }
        result.insert_str(at,&addition);
    }
    (content_key(&result) == source).then_some(result)
}

/// The last source word actually submitted to the editor. Text alone can be
/// revised; its occurrence in the retained PCM identifies the delivery seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimedDeliveryBoundary {
    key: String,
    start_ms: u64,
    end_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct AlignedTimedWord {
    word_index: usize,
    from: usize,
    through: usize,
    start_ms: u64,
    end_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ControlPrefixEvidence {
    pub source_key: String,
    pub control_key: String,
}

/// A partial rendering of the already locally confirmed activation. Across
/// provider revisions, only the same leading source seam and ordered PCM
/// spans may supply the missing phrase component.
#[derive(Clone, Debug)]
pub(crate) struct ActivationFragment {
    pub lead_key: String,
    pub phrase_part: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl TimedTranscript {
    fn aligned_word_ranges(&self) -> Vec<AlignedTimedWord> {
        let source = content_key(&self.text);
        let mut offset = 0;
        let mut ranges = Vec::new();
        let mut previous = (0, 0);
        for (word_index, word) in self.words.iter().enumerate() {
            let key = content_key(&word.text);
            if key.is_empty() { continue; }
            if !source[offset..].starts_with(&key) { break; }
            let (Some(start), Some(end)) = (word.start_ms, word.end_ms) else { break; };
            if start >= end || start < previous.0 || end < previous.1 { break; }
            ranges.push(AlignedTimedWord {
                word_index,
                from: offset,
                through: offset + key.len(),
                start_ms: start,
                end_ms: end,
            });
            offset += key.len();
            previous = (start, end);
        }
        ranges
    }

    /// A snapshot continuation is an irreversible source coordinate, so its
    /// complete content and every provider token must have a valid,
    /// non-overlapping timestamp. This is deliberately stricter than the
    /// partial timing accepted by preview-only helpers.
    fn strictly_aligned_word_ranges(&self) -> Option<Vec<AlignedTimedWord>> {
        let source = content_key(&self.text);
        let mut offset = 0;
        let mut previous_end = None;
        let mut ranges = Vec::new();
        for (word_index, word) in self.words.iter().enumerate() {
            let (Some(start_ms), Some(end_ms)) = (word.start_ms, word.end_ms) else {
                return None;
            };
            if start_ms >= end_ms || previous_end.is_some_and(|end| start_ms < end) {
                return None;
            }
            let key = content_key(&word.text);
            if !key.is_empty() {
                if !source.get(offset..)?.starts_with(&key) {
                    return None;
                }
                ranges.push(AlignedTimedWord {
                    word_index,
                    from: offset,
                    through: offset + key.len(),
                    start_ms,
                    end_ms,
                });
                offset += key.len();
            }
            previous_end = Some(end_ms);
        }
        (offset == source.len()).then_some(ranges)
    }

    /// Locate the unique whole-token occurrence of a previously submitted
    /// provider word that contains the same retained-PCM midpoint. Text alone
    /// cannot select between repeated words or a later acoustic occurrence.
    fn unique_delivery_seam(
        &self,
        boundary: &TimedDeliveryBoundary,
        ranges: &[AlignedTimedWord],
    ) -> Option<(usize, usize, u64)> {
        if boundary.key.is_empty() || boundary.start_ms >= boundary.end_ms {
            return None;
        }
        let source = content_key(&self.text);
        let midpoint = boundary.start_ms + (boundary.end_ms - boundary.start_ms) / 2;
        let mut seams = source.match_indices(&boundary.key).filter_map(|(from, _)| {
            let through = from + boundary.key.len();
            let first = ranges.iter().find(|row| row.from == from)?;
            let last = ranges.iter().find(|row| row.through == through)?;
            (first.word_index <= last.word_index
                && first.start_ms <= midpoint
                && midpoint < last.end_ms)
                .then_some((through, last.word_index, last.end_ms))
        });
        let seam = seams.next()?;
        if seams.next().is_some() { return None; }
        Some(seam)
    }

    pub(crate) fn delivery_boundary(&self, consumed_body_key: &str) -> Option<TimedDeliveryBoundary> {
        if consumed_body_key.is_empty() { return None; }
        let source = content_key(&self.text);
        let mut occurrences = source.match_indices(consumed_body_key);
        let (start, _) = occurrences.next()?;
        if occurrences.next().is_some() { return None; }
        let end = start + consumed_body_key.len();
        let ranges = self.aligned_word_ranges();
        let row = ranges.iter().find(|row| row.through == end)?;
        if row.from < start { return None; }
        Some(TimedDeliveryBoundary {
            key: source[row.from..row.through].into(),
            start_ms: row.start_ms,
            end_ms: row.end_ms,
        })
    }

    /// Locate a submitted word in a later owner-approved snapshot. Retokenizing
    /// it is allowed; a different occurrence, absent timing or ambiguous match
    /// cannot consume text. This maps a seam, never certifies speaker identity.
    pub(crate) fn body_suffix_after_delivery<'a>(&self, body: &'a str, boundary: &TimedDeliveryBoundary) -> Option<&'a str> {
        let source = content_key(&self.text);
        let body_key = content_key(body);
        if body_key.is_empty() { return None; }
        let mut bodies = source.match_indices(&body_key);
        let (body_start, _) = bodies.next()?;
        if bodies.next().is_some() { return None; }
        let body_end = body_start + body_key.len();
        let ranges = self.aligned_word_ranges();
        let (through, _, _) = self.unique_delivery_seam(boundary, &ranges)?;
        let seam_start = through.checked_sub(boundary.key.len())?;
        if seam_start < body_start || through > body_end { return None; }
        let offset = consumed_prefix_end(body, &source[body_start..through])?;
        Some(&body[offset..])
    }

    /// Return only complete child-decoder tokens after a submitted audio seam.
    /// The returned timing belongs entirely to this transcript; parent and
    /// child decoder coordinates are never concatenated.
    pub(crate) fn timed_suffix_after_delivery(
        &self,
        boundary: &TimedDeliveryBoundary,
    ) -> Option<TimedTranscript> {
        let ranges = self.strictly_aligned_word_ranges()?;
        let source = content_key(&self.text);
        let (through, last_word_index, seam_end_ms) =
            self.unique_delivery_seam(boundary, &ranges)?;
        let text_offset = consumed_prefix_end(&self.text, &source[..through])?;
        let suffix_text = self.text[text_offset..].to_string();
        if content_key(&suffix_text).is_empty() {
            return Some(TimedTranscript::default());
        }
        let first = ranges.iter().find(|row| row.from == through)?;
        if first.word_index <= last_word_index || first.start_ms < seam_end_ms {
            return None;
        }
        Some(TimedTranscript {
            text: suffix_text,
            words: self.words[first.word_index..].to_vec(),
        })
    }

    /// Convert decoder-local word coordinates for one retained PCM clip into
    /// the parent stream's global coordinate. Every token must be timed and
    /// wholly contained in the clip; overflow or partial coverage is rejected.
    pub(crate) fn shifted_from_clip(
        &self,
        origin_ms: u64,
        end_ms: u64,
    ) -> Option<TimedTranscript> {
        let clip_duration_ms = end_ms.checked_sub(origin_ms)?;
        self.strictly_aligned_word_ranges()?;
        let mut shifted = self.clone();
        for word in &mut shifted.words {
            let (Some(start_ms), Some(word_end_ms)) = (word.start_ms, word.end_ms) else {
                return None;
            };
            if word_end_ms > clip_duration_ms {
                return None;
            }
            let shifted_start = origin_ms.checked_add(start_ms)?;
            let shifted_end = origin_ms.checked_add(word_end_ms)?;
            if shifted_end > end_ms {
                return None;
            }
            word.start_ms = Some(shifted_start);
            word.end_ms = Some(shifted_end);
        }
        Some(shifted)
    }

    /// Admit only a contiguous revision prefix whose new words are covered by
    /// positive owner PCM intervals. Previously admitted words remain owned;
    /// a later positive sample cannot authorize an earlier unobserved gap.
    pub(crate) fn owner_supported_prefix(&self, accepted_key: &str, intervals: &[(u64, u64)], foreign_intervals: &[(u64, u64)]) -> String {
        let source_key = content_key(&self.text);
        let accepted_len = if source_key.starts_with(accepted_key) { accepted_key.len() } else { 0 };
        let mut covered_key = String::new();
        let mut previous_end = None;
        for word in &self.words {
            let key = content_key(&word.text);
            if key.is_empty() { continue; }
            if !source_key[covered_key.len()..].starts_with(&key) { break; }
            let new_len = covered_key.len() + key.len();
            if new_len > accepted_len {
                let (Some(start), Some(end)) = (word.start_ms, word.end_ms) else { break; };
                if start >= end || previous_end.is_some_and(|previous| end < previous) { break; }
                // Provider words and local PCM windows use independent chunk
                // edges. Assign the indivisible word by its midpoint, as in
                // source-partition recovery; full coverage stalls a whole row
                // when its first word straddles an owner-window boundary.
                // Confirmed foreign overlap remains a veto. An unobserved
                // midpoint remains pending and blocks later words as before.
                let midpoint = start + (end - start) / 2;
                if foreign_intervals.iter().any(|&(foreign_start, foreign_end)|
                    start < foreign_end && end > foreign_start)
                    || !intervals.iter().any(|&(owner_start, owner_end)|
                        owner_start <= midpoint && midpoint < owner_end)
                {
                    break;
                }
            }
            covered_key.push_str(&key);
            previous_end = word.end_ms;
        }
        // Untimed accepted text may be re-rendered, but never extended.
        if covered_key.len() < accepted_len { covered_key = source_key[..accepted_len].to_string(); }
        if covered_key.is_empty() { return String::new(); }
        consumed_prefix_end(&self.text, &covered_key)
            .map(|end| self.text[..end].trim().to_string())
            .unwrap_or_default()
    }

    pub(crate) fn activation_fragments(&self, window: (u64,u64), phrase: &str) -> Vec<ActivationFragment> {
        let source_key=content_key(&self.text); let phrase_key=content_key(phrase);
        let mut offset=0; let mut aligned=Vec::new();
        for word in &self.words {
            let key=content_key(&word.text); if key.is_empty(){continue;}
            if !source_key[offset..].starts_with(&key){break;}
            let (Some(start),Some(end))=(word.start_ms,word.end_ms) else {break;};
            if start>=end {break;}
            aligned.push((offset,key.clone(),start,end)); offset+=key.len();
        }
        let mut fragments=Vec::new();
        for (i,(offset,_,start,_)) in aligned.iter().enumerate() {
            if *start<window.0 || *start>=window.1 {continue;}
            let mut part=String::new();let mut last_end=*start;
            for (_,key,word_start,word_end) in &aligned[i..] {
                if *word_start<last_end || *word_end>window.1 {break;}
                part.push_str(key);last_end=*word_end;
                if part==phrase_key {break;}
                if phrase_key.starts_with(&part) || phrase_key.ends_with(&part) {
                    fragments.push(ActivationFragment {lead_key:source_key[..*offset].into(),
                        phrase_part:part.clone(),start_ms:*start,end_ms:last_end});
                } else {break;}
            }
        }
        fragments
    }

    /// Locate one exact activation occurrence anchored in locally confirmed
    /// PCM. The confirmation input end is not a word-aligned ASR endpoint.
    /// A phrase starting outside that window is ordinary body text.
    /// A provider word spanning phrase + body cannot invent a phrase endpoint.
    pub(crate) fn confirmed_activation_prefix(
        &self, window_ms: (u64, u64), phrase: &str, keyword_end_ms: Option<u64>,
    ) -> Option<(u64, ControlPrefixEvidence)> {
        let source_key = content_key(&self.text);
        let phrase_key = content_key(phrase);
        if phrase_key.is_empty() || window_ms.0 >= window_ms.1 { return None; }
        let mut covered_key = String::new();
        let mut coordinates = Vec::new();
        let mut previous_end = 0;
        for word in &self.words {
            let key = content_key(&word.text);
            if key.is_empty() { continue; }
            if !source_key[covered_key.len()..].starts_with(&key) { break; }
            let (Some(start), Some(end)) = (word.start_ms, word.end_ms) else { break; };
            if start >= end || start < previous_end { break; }
            let from = covered_key.len();
            covered_key.push_str(&key);
            coordinates.push((from, covered_key.len(), start, end));
            previous_end = end;
        }
        // Provider token endpoints can straddle a real KWS endpoint. The
        // independent local confirmation of this same PCM window plus a KWS
        // endpoint inside the first phrase word establishes the occurrence;
        // it does not classify later wake-word mentions as control text.
        let crosses_keyword = |word_start: u64, word_end: u64| keyword_end_ms
            .is_some_and(|end| word_start <= end && end < word_end
                && word_start >= window_ms.0 && word_start < window_ms.1);
        if phrase_key.starts_with(&source_key) {
            if let Some(&(0, _, start, end)) = coordinates.first() {
                if keyword_end_ms.is_some() && (crosses_keyword(start,end)
                    || (start >= window_ms.0 && end <= window_ms.1)) {
                    return Some((keyword_end_ms.unwrap(), ControlPrefixEvidence {
                        control_key: source_key.clone(), source_key,
                    }));
                }
            }
        }
        let mut matches = source_key.match_indices(&phrase_key).filter_map(|(start, _)| {
            let end = start + phrase_key.len();
            let (_, _, audio_start, first_word_end) = coordinates.iter().find(|(from, _, _, _)| *from == start)?;
            let (_, _, _, audio_end) = coordinates.iter().find(|(_, through, _, _)| *through == end)?;
            // Local confirmation can complete a phrase before the streaming
            // provider's last token endpoint (captured f80: local 1833 ms,
            // provider first token 1560..1640, last token 1940..2020).
            // Bind the independently confirmed phrase by its first complete
            // word in that PCM window, then use the exact provider phrase's
            // endpoint. Requiring every token inside the local input window
            // conflated two recognizers' timing and held all later body.
            // Alignment, unique occurrence and a separate phrase end remain
            // mandatory; a later body mention or phrase+body token cannot
            // manufacture this anchor.
            ((*audio_start >= window_ms.0 && *first_word_end <= window_ms.1)
                || crosses_keyword(*audio_start,*first_word_end))
                .then_some((*audio_end, end))
        });
        let (audio_end, end) = matches.next()?;
        if matches.next().is_some() { return None; }
        Some((audio_end, ControlPrefixEvidence {
            control_key: source_key[..end].to_string(), source_key,
        }))
    }

    pub(crate) fn activation_prefix(&self, activation_end_ms: u64, phrase: &str) -> Option<ControlPrefixEvidence> {
        let source_key = content_key(&self.text);
        if source_key.is_empty() { return None; }
        let mut control_key = String::new();
        let mut last_end_ms = 0;
        let mut observed_coordinate = false;
        for word in &self.words {
            let key = content_key(&word.text);
            if key.is_empty() { continue; }
            if !source_key[control_key.len()..].starts_with(&key) { break; }
            let Some(end_ms) = word.end_ms else { break; };
            if end_ms < last_end_ms { break; }
            observed_coordinate = true;
            if end_ms > activation_end_ms { break; }
            control_key.push_str(&key);
            last_end_ms = end_ms;
        }
        // A word can straddle the detector's keyword end. Finish only an exact
        // accepted phrase that already began in the timed control prefix;
        // an ordinary wake-word mention starting in the body is untouched.
        let phrase_key = content_key(phrase);
        if !phrase_key.is_empty() {
            if let Some(start) = source_key.find(&phrase_key) {
                if start < control_key.len() && start + phrase_key.len() > control_key.len() {
                    control_key = source_key[..start + phrase_key.len()].to_string();
                }
            }
        }
        observed_coordinate.then_some(ControlPrefixEvidence { source_key, control_key })
    }
}

impl TimedDeliveryBoundary {
    pub(crate) fn start_ms(&self) -> u64 { self.start_ms }

    pub(crate) fn end_ms(&self) -> u64 { self.end_ms }

    pub(crate) fn range_ms(&self) -> (u64, u64) { (self.start_ms, self.end_ms) }
}

impl ControlPrefixEvidence {
    pub(crate) fn body<'a>(&self, text: &'a str) -> Option<&'a str> {
        // Bind the decision to this provider revision. Reusing a phrase-only
        // prefix on a later body that mentions the wake words would erase it.
        if content_key(text) != self.source_key { return None; }
        if self.control_key.is_empty() { return Some(text.trim()); }
        let end = consumed_prefix_end(text, &self.control_key)?;
        Some(text[end..].trim_start_matches(is_decorative))
    }
}

pub(crate) fn terminal_punctuation_only(text: &str) -> bool {
    let mut terminal = false;
    for ch in text.chars() {
        if matches!(ch, '。' | '.' | '！' | '!' | '？' | '?' | '…') {
            terminal = true;
        } else if !ch.is_whitespace() && !matches!(ch, '”' | '’' | '"' | '\'' | '）' | ')' | '】' | ']') {
            return false;
        }
    }
    terminal
}

/// Map the terminal decoration of an already covered body. A late two-pass
/// spelling correction may preserve that boundary without permitting any
/// rewrite or new word to cross the irreversible delivery seam.
pub(crate) fn revised_terminal_suffix<'a>(text: &'a str, consumed_key: &str) -> Option<&'a str> {
    let final_key = content_key(text);
    if final_key != consumed_key {
        let consumed: Vec<char> = consumed_key.chars().collect();
        if !(8..=512).contains(&consumed.len()) { return None; }
        let final_content: Vec<char> = final_key.chars().collect();
        let anchor = &consumed[consumed.len() - 8..];
        let max_edits = (consumed.len() / 5).clamp(2, 12);
        if consumed.len().abs_diff(final_content.len()) > max_edits { return None; }
        let retained_anchor = final_content.ends_with(anchor)
            && final_content.windows(anchor.len()).filter(|window| *window == anchor).count() == 1;
        if !retained_anchor && consumed.last() != final_content.last() { return None; }
        let mut previous: Vec<usize> = (0..=final_content.len()).collect();
        let mut current = vec![0; final_content.len() + 1];
        for (index, expected) in consumed.iter().enumerate() {
            current[0] = index + 1;
            for prefix in 1..=final_content.len() {
                current[prefix] = (previous[prefix] + 1)
                    .min(current[prefix - 1] + 1)
                    .min(previous[prefix - 1] + usize::from(*expected != final_content[prefix - 1]));
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let terminal_cost = previous[final_content.len()];
        if terminal_cost > max_edits { return None; }
        if !retained_anchor {
            // A correction inside the old suffix can remove that exact
            // anchor. The complete consumed body must then align uniquely
            // to this terminal position, with the last content unit retained.
            // A cheaper or equally good earlier seam means possible new
            // speech, so it cannot authorize punctuation-only coverage.
            let first_possible_seam = consumed.len().saturating_sub(max_edits);
            if previous[first_possible_seam..final_content.len()]
                    .iter().any(|cost| *cost <= terminal_cost)
            { return None; }
        }
    }
    let (offset, last_content) = text.char_indices().rev().find(|(_, ch)| !is_decorative(*ch))?;
    Some(&text[offset + last_content.len_utf8()..])
}

/// Locate an already consumed prefix. Only case/formatting differences are
/// tolerated; a changed word, a shortened prefix, or a shifted opening fails.
/// Decorations at that committed seam belong to the prefix, not the new body.
pub(crate) fn consumed_prefix_end(text: &str, key: &str) -> Option<usize> {
    if key.is_empty() {
        return Some(0);
    }
    let mut expected = key.chars();
    let mut next = expected.next();
    for (offset, ch) in text.char_indices() {
        if is_decorative(ch) {
            continue;
        }
        if next.is_none() {
            return Some(offset);
        }
        for lower in ch.to_lowercase() {
            if next != Some(lower) {
                return None;
            }
            next = expected.next();
        }
    }
    next.is_none().then_some(text.len())
}

/// Locate the end of a revised, already consumed prefix in an owner-approved
/// snapshot. The editor's submitted words remain immutable: this only maps
/// their source coordinate so genuinely new words can continue after it.
/// Require retained context and a unique best alignment, including competing
/// terminal seams, rather than choosing a later equally plausible boundary.
pub(crate) fn revised_prefix_end(text: &str, consumed_key: &str) -> Option<usize> {
    let consumed: Vec<char> = consumed_key.chars().collect();
    if consumed.len() > 512 {
        return None;
    }
    let max_edits = (consumed.len() / 5).clamp(1, 12);
    if consumed.len().saturating_sub(max_edits) < 4 {
        return None;
    }
    let mut content = Vec::new();
    let mut ends = Vec::new();
    for (offset, ch) in text.char_indices() {
        if is_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            content.push(lower);
            ends.push(offset + ch.len_utf8());
        }
    }
    // An insertion inside the old body is not independent proof of a new
    // continuation, even when it can be aligned to an earlier prefix.
    if content.ends_with(&consumed[consumed.len() - 4..]) {
        return None;
    }
    let first = consumed.len().saturating_sub(max_edits);
    let last = content.len().min(consumed.len() + max_edits);
    if first > last {
        return None;
    }
    let mut previous: Vec<usize> = (0..=last).collect();
    let mut current = vec![0; last + 1];
    for (index, expected) in consumed.iter().enumerate() {
        current[0] = index + 1;
        for prefix in 1..=last {
            current[prefix] = (previous[prefix] + 1)
                .min(current[prefix - 1] + 1)
                .min(previous[prefix - 1] + usize::from(*expected != content[prefix - 1]));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let cost = *previous[first..=last].iter().min()?;
    if cost > max_edits {
        return None;
    }
    let mut seams = (first..=last).filter(|&prefix| previous[prefix] == cost);
    let boundary = seams.next()?;
    if seams.next().is_some()
        || boundary == 0
        || boundary >= content.len()
        || content[boundary - 1] != *consumed.last()?
        || ends[boundary - 1] == ends[boundary]
    {
        return None;
    }
    Some(ends[boundary - 1])
}

fn stability_units(text: &str) -> Vec<(char, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut units = Vec::new();
    for (index, &(offset, ch)) in chars.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| chars[i].1);
        let next = chars.get(index + 1).map(|(_, ch)| *ch);
        // Decimal separators and literal operators belong to the body. Word
        // spacing also distinguishes e.g. "no where" from "nowhere".
        let numeric_separator = matches!(ch, '.' | ',' | '，' | '。' | ':' | '：')
            && previous.is_some_and(|ch| ch.is_ascii_digit())
            && next.is_some_and(|ch| ch.is_ascii_digit());
        let literal_operator = ch.is_ascii_punctuation()
            && !matches!(ch, ',' | '.' | '!' | '?' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>');
        let word_space = ch.is_whitespace()
            && previous.is_some_and(|ch| ch.is_ascii_alphabetic())
            && next.is_some_and(|ch| ch.is_ascii_alphabetic());
        if is_decorative(ch) && !numeric_separator && !literal_operator && !word_space {
            continue;
        }
        for lower in ch.to_lowercase() {
            units.push((lower, offset));
        }
    }
    units
}

/// Stability belongs to spoken content. Once all retained revisions agree
/// on that content, use the newest clause punctuation without waiting for it
/// to age again. No new or revised body word can cross the stable boundary.
pub(crate) fn stable_content_prefix<'a>(
    current: &str,
    revisions: impl Iterator<Item = &'a str>,
) -> String {
    let units = stability_units(current);
    let mut count = units.len();
    for revision in revisions {
        let older = stability_units(revision);
        count = units.iter().take(count).zip(older.iter())
            .take_while(|(left, right)| left.0 == right.0).count();
    }
    // Never split a Unicode character whose lowercase mapping has several
    // comparison units.
    while count > 0 && count < units.len() && units[count - 1].1 == units[count].1 {
        count -= 1;
    }
    if count == 0 {
        return String::new();
    }
    let end = units.get(count).map_or(current.len(), |(_, offset)| *offset);
    current[..end].to_string()
}

#[cfg(test)]
mod tests {
    fn timed_word(text: &str, start_ms: u64, end_ms: u64) -> TimedTranscriptWord {
        TimedTranscriptWord {
            text: text.into(),
            start_ms: Some(start_ms),
            end_ms: Some(end_ms),
        }
    }

    #[test]
    fn nonzero_clip_suffix_keeps_only_child_words_on_the_global_timeline() {
        let parent = TimedTranscript {
            text: "已经提交".into(),
            words: vec![timed_word("已经提交", 60_100, 60_300)],
        };
        let boundary = parent.delivery_boundary(&content_key(&parent.text)).unwrap();
        assert_eq!(boundary.range_ms(), (60_100, 60_300));
        assert_eq!(boundary.start_ms(), 60_100);
        assert_eq!(boundary.end_ms(), 60_300);

        let local_child = TimedTranscript {
            text: "已经，提交；然后继续。".into(),
            words: vec![
                timed_word("已经", 100, 190),
                timed_word("提交", 190, 300),
                timed_word("然后继续", 500, 900),
            ],
        };
        let child = local_child.shifted_from_clip(60_000, 61_000).unwrap();
        let suffix = child.timed_suffix_after_delivery(&boundary).unwrap();
        assert_eq!(suffix.text, "然后继续。");
        assert_eq!(suffix.words.len(), 1);
        assert_eq!(suffix.words[0].text, "然后继续");
        assert_eq!(suffix.words[0].start_ms, Some(60_500));
        assert_eq!(suffix.words[0].end_ms, Some(60_900));
        assert!(suffix.delivery_boundary(&content_key(&suffix.text)).is_some());
    }

    #[test]
    fn timed_suffix_uses_the_same_audio_occurrence_across_tokenization() {
        let parent = TimedTranscript {
            text: "需要保留的内容".into(),
            words: vec![timed_word("需要保留的内容", 1_000, 2_000)],
        };
        let boundary = parent.delivery_boundary(&content_key(&parent.text)).unwrap();
        let child = TimedTranscript {
            text: "需要保留的内容。前文。需要保留的内容，然后继续。".into(),
            words: vec![
                timed_word("需要", 1_000, 1_250),
                timed_word("保留", 1_250, 1_500),
                timed_word("的内容", 1_500, 2_000),
                timed_word("前文", 2_100, 2_500),
                timed_word("需要保留的内容", 4_000, 5_000),
                timed_word("然后继续", 5_100, 5_800),
            ],
        };
        let suffix = child.timed_suffix_after_delivery(&boundary).unwrap();
        assert_eq!(suffix.text, "前文。需要保留的内容，然后继续。");
        assert_eq!(suffix.words[0].text, "前文");

        let different_occurrence = TimedDeliveryBoundary {
            key: boundary.key.clone(),
            start_ms: 3_000,
            end_ms: 3_500,
        };
        assert!(child.timed_suffix_after_delivery(&different_occurrence).is_none());
    }

    #[test]
    fn timed_suffix_rejects_missing_coordinates_split_seams_and_overlap() {
        let parent = TimedTranscript {
            text: "已经提交".into(),
            words: vec![timed_word("已经提交", 100, 300)],
        };
        let boundary = parent.delivery_boundary(&content_key(&parent.text)).unwrap();

        let mut missing = TimedTranscript {
            text: "已经提交，然后继续".into(),
            words: vec![timed_word("已经提交", 100, 300), timed_word("然后继续", 400, 700)],
        };
        missing.words[1].start_ms = None;
        assert!(missing.timed_suffix_after_delivery(&boundary).is_none());

        let split_seam = TimedTranscript {
            text: "已经提交然后继续".into(),
            words: vec![timed_word("已经提交然后", 100, 450), timed_word("继续", 450, 700)],
        };
        assert!(split_seam.timed_suffix_after_delivery(&boundary).is_none());

        let overlapping_tail = TimedTranscript {
            text: "已经提交，然后继续".into(),
            words: vec![timed_word("已经提交", 100, 300), timed_word("然后继续", 299, 700)],
        };
        assert!(overlapping_tail.timed_suffix_after_delivery(&boundary).is_none());

        let complete = TimedTranscript {
            text: "已经提交。".into(),
            words: vec![timed_word("已经提交", 100, 300)],
        };
        let empty = complete.timed_suffix_after_delivery(&boundary).unwrap();
        assert!(empty.text.is_empty() && empty.words.is_empty());
    }

    #[test]
    fn shifted_clip_rejects_partial_or_invalid_local_coordinates() {
        let valid = TimedTranscript {
            text: "新的正文".into(),
            words: vec![timed_word("新的正文", 10, 200)],
        };
        assert_eq!(
            valid.shifted_from_clip(80_000, 80_200).unwrap().words[0].end_ms,
            Some(80_200)
        );
        assert!(valid.shifted_from_clip(80_000, 80_199).is_none());
        assert!(valid.shifted_from_clip(80_200, 80_000).is_none());

        let mut missing = valid.clone();
        missing.words[0].end_ms = None;
        assert!(missing.shifted_from_clip(80_000, 80_200).is_none());
        let reversed = TimedTranscript {
            text: "新的正文".into(),
            words: vec![timed_word("新的正文", 200, 100)],
        };
        assert!(reversed.shifted_from_clip(80_000, 80_300).is_none());
    }

    #[test]
    fn two_child_continuations_advance_with_child_only_boundaries() {
        let parent = TimedTranscript {
            text: "第一段".into(),
            words: vec![timed_word("第一段", 100, 300)],
        };
        let first_boundary = parent.delivery_boundary(&content_key(&parent.text)).unwrap();
        let first_child = TimedTranscript {
            text: "第一段，然后第二段。".into(),
            words: vec![timed_word("第一段", 100, 300), timed_word("然后第二段", 400, 800)],
        };
        let first_suffix = first_child.timed_suffix_after_delivery(&first_boundary).unwrap();
        assert_eq!(first_suffix.text, "然后第二段。");
        let second_boundary = first_suffix
            .delivery_boundary(&content_key(&first_suffix.text))
            .unwrap();

        let second_child = TimedTranscript {
            text: "然后，第二段；最后第三段。".into(),
            words: vec![
                timed_word("然后", 400, 520),
                timed_word("第二段", 520, 800),
                timed_word("最后第三段", 900, 1_300),
            ],
        };
        let second_suffix = second_child.timed_suffix_after_delivery(&second_boundary).unwrap();
        assert_eq!(second_suffix.text, "最后第三段。");
        assert_eq!(second_suffix.words.len(), 1);
        assert_eq!(second_suffix.words[0].start_ms, Some(900));
    }

    #[test]
    fn submitted_pcm_word_locates_seam_after_opening_and_tokenization_revision() {
        let word = |text: &str, start, end| TimedTranscriptWord {
            text: text.into(), start_ms: Some(start), end_ms: Some(end),
        };
        // Recorded f80 replay: two-pass changes both the opening and the
        // beginning of the submitted clause, and splits its final word.
        let old = TimedTranscript { text: "开始录音，听见的安排已经结束，有资料放在左边，设备".into(), words: vec![
            word("开始录音", 1000, 2020), word("听见的安排已经结束", 2300, 3900),
            word("有资料放在", 4500, 5480), word("左边", 5560, 5640), word("设备", 6180, 6260),
        ] };
        let source = content_key("听见的安排已经结束，有资料放在左边，");
        let boundary = old.delivery_boundary(&source).unwrap();
        let revised = TimedTranscript { text: "开始录音，今天的安排已经结束，然后资料放在左边，设备放在右边。".into(), words: vec![
            word("开始录音", 1000, 2040), word("今天的安排已经结束", 2160, 3760),
            word("然后资料放在", 4560, 5520), word("左", 5520, 5680), word("边", 5680, 5840),
            word("设备放在右边", 5920, 6880),
        ] };
        let body = "今天的安排已经结束，然后资料放在左边，设备放在右边。";
        assert_eq!(revised_prefix_end(body, &source), None, "text-only boundary is not established");
        assert_eq!(revised.body_suffix_after_delivery(body, &boundary), Some("设备放在右边。"));
        assert!(revised.body_suffix_after_delivery("另一份正文，设备放在右边。", &boundary).is_none());
        assert!(revised.body_suffix_after_delivery("边，设备放在右边。", &boundary).is_none(),
            "a seam beginning before the body cannot authorize its internal tail");
        let mut shifted = revised.clone();
        for token in &mut shifted.words { token.start_ms = token.start_ms.map(|n| n + 10000); token.end_ms = token.end_ms.map(|n| n + 10000); }
        assert!(shifted.body_suffix_after_delivery(body, &boundary).is_none(), "same words in a different audio occurrence cannot consume text");
        let mut absent = revised.clone(); absent.words[4].end_ms = None;
        assert!(absent.body_suffix_after_delivery(body, &boundary).is_none());
        assert!(old.delivery_boundary(&content_key("听见的安排已经结束有资料放在左")).is_none(), "a partial provider word is not a submitted seam");
    }

    #[test]
    fn local_confirmation_input_end_is_not_the_provider_phrase_endpoint() {
        let word = |text: &str, start, end| TimedTranscriptWord {
            text: text.into(), start_ms: Some(start), end_ms: Some(end),
        };
        // Coordinates captured by replaying the failed Beta30 f80 recording.
        let source = TimedTranscript { text: "嗯，开始录音，今天的安排已经结束。".into(), words: vec![
            word("嗯", 1240, 1320), word("开始", 1560, 1640),
            word("录音", 1940, 2020), word("今天的安排已经结束", 2300, 4200),
        ] };
        let (end, evidence) = source.confirmed_activation_prefix((0, 1833), "开始录音", None).unwrap();
        assert_eq!(end, 2020);
        assert_eq!(evidence.body(&source.text), Some("今天的安排已经结束。"));
        assert!(source.confirmed_activation_prefix((0, 1500), "开始录音", None).is_none(),
            "an entirely later phrase cannot use the earlier confirmation");
        assert!(source.confirmed_activation_prefix((0, 1600), "开始录音", None).is_none(),
            "without a KWS endpoint require a complete first word in confirmed PCM");
        let mut missing = source.clone();
        missing.words[2].start_ms = None;
        assert!(missing.confirmed_activation_prefix((0, 1833), "开始录音", None).is_none());
        let mut overlap = source.clone();
        overlap.words[2].start_ms = Some(1600);
        assert!(overlap.confirmed_activation_prefix((0, 1833), "开始录音", None).is_none());
    }

    #[test]
    fn activation_confirmation_needs_unique_aligned_words_in_the_confirmed_window() {
        let word=|text:&str,start,end| TimedTranscriptWord {text:text.into(),start_ms:Some(start),end_ms:Some(end)};
        let source=TimedTranscript {text:"旁人的前文。开始录音。我的正文。".into(),words:vec![
            word("旁人的前文",100,900),word("开始",1000,1300),word("录音",1300,1600),word("我的正文",1800,2400)]};
        let (end,evidence)=source.confirmed_activation_prefix((0,1700),"开始录音",None).unwrap();
        assert_eq!(end,1600);assert_eq!(evidence.body(&source.text),Some("我的正文。"));
        assert!(source.confirmed_activation_prefix((0,900),"开始录音",None).is_none(),"later body mentions are not an activation");
        assert!(source.confirmed_activation_prefix((1100,1700),"开始录音",None).is_none());
        assert!(evidence.body("另一个前文。开始录音。我的正文。").is_none(),"revision evidence cannot be borrowed");
        let mut mixed=source.clone();mixed.words=vec![word("旁人的前文",100,900),word("开始录音我的正文",1000,2400)];
        assert!(mixed.confirmed_activation_prefix((0,3000),"开始录音",None).is_none(),"a mixed word cannot place an internal audio boundary");
        let repeated=TimedTranscript {text:"开始录音，开始录音".into(),words:vec![word("开始录音",100,500),word("开始录音",600,1000)]};
        assert!(repeated.confirmed_activation_prefix((0,1100),"开始录音",None).is_none());
        let mut invalid=source.clone();invalid.words[1].start_ms=None;
        assert!(invalid.confirmed_activation_prefix((0,1700),"开始录音",None).is_none());
    }

    #[test]
    fn owner_word_admission_requires_coverage_of_the_same_audio_occurrence() {
        let timing = TimedTranscript {
            text: "本人继续。旁人说话。本人回来。".into(),
            words: vec![
                TimedTranscriptWord {text:"本人继续".into(),start_ms:Some(100),end_ms:Some(700)},
                TimedTranscriptWord {text:"旁人说话".into(),start_ms:Some(900),end_ms:Some(1500)},
                TimedTranscriptWord {text:"本人回来".into(),start_ms:Some(1700),end_ms:Some(2300)},
            ],
        };
        assert_eq!(timing.owner_supported_prefix("", &[(100,500),(400,800),(1700,2400)], &[]),"本人继续。");
        assert_eq!(timing.owner_supported_prefix("", &[(1700,2400)], &[]),"");
        assert_eq!(timing.owner_supported_prefix(&content_key("本人继续"), &[(1700,2400)], &[]),"本人继续。");
        assert_eq!(timing.owner_supported_prefix("", &[(100,2400)], &[]),timing.text);
    }

    #[test]
    fn owner_word_window_seam_uses_midpoint_without_bridging_pending_or_foreign_audio() {
        let source = TimedTranscript {
            text: "然后我现在回来继续说编号是后文".into(),
            words: vec![
                ("然后", 7772, 7852), ("我", 9372, 9452),
                ("现在", 9532, 9612), ("回来", 9852, 9932),
                ("继续", 10252, 10332), ("说", 10572, 10652),
                ("编号", 11452, 11532), ("是", 11932, 12012),
                ("后文", 12412, 12972),
            ].into_iter().map(|(text,start,end)| TimedTranscriptWord {
                text: text.into(),start_ms:Some(start),end_ms:Some(end),
            }).collect(),
        };
        let owner = [(7800,9000),(9400,10600),(10600,11800),(12200,13400)];
        assert_eq!(source.owner_supported_prefix("", &owner, &[]), "然后我现在回来继续说编号");
        assert_eq!(source.owner_supported_prefix("", &owner, &[(7772,7800)]), "");
        assert_eq!(source.owner_supported_prefix("", &[(7852,14000)], &[]), "");
        let seam = TimedTranscript { text:"边界".into(), words:vec![TimedTranscriptWord {
            text:"边界".into(), start_ms:Some(100),end_ms:Some(200),
        }] };
        assert_eq!(seam.owner_supported_prefix("", &[(0,150)], &[]), "");
        assert_eq!(seam.owner_supported_prefix("", &[(150,300)], &[]), "边界");
    }

    #[test]
    fn absent_or_conflicting_word_coordinates_never_extend_owner_text() {
        let mut timing = TimedTranscript {
            text:"本人继续。".into(),
            words:vec![TimedTranscriptWord {text:"本人继续".into(),start_ms:None,end_ms:Some(700)}],
        };
        assert_eq!(timing.owner_supported_prefix("", &[(0,1000)], &[]),"");
        assert_eq!(timing.owner_supported_prefix(&content_key("本人继续"), &[], &[]),"本人继续。");
        timing.words[0].start_ms=Some(800);
        assert_eq!(timing.owner_supported_prefix("", &[(0,1000)], &[]),"");
        timing.words[0].start_ms=Some(100);
        timing.words[0].text="另一修订".into();
        assert_eq!(timing.owner_supported_prefix("", &[(0,1000)], &[]),"");
    }

    use super::*;

    #[test]
    fn activation_coordinates_preserve_straddling_body_and_unknown_words() {
        let timing = |text: &str, words: Vec<(&str, Option<u64>)>| TimedTranscript {
            text: text.into(),
            words: words.into_iter().map(|(text, end_ms)| TimedTranscriptWord {
                text: text.into(), start_ms: None, end_ms,
            }).collect(),
        };
        let evidence = timing("开始录音，然后继续。", vec![("开始录音", Some(900)), ("然后", Some(1200)), ("继续", Some(1500))])
            .activation_prefix(1000, "开始录音").unwrap();
        assert_eq!(evidence.body("开始录音，然后继续。"), Some("然后继续。"));
        assert_eq!(evidence.body("开始录音，另外一段。"), None);
        assert!(timing("正文不应消失。", vec![("正文", None)])
            .activation_prefix(1000, "开始录音").is_none());
        assert!(timing("正文不应消失。", vec![("别人的", Some(500))])
            .activation_prefix(1000, "开始录音").is_none());
        let gap = timing("开始录音，然后继续。", vec![("开始录音", Some(900)), ("然后", None)])
            .activation_prefix(1000, "开始录音").unwrap();
        assert_eq!(gap.body("开始录音，然后继续。"), Some("然后继续。"));
        let reversal = timing("前文，然后继续。", vec![("前文", Some(900)), ("然后", Some(800))])
            .activation_prefix(1000, "开始录音").unwrap();
        assert_eq!(reversal.body("前文，然后继续。"), Some("然后继续。"));
    }

    #[test]
    fn activation_coordinates_keep_later_wake_mentions_and_finish_only_control_phrase() {
        let body_mention = TimedTranscript {
            text: "开始录音是产品的唤醒词。".into(),
            words: vec![TimedTranscriptWord { start_ms: None, text: "开始录音".into(), end_ms: Some(1800) }],
        }.activation_prefix(1000, "开始录音").unwrap();
        assert_eq!(body_mention.body("开始录音是产品的唤醒词。"), Some("开始录音是产品的唤醒词。"));
        assert_eq!(body_mention.body("“开始录音”是产品的唤醒词。"), Some("“开始录音”是产品的唤醒词。"));
        let control = TimedTranscript {
            text: "前声开始录音，然后正文。".into(),
            words: vec![
                TimedTranscriptWord { start_ms: None, text: "前声开始".into(), end_ms: Some(900) },
                TimedTranscriptWord { start_ms: None, text: "录音".into(), end_ms: Some(1050) },
            ],
        }.activation_prefix(1000, "开始录音").unwrap();
        assert_eq!(control.body("前声开始录音，然后正文。"), Some("然后正文。"));
    }

    #[test]
    fn consumed_boundary_uses_the_shared_content_coordinate() {
        let key = content_key("确认 NC 的结果。");
        let revised = "确认 nc 的结果，后续继续。";
        let end = consumed_prefix_end(revised, &key).unwrap();
        assert_eq!(&revised[end..], "后续继续。");
        assert_eq!(consumed_prefix_end("确认 nc 的结果。", &key), Some("确认 nc 的结果。".len()));
        assert_eq!(consumed_prefix_end("确认另外的结果。后续继续。", &key), None);
        assert_eq!(consumed_prefix_end("确认 nc", &key), None);
        assert_eq!(consumed_prefix_end("前面插入确认 nc 的结果。", &key), None);
    }

    #[test]
    fn consumed_boundary_preserves_unicode_byte_offsets() {
        let text = "İ 已确认。新的正文。";
        let end = consumed_prefix_end(text, &content_key("İ 已确认。")).unwrap();
        assert_eq!(&text[end..], "新的正文。");
    }

    #[test]
    fn revised_prefix_maps_short_corrected_openings_and_small_growth() {
        let key = content_key("那里看一下先");
        for (text, tail) in [
            ("那你看一下先优化速度。", "优化速度。"),
            ("那你看一下先，继续。", "，继续。"),
            ("那里看一下先好。", "好。"),
        ] {
            let end = revised_prefix_end(text, &key).unwrap();
            assert_eq!(&text[end..], tail);
        }
        let text = "İ 那你看一下先，继续说话。";
        let end = revised_prefix_end(text, &content_key("İ 那里看一下先")).unwrap();
        assert_eq!(&text[end..], "，继续说话。");
    }

    #[test]
    fn revised_prefix_rejects_ambiguous_or_unproved_continuations() {
        for (consumed, text) in [
            ("aaaaab", "aaaabbbc"), // Both five and six units cost one edit.
            ("那里看一下先", "完全不同的任务继续说话。"),
            ("那里", "那你继续说话。"), // Too little retained context.
            ("那里看一下先", "那你看一下先。"), // Only a correction, no new body.
            ("那里看一下先", "那里现在看一下先。"), // Internal insertion only.
            ("那里看一下先", "那你看一下后优化速度。"), // The old seam was also rewritten.
            ("那里看一下先", "先把那里看一下先优化速度。"), // Shifted opening exceeds the budget.
            ("abcdxi", "abcdİ继续。"), // A seam inside the multi-unit lowercase of İ is invalid.
        ] {
            assert_eq!(revised_prefix_end(text, &content_key(consumed)), None, "{consumed} -> {text}");
        }
    }

    #[test]
    fn stable_body_accepts_late_clause_punctuation_without_reaging_words() {
        assert_eq!(stable_content_prefix("第一句话？接下来", ["第一句话", "第一句话？接"].into_iter()), "第一句话？");
        assert_eq!(stable_content_prefix("确认 nc 的结果，继续下一句。", ["确认 NC 的结果。继续下一句", "确认 nc 的结果，继续下一句"].into_iter()), "确认 nc 的结果，继续下一句。");
        assert_eq!(stable_content_prefix("今天谈论速度。", ["今天讨论速度"].into_iter()), "今天");
    }

    #[test]
    fn terminal_boundary_survives_a_revision_inside_the_old_suffix_anchor() {
        let consumed = content_key("我们先检查这个流程然后看下怎么处理");
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个流程然后看一下怎么处理。", &consumed), Some("。"));
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个系统然后看一下怎么处理。", &consumed), Some("。"));
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个流程然后看下怎么处理，然后继续处理。", &consumed), None);
        assert_eq!(revised_terminal_suffix(
            "完全不同的任务现在还需要知道如何处理。", &consumed), None);
    }

    #[test]
    fn stability_preserves_numeric_literals_word_boundaries_and_unicode() {
        for (current, earlier, expected) in [
            ("价格 1.2。", "价格 12。", "价格 1"),
            ("比例 1/2。", "比例 12。", "比例 1"),
            ("时间 12:30。", "时间 1230。", "时间 12"),
            ("no where。", "nowhere。", "no"),
            ("İ 后续。", "i 后续。", ""),
        ] {
            assert_eq!(stable_content_prefix(current, [earlier].into_iter()), expected);
        }
    }
}


#[cfg(test)]
mod final_supplement_tests {
    use super::*;

    fn native(words: &[&str]) -> TimedTranscript {
        let words = words.iter().enumerate().map(|(i, word)| TimedTranscriptWord {
            text: word.to_string(), start_ms: Some(i as u64 * 100), end_ms: Some(i as u64 * 100 + 90),
        }).collect();
        TimedTranscript { text: String::new(), words }.native_word_view().unwrap()
    }

    #[test]
    fn covered_final_requires_the_submitted_acoustic_occurrence() {
        let source = native(&["one", "person"]);
        let boundary = source.delivery_boundary(&content_key(&source.text)).unwrap();
        assert_eq!(final_delivery_after_consumption("one person.", &source, &source.text, Some(&boundary)),
            Some(FinalSourceDelivery::Covered));
        assert!(final_delivery_after_consumption("one person.", &source, &source.text, None).is_none(),
            "a textual match without the submitted occurrence cannot override final reconciliation");
        let mut repeated = source.clone();
        for word in &mut repeated.words {
            word.start_ms = word.start_ms.map(|time| time + 1_000);
            word.end_ms = word.end_ms.map(|time| time + 1_000);
        }
        assert!(final_delivery_after_consumption("one person.", &repeated, &repeated.text, Some(&boundary)).is_none(),
            "a later equal-sounding occurrence is not the already delivered source");
    }

    #[test]
    fn native_tail_uses_append_but_middle_omissions_require_owned_range() {
        for (old, tokens, rendering, suffix) in [
            ("今天保留。", vec!["今天", "保留", "明天", "开会"], "今天保留。明天，开会！", "明天，开会！"),
            ("Open the file.", vec!["Open", "the", "file", "then", "save"], "Open the file, then save!", " then save!"),
            ("今日は保存。", vec!["今日は", "保存", "明日", "確認"], "今日は保存。明日、確認。", "明日、確認。"),
        ] {
            let mut source = native(&tokens);
            source.text = rendering.into();
            assert_eq!(lossless_final_delivery(old, &source, rendering),
                Some(FinalSourceDelivery::Append { suffix: suffix.into() }));
            assert!(lossless_final_delivery(&format!("{old}{suffix}"), &source, rendering).is_none(),
                "a repeated source does not submit another tail");
        }
        let source = native(&["Open", "the", "red", "file", "then", "save"]);
        assert_eq!(lossless_final_delivery("Open the file.", &source, &source.text),
            Some(FinalSourceDelivery::SupplementOwnedRange { body: "Open the red file. then save".into() }));
        assert!(lossless_final_delivery("user changed this", &source, &source.text).is_none(),
            "a different committed body cannot be used as an append seam");
    }

    #[test]
    fn final_supplement_keeps_bytes_numbers_punctuation_and_adds_middle_and_tail_once() {
        let n = native(&["Open", "the", "red", "file", "at", "9", "then", "save"]);
        let old = "Open the file at 9.";
        let result = lossless_final_supplement(old, &n, &n.text).unwrap();
        assert_eq!(result, "Open the red file at 9. then save");
        assert!(result.contains("9."));
        assert!(lossless_final_supplement(&result, &n, &n.text).is_none());
    }

    #[test]
    fn final_supplement_preserves_selected_provider_punctuation_in_new_body() {
        for (old, tokens, rendering, expected) in [
            ("今天保留。", vec!["今天", "保留", "明天", "九点", "开会"], "今天保留。明天九点，开会。", "今天保留。明天九点，开会。"),
            ("Open the file.", vec!["Open", "the", "red", "file", "then", "save"], "Open the red file, then save!", "Open the red file. then save!"),
            ("今日は保存。", vec!["今日は", "保存", "明日", "確認"], "今日は保存。明日、確認。", "今日は保存。明日、確認。"),
            ("오늘 저장.", vec!["오늘", "저장", "내일", "확인"], "오늘 저장. 내일, 확인!", "오늘 저장. 내일, 확인!"),
            ("مرحبا بك.", vec!["مرحبا", "بك", "شكرا"], "مرحبا بك. شكرا!", "مرحبا بك. شكرا!"),
        ] {
            let mut source = native(&tokens);
            source.text = rendering.into();
            let result = lossless_final_supplement(old, &source, rendering).unwrap();
            assert_eq!(result, expected);
            assert!(lossless_final_supplement(&result, &source, rendering).is_none());
        }
        let mut source = native(&["开始录音", "今天", "保留", "明天", "开会"]);
        source.text = "开始录音，今天保留。明天，开会！".into();
        assert_eq!(lossless_final_supplement("今天保留。", &source, "今天保留。明天，开会！").as_deref(), Some("今天保留。明天，开会！"));
        let mut divergent = native(&["9", "people", "arrive"]);
        divergent.text = "nine people arrive!".into();
        assert_eq!(divergent.native_word_rendering_view().unwrap().text, "9 people arrive", "divergent spelling cannot borrow native rendering authority");
    }

    #[test]
    fn final_supplement_rejects_substitution_deletion_ambiguity_partial_words_and_bad_coordinates() {
        for (old, words) in [("keep blue", vec!["keep", "red", "now"]),
            ("keep all words", vec!["keep", "words"]),
            ("yes", vec!["yes", "yes"]),
            ("store", vec!["restore"])] {
            let n=native(&words);
            assert!(lossless_final_supplement(old, &n, &n.text).is_none());
        }
        let mut n=native(&["one", "new", "two"]);
        n.words[1].start_ms=None;
        assert!(lossless_final_supplement("one two", &n, &n.text).is_none());
        n.words[1].start_ms=Some(1);
        assert!(lossless_final_supplement("one two", &n, &n.text).is_none());
    }

    #[test]
    fn final_supplement_multilingual_and_unicode_preserves_every_delivered_byte() {
        for (old, tokens, expected) in [
            ("今天。保存。", vec!["今天", "完整", "保存"], "今天。完整保存。"),
            ("今日は保存。", vec!["今日は", "必ず", "保存"], "今日は必ず保存。"),
            ("Wir speichern.", vec!["Wir", "alles", "speichern"], "Wir alles speichern."),
            ("مرحبا بك", vec!["مرحبا", "أهلا", "بك"], "مرحبا أهلا بك"),
            ("A😀B", vec!["A", "😀", "中", "B"], "A😀中B"),
            ("오늘 저장", vec!["오늘", "모두", "저장"], "오늘 모두 저장"),
            ("วันนี้บันทึก", vec!["วันนี้", "ทั้งหมด", "บันทึก"], "วันนี้ทั้งหมดบันทึก"),
            ("Nous gardons.", vec!["Nous", "les", "idées", "gardons"], "Nous les idées gardons."),
        ] {
            let n=native(&tokens);
            assert_eq!(lossless_final_supplement(old, &n, &n.text).as_deref(),Some(expected));
        }
    }

    #[test]
    #[ignore = "reads the saved real provider response selected explicitly by the repair runner"]
    fn final_supplement_saved_1325_real_provider_restores_middle_without_replacing_existing_text() {
        let path=std::env::var("LISTENER_FINAL_SUPPLEMENT_FIXTURE").unwrap();
        let fixture:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let words=fixture["response"]["utterances"].as_array().unwrap().iter()
            .flat_map(|u| u["words"].as_array().into_iter().flatten()).map(|w| TimedTranscriptWord {
                text:w["text"].as_str().unwrap().into(), start_ms:w["start_time"].as_u64(),end_ms:w["end_time"].as_u64(),
            }).collect();
        let n=TimedTranscript { text:fixture["response"]["text"].as_str().unwrap().into(),words }.native_word_view().unwrap();
        let body=n.text.strip_prefix("开始录音").unwrap();
        let old=fixture["delivered"].as_str().unwrap();
        let result=lossless_final_supplement(old,&n,body).expect("a unique native-word insertion must repair the saved failure");
        assert!(result.contains("下不为例。已经要完成的内容要保留时间就这么下午请把客户反馈整理出来。"));
        for number in ["9点", "12人", "45分钟", "350美元"] { assert!(result.contains(number)); }
        assert_eq!(result.replace("已经要完成的内容要保留时间就这么", ""),old);
        assert_eq!(content_key(&result),content_key(body));
        assert!(lossless_final_supplement(&result,&n,body).is_none());
        let mut without= n.clone();
        without.words.retain(|w| w.start_ms.unwrap()<7792 || w.end_ms.unwrap()>10502);
        let without=without.native_word_view().unwrap();
        assert!(lossless_final_supplement(old,&without,without.text.strip_prefix("开始录音").unwrap()).is_none(),
            "missing cloud content must not be fabricated from the test prompt");
        let mut edited=old.to_string(); edited.push_str("用户自己的修改");
        assert!(lossless_final_supplement(&edited,&n,body).is_none());
        std::fs::write(std::env::var("LISTENER_FINAL_SUPPLEMENT_RESULT").unwrap(),result).unwrap();
    }
}
