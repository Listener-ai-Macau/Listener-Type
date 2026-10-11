//! Conservative ordinary-ASR speaker projection. Identity and text revisions
//! stay in the capture clock; an unavailable observation never excludes words.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceKind {
    Owner,
    Other,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SpeakerSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    pub kind: EvidenceKind,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LocalSpeakerTimeline {
    /// Quality-qualified verification against the independent enrollment bank.
    pub identity: Vec<SpeakerSpan>,
    /// Per-chunk personal activity, including Unknown and positive vetoes.
    pub presence: Vec<SpeakerSpan>,
}

impl LocalSpeakerTimeline {
    pub(crate) fn note_identity(&mut self, span: SpeakerSpan) {
        if span.start_ms < span.end_ms {
            self.identity.push(span);
        }
    }

    pub(crate) fn note_presence(&mut self, span: SpeakerSpan) {
        if span.start_ms >= span.end_ms {
            return;
        }
        if span.kind != EvidenceKind::Unknown
            && self.presence.iter().any(|old| {
                old.kind == span.kind
                    && old.start_ms <= span.start_ms
                    && old.end_ms >= span.end_ms
            })
        {
            return;
        }
        if let Some(last) = self
            .presence
            .last_mut()
            .filter(|last| last.end_ms == span.start_ms && last.kind == span.kind)
        {
            last.end_ms = span.end_ms;
        } else {
            self.presence.push(span);
        }
    }

    pub(crate) fn confirms_other(&self, start: u64, end: u64) -> bool {
        // Neither a personal-model rejection nor a dominant-speaker decision
        // by itself proves every word in a mixed interval belongs to another.
        !self.presence.iter().chain(&self.identity).any(|span| {
            span.kind == EvidenceKind::Owner && overlaps(start, end, span.start_ms, span.end_ms)
        }) && covered(&self.presence, start, end, EvidenceKind::Other)
            && covered(&self.identity, start, end, EvidenceKind::Other)
    }

    fn owner_vetoes(&self, start: u64, end: u64) -> bool {
        self.presence.iter().chain(&self.identity).any(|span| {
            span.kind == EvidenceKind::Owner && overlaps(start, end, span.start_ms, span.end_ms)
        })
    }

    /// Reconcile pending activity at its original coordinates. A newer
    /// independently confirmed Other observation can resolve older Unknown;
    /// missing coverage, gaps and any Owner evidence still preserve it.
    pub(crate) fn unresolved_activity_end_ms_after(&self, owner_end: u64) -> Option<u64> {
        let mut unknown: Vec<_> = self.presence.iter()
            .filter(|span| span.kind == EvidenceKind::Unknown && span.end_ms > owner_end)
            .collect();
        unknown.sort_unstable_by_key(|span| std::cmp::Reverse(span.end_ms));
        let mut latest = None;
        for span in unknown {
            if latest.is_some_and(|end| end >= span.end_ms) { break; }
            let start = span.start_ms.max(owner_end);
            let mut cuts = vec![start, span.end_ms];
            for proof in self.presence.iter().chain(&self.identity) {
                if proof.kind != EvidenceKind::Unknown
                    && overlaps(start, span.end_ms, proof.start_ms, proof.end_ms)
                {
                    cuts.push(start.max(proof.start_ms));
                    cuts.push(span.end_ms.min(proof.end_ms));
                }
            }
            cuts.sort_unstable(); cuts.dedup();
            for range in cuts.windows(2).rev() {
                if !self.confirms_other(range[0], range[1]) {
                    latest = Some(latest.unwrap_or(0).max(range[1]));
                    break;
                }
            }
        }
        latest
    }
}

fn covered(spans: &[SpeakerSpan], start: u64, end: u64, kind: EvidenceKind) -> bool {
    if start >= end {
        return false;
    }
    let mut ranges: Vec<_> = spans
        .iter()
        .filter(|span| span.kind == kind)
        .map(|span| (span.start_ms, span.end_ms))
        .collect();
    ranges.sort_unstable();
    let mut through = start;
    for (left, right) in ranges {
        if right <= through {
            continue;
        }
        if left > through {
            break;
        }
        through = through.max(right);
        if through >= end {
            return true;
        }
    }
    false
}

fn overlaps(a: u64, b: u64, c: u64, d: u64) -> bool {
    a < d && c < b
}
fn content(text: &str) -> String {
    text.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

fn millis(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

fn speaker_id(row: &Value) -> Option<String> {
    let value = row
        .get("additions")
        .and_then(|v| v.get("speaker_id").or_else(|| v.get("speaker")))
        .or_else(|| row.get("speaker_id"))
        .or_else(|| row.get("speaker"))?;
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|v| v.to_string()))
}

#[derive(Clone, Debug)]
pub(super) struct SourceRow {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) text: String,
    pub(super) speaker: Option<String>,
    pub(super) definite: bool,
    payload: Value,
    words: Vec<SourceWord>,
    retained: usize,
}

#[derive(Clone, Debug)]
struct SourceWord {
    start: u64,
    end: u64,
    key: String,
}

impl SourceRow {
    pub(super) fn word_ranges(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.words.iter().map(|word| (word.start, word.end))
    }
    fn visible_text(&self) -> String {
        if self.retained == 0 {
            return String::new();
        }
        if self.retained == self.words.len() {
            return self.text.clone();
        }
        let expected: String = self.words[..self.retained]
            .iter()
            .map(|word| word.key.as_str())
            .collect();
        let mut key = String::new();
        let mut boundary = 0;
        for (offset, ch) in self.text.char_indices() {
            if ch.is_alphanumeric() {
                if key == expected {
                    break;
                }
                key.push(ch);
            }
            boundary = offset + ch.len_utf8();
        }
        self.text[..boundary].trim_end().to_string()
    }

    fn visible_payload(&self) -> Value {
        let mut row = self.payload.clone();
        row["text"] = Value::String(self.visible_text());
        if self.retained < self.words.len() {
            row["text"] = Value::String(self.visible_text());
            if let Some(words) = row.get_mut("words").and_then(Value::as_array_mut) {
                let mut substantive = 0;
                words.retain(|word| {
                    if content(word.get("text").and_then(Value::as_str).unwrap_or_default())
                        .is_empty()
                    {
                        return true;
                    }
                    substantive += 1;
                    substantive <= self.retained
                });
            }
            row["end_time"] = Value::from(self.words[self.retained - 1].end);
        }
        row
    }
}

/// Coordinates must describe this rendering, not an older utterance branch.
/// ITN/word disagreement, missing clocks and empty rows keep the ordinary view.
pub(super) fn aligned_rows(result: &Value) -> Option<Vec<SourceRow>> {
    let rows = result.get("utterances")?.as_array()?;
    let mut parsed = Vec::new();
    let mut rendered = String::new();
    for row in rows {
        // A confirmed empty native occurrence has no text/words. It cannot
        // invalidate the coordinates of the spoken rows in the same packet.
        // Use the recognition certificate's exact validator; an ordinary
        // missing or contradictory row still keeps the source unaligned.
        if super::volcengine_transcript::has_explicit_empty_flag(row) {
            super::volcengine_transcript::explicit_confirmed_empty_range(row)?;
            continue;
        }
        let text = row.get("text")?.as_str()?.trim();
        if content(text).is_empty() {
            continue;
        }
        let words = row.get("words").and_then(Value::as_array);
        let mut word_text = String::new();
        let mut first = None;
        let mut last = None;
        let mut source_words = Vec::new();
        if let Some(words) = words.filter(|words| !words.is_empty()) {
            for word in words {
                let key = content(word.get("text").and_then(Value::as_str).unwrap_or_default());
                if key.is_empty() {
                    continue;
                }
                let start = millis(word, &["start_time", "startTime", "start_ms"])?;
                let end = millis(word, &["end_time", "endTime", "end_ms"])?;
                if start >= end || last.is_some_and(|last| start < last) {
                    return None;
                }
                first.get_or_insert(start);
                last = Some(end);
                word_text.push_str(&key);
                source_words.push(SourceWord { start, end, key });
            }
            if word_text != content(text) {
                return None;
            }
        }
        let start = first.or_else(|| millis(row, &["start_time", "startTime", "start_ms"]))?;
        let end = last.or_else(|| millis(row, &["end_time", "endTime", "end_ms"]))?;
        if start >= end {
            return None;
        }
        if source_words.is_empty() {
            source_words.push(SourceWord {
                start,
                end,
                key: content(text),
            });
        }
        rendered.push_str(text);
        parsed.push(SourceRow {
            start,
            end,
            text: text.into(),
            speaker: speaker_id(row),
            definite: row.get("definite").and_then(Value::as_bool) == Some(true),
            payload: row.clone(),
            retained: source_words.len(),
            words: source_words,
        });
    }
    let main = result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if parsed.is_empty() || content(&rendered) != content(main) {
        return None;
    }
    // Keep provider punctuation and spacing at the original row boundaries.
    let mut cursor = 0;
    for row in &mut parsed {
        let wanted = content(&row.text).chars().count();
        let mut spoken = 0;
        let mut boundary = cursor;
        for (offset, ch) in main[cursor..].char_indices() {
            if ch.is_alphanumeric() {
                if spoken == wanted {
                    break;
                }
                spoken += 1;
            }
            boundary = cursor + offset + ch.len_utf8();
        }
        row.text = main[cursor..boundary].to_string();
        cursor = boundary;
    }
    Some(parsed)
}

fn append_source_text(output: &mut String, text: &str) {
    let left = output.chars().next_back();
    let right = text.chars().next();
    if left.zip(right).is_some_and(|(left, right)| {
        (left.is_ascii_alphanumeric() || matches!(left, '.' | '?' | '!' | ';' | ':' | ','))
            && right.is_ascii_alphanumeric()
    }) {
        output.push(' ');
    }
    output.push_str(text);
}

#[derive(Clone, Debug)]
struct SourceWindow {
    rows: Vec<SourceRow>,
    prefix: String,
    rendered: String,
}

#[derive(Clone, Copy)]
enum ResponseScope {
    FullCapture,
    Window,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ConservativeSpeakerLedger {
    owner_ids: std::collections::BTreeSet<String>,
    window: Option<SourceWindow>,
    #[cfg(target_os = "windows")]
    native_words: Option<super::volcengine_native_identity::NativeWordIdentity>,
}

#[derive(Clone, Debug)]
pub(crate) struct SpeakerProjection {
    pub result: Value,
    /// A source-proven revision, with older text outside the window retained.
    pub revised_ledger: Option<String>,
    pub excluded_rows: usize,
    pub speaker_ids: Vec<String>,
    pub owner_id: Option<String>,
    pub owner_ids: Vec<String>,
    /// Protection against removal only. Positive activity need not also
    /// pass the independent identity model to protect its entire native
    /// track. This does not name that track as the enrolled owner.
    pub activity_protected_ids: Vec<String>,
    pub coordinates_aligned: bool,
}

impl ConservativeSpeakerLedger {
    #[cfg(target_os = "windows")]
    pub(crate) fn set_native_word_identity(&mut self, words: super::volcengine_native_identity::NativeWordIdentity) {
        self.native_words = words.qualified_reference.then_some(words);
    }

    pub(crate) fn native_word_identity_diagnostics(&self) -> Value {
        #[cfg(target_os = "windows")]
        if let Some(native)=self.native_words.as_ref() {
            return serde_json::json!({"qualified_reference":true,
                "source_rows":native.rows.iter().map(|(start,end,kind)|
                    serde_json::json!([start,end,format!("{:?}",kind)])).collect::<Vec<_>>(),
                "source_activity":native.rows.iter().map(|(start,end,_)|
                    serde_json::json!({"range_ms":[start,end],
                        "raw_observed_without_owner":native.detector_excludes_owner(*start,*end),
                        "raw_owner_vetoes":native.raw_activity.presence.iter().filter(|span|
                            span.kind==EvidenceKind::Owner && overlaps(*start,*end,span.start_ms,span.end_ms))
                            .map(|span|[span.start_ms,span.end_ms]).collect::<Vec<_>>()})).collect::<Vec<_>>()});
        }
        serde_json::json!({"qualified_reference":false,"source_rows":[]})
    }
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn project_full_capture(
        &mut self,
        result: &Value,
        local: &LocalSpeakerTimeline,
        previous_ledger: &str,
    ) -> SpeakerProjection {
        self.project_with_scope(result, local, previous_ledger, ResponseScope::FullCapture)
    }

    pub(crate) fn project(
        &mut self,
        result: &Value,
        local: &LocalSpeakerTimeline,
        previous_ledger: &str,
    ) -> SpeakerProjection {
        self.project_with_scope(result, local, previous_ledger, ResponseScope::Window)
    }

    fn project_with_scope(
        &mut self,
        result: &Value,
        local: &LocalSpeakerTimeline,
        previous_ledger: &str,
        scope: ResponseScope,
    ) -> SpeakerProjection {
        let mut view = SpeakerProjection {
            result: result.clone(),
            revised_ledger: None,
            excluded_rows: 0,
            speaker_ids: result
                .get("utterances")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(speaker_id)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            owner_id: self.owner_ids.first().cloned(),
            owner_ids: self.owner_ids.iter().cloned().collect(),
            activity_protected_ids: Vec::new(),
            coordinates_aligned: false,
        };
        let Some(incoming) = aligned_rows(result) else {
            // Never borrow a valid earlier packet's range for a new spelling.
            return view;
        };
        view.coordinates_aligned = true;
        let source_text = result
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (mut rows, prefix) = if matches!(scope, ResponseScope::FullCapture) {
            // The request's result_type=full makes this packet the current
            // rendering of the whole capture. Two-pass word clocks/spelling
            // may move or shrink; old word bounds do not own that revision.
            // Word-level identity still uses this packet's validated clocks.
            (incoming.clone(), String::new())
        } else if let Some(previous) = &self.window {
            if content(previous_ledger) != content(&previous.rendered) {
                // An unaligned preview introduced new words. Do not overwrite
                // them until this packet explicitly covers their rendering.
                if !content(source_text).starts_with(&content(previous_ledger)) {
                    return view;
                }
                (incoming.clone(), String::new())
            } else {
                let spans: Vec<_> = incoming
                    .iter()
                    .map(|row| SpeakerSpan {
                        start_ms: row.start,
                        end_ms: row.end,
                        kind: EvidenceKind::Unknown,
                    })
                    .collect();
                // A partial overlap does not own the earlier words of a row.
                // Wait rather than discard those owner words or append a copy.
                if previous.rows.iter().any(|old| {
                    incoming
                        .iter()
                        .any(|new| overlaps(old.start, old.end, new.start, new.end))
                        && !covered(&spans, old.start, old.end, EvidenceKind::Unknown)
                }) {
                    return view;
                }
                let mut merged: Vec<_> = previous
                    .rows
                    .iter()
                    .filter(|old| {
                        !incoming
                            .iter()
                            .any(|new| overlaps(old.start, old.end, new.start, new.end))
                    })
                    .cloned()
                    .collect();
                merged.extend(incoming.clone());
                merged.sort_by_key(|row| row.start);
                (merged, previous.prefix.clone())
            }
        } else if previous_ledger.trim().is_empty()
            || content(source_text).starts_with(&content(previous_ledger))
        {
            (incoming.clone(), String::new())
        } else if let Some(prefix) = previous_ledger.trim_end().strip_suffix(source_text) {
            (incoming.clone(), prefix.to_string())
        } else {
            return view;
        };
        // A newly pending tail must not resurrect an unchanged occurrence
        // that was independently excluded in an earlier full/window packet.
        if let Some(previous) = &self.window {
            for row in &mut rows {
                if let Some(old) = previous.rows.iter().find(|old| {
                    old.start == row.start
                        && old.end == row.end
                        && old.speaker == row.speaker
                        && content(&old.text) == content(&row.text)
                }) {
                    row.retained = old.retained;
                }
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &rows {
            if let Some(id) = &row.speaker {
                ids.insert(id.clone());
            }
        }
        view.speaker_ids = ids.into_iter().collect();
        let mut owner_ids = std::collections::BTreeSet::new();
        for verified in local
            .identity
            .iter()
            .filter(|span| span.kind == EvidenceKind::Owner)
        {
            let intersecting: Vec<_> = rows
                .iter()
                .filter(|row| overlaps(row.start, row.end, verified.start_ms, verified.end_ms))
                .collect();
            if intersecting.is_empty()
                || intersecting
                    .iter()
                    .any(|row| !row.definite || row.speaker.is_none())
            {
                continue;
            }
            let candidates: std::collections::BTreeSet<_> = intersecting
                .iter()
                .filter_map(|row| row.speaker.as_ref())
                .collect();
            // Identity verifies a whole model window; activity describes its
            // shorter chunks. Naming requires positive activity in actual
            // words inside that verified window, not every chunk being Owner.
            // Unknown gaps do not erase positive identity. A mixed/ambiguous
            // source window still cannot name either of its speakers.
            if candidates.len() == 1
                && intersecting.iter().any(|row| {
                    row.words.iter().any(|word| {
                        let start = verified.start_ms.max(word.start);
                        let end = verified.end_ms.min(word.end);
                        start < end
                            && local.presence.iter().any(|span| {
                                span.kind == EvidenceKind::Owner
                                    && overlaps(start, end, span.start_ms, span.end_ms)
                            })
                    })
                })
            {
                owner_ids.insert((*candidates.first().unwrap()).clone());
            }
        }
        // Different disjoint windows can independently verify aliases of one
        // enrolled person (wake and body may receive different cloud IDs).
        // Rebuild from current source rows, so a relabelled old ID is not kept.
        #[cfg(target_os = "windows")]
        if let Some(native) = self.native_words.as_ref() {
            // A rolling identity window cannot bind a native track when its
            // own exact-source observations are available. Rebuild after each
            // provider spelling, timing and anonymous-ID revision.
            owner_ids = rows.iter().filter(|row| native.kind(row) == EvidenceKind::Owner)
                .filter_map(|row| row.speaker.clone()).collect();
        }
        self.owner_ids = owner_ids;
        view.owner_id = self.owner_ids.first().cloned();
        view.owner_ids = self.owner_ids.iter().cloned().collect();

        let mut protected_ids = self.owner_ids.clone();
        #[cfg(target_os = "windows")]
        let source_scoped = self.native_words.is_some();
        #[cfg(not(target_os = "windows"))]
        let source_scoped = false;
        for row in &rows {
            if !source_scoped && row.definite && row.words.iter().any(|word| local.owner_vetoes(word.start, word.end)) {
                if let Some(id) = &row.speaker { protected_ids.insert(id.clone()); }
            }
        }
        view.activity_protected_ids = protected_ids.iter().cloned().collect();

        for row in &mut rows {
            let excluded_owner = row.words[row.retained..]
                .iter()
                .any(|word| local.owner_vetoes(word.start, word.end));
            if protected_ids.is_empty()
                || row
                    .speaker
                    .as_ref()
                    .is_some_and(|id| protected_ids.contains(id))
                || excluded_owner
            {
                row.retained = row.words.len();
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(native) = self.native_words.as_ref() {
            for index in 0..rows.len() {
                let row = &rows[index];
                let qualified_other_source = !self.owner_ids.is_empty()
                    && native.kind(row) == EvidenceKind::Other
                    && row.speaker.as_ref().is_some_and(|id| !self.owner_ids.contains(id))
                    && !rows.iter().any(|other| other.speaker != row.speaker
                        && overlaps(row.start, row.end, other.start, other.end));
                let proven_other = qualified_other_source
                    && native.detector_excludes_owner(row.start, row.end);
                // Unknown and weak sources remain visible. A different
                // qualified Other source cannot inherit a positive that came
                // from context elsewhere on the same anonymous video track.
                let mut retain = if proven_other { 0 } else { row.words.len() };
                if qualified_other_source && !proven_other {
                    // A positive/unknown at a turn boundary protects its own
                    // complete words, not every later word in this source.
                    // Partial exclusion needs BOTH exact native source identity
                    // and complete independent Other/absence coverage. This is
                    // stricter than the ordinary rolling-window tail fallback.
                    while retain > 0 {
                        let word = &row.words[retain - 1];
                        if !native.detector_excludes_owner(word.start, word.end)
                            || !local.confirms_other(word.start, word.end)
                        { break; }
                        retain -= 1;
                    }
                }
                rows[index].retained = retain;
            }
        }
        if !source_scoped && !protected_ids.is_empty() {
            for index in (0..rows.len()).rev() {
                let row = &rows[index];
                if !row.definite
                    || !row
                        .speaker
                        .as_ref()
                        .is_some_and(|id| !protected_ids.contains(id))
                {
                    break;
                }
                let mut retain = row.retained;
                while retain > 0 {
                    let word = &row.words[retain - 1];
                    if !local.confirms_other(word.start, word.end)
                        || rows.iter().any(|other| {
                            other
                                .speaker
                                .as_ref()
                                .is_some_and(|id| protected_ids.contains(id))
                                && overlaps(word.start, word.end, other.start, other.end)
                        })
                    {
                        break;
                    }
                    retain -= 1;
                }
                rows[index].retained = retain;
                if retain > 0 {
                    break;
                }
            }
        }
        view.excluded_rows = rows
            .iter()
            .filter(|row| row.retained < row.words.len())
            .count();
        let mut rendered = prefix.clone();
        for row in &rows {
            append_source_text(&mut rendered, &row.visible_text());
        }
        let rendered = rendered.trim_end().to_string();
        view.result["text"] = Value::String(rendered.clone());
        view.result["utterances"] = Value::Array(
            rows.iter()
                .filter(|row| row.retained > 0)
                .map(|row| row.visible_payload())
                .collect(),
        );
        view.revised_ledger = Some(rendered.clone());
        self.window = Some(SourceWindow {
            rows,
            prefix,
            rendered,
        });
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(target_os = "windows")]
    #[test]
    fn native_partial_activity_veto_protects_words_without_protecting_other_tail() {
        use super::super::volcengine_native_identity::NativeWordIdentity;
        let mut source=packet(vec![row("Owner.","1",100,900),json!({
            "text":"Boundary uncertain confirmed other.","definite":true,
            "additions":{"speaker_id":"2"},"start_time":1000,"end_time":4000,
            "words":[{"text":"Boundary","start_time":1000,"end_time":1200},
                {"text":"uncertain","start_time":1300,"end_time":1500},
                {"text":"confirmed","start_time":1800,"end_time":2500},
                {"text":"other","start_time":2800,"end_time":4000}]} )]);
        source["text"]=json!("Owner. Boundary uncertain confirmed other.");
        let local=LocalSpeakerTimeline {identity:vec![span(1000,1160,EvidenceKind::Owner),span(1600,4000,EvidenceKind::Other)],
            presence:vec![span(1000,1160,EvidenceKind::Owner),span(1600,4000,EvidenceKind::Other)]};
        let mut native=NativeWordIdentity::default();
        native.qualified_reference=true;
        native.rows=vec![(100,900,EvidenceKind::Owner),(1000,4000,EvidenceKind::Other)];
        native.raw_activity.presence=vec![span(0,4000,EvidenceKind::Unknown),span(1000,1160,EvidenceKind::Owner)];
        let project=|native:NativeWordIdentity,local:&LocalSpeakerTimeline| {
            let mut ledger=ConservativeSpeakerLedger::default();
            ledger.set_native_word_identity(native);
            ledger.project_full_capture(&source,local,"")
        };
        let projected=project(native.clone(),&local);
        assert_eq!(projected.result["text"],"Owner. Boundary uncertain");
        assert_eq!(projected.result["utterances"][1]["words"].as_array().unwrap().len(),2);
        assert_eq!(projected.result["utterances"][1]["end_time"],1500);
        // Unknown words cannot inherit the later confirmed Other decision.
        assert_eq!(project(native.clone(),&LocalSpeakerTimeline::default()).result["text"],source["text"]);
        // Incomplete raw observation is not an absence certificate.
        let mut missing=native.clone();
        missing.raw_activity.presence[0].end_ms=3500;
        assert_eq!(project(missing,&local).result["text"],source["text"]);
        // A positive within a later word preserves that whole word and prefix.
        let mut positive=native.clone();
        positive.raw_activity.note_presence(span(2200,2360,EvidenceKind::Owner));
        assert_eq!(project(positive,&local).result["text"],"Owner. Boundary uncertain confirmed");
        // The old rolling negative alone cannot authorize this new branch.
        for kind in [EvidenceKind::Owner,EvidenceKind::Unknown] {
            let mut unqualified=native.clone(); unqualified.rows[1].2=kind;
            assert_eq!(project(unqualified,&local).result["text"],source["text"]);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn source_identity_does_not_extend_mixed_activity_protection_to_video_track() {
        use super::super::volcengine_native_identity::NativeWordIdentity;
        let source=packet(vec![row("Own one.","1",100,1000),row("Video one.","0",1200,2400),
            row("Own two.","1",2500,3500),row("Short video.","0",3800,4000)]);
        let local=LocalSpeakerTimeline {identity:vec![span(0,2400,EvidenceKind::Owner),span(1200,2400,EvidenceKind::Other)],
            presence:vec![span(100,1600,EvidenceKind::Owner),span(1200,2400,EvidenceKind::Other),span(2500,3500,EvidenceKind::Owner)]};
        let mut native=NativeWordIdentity::default();
        native.qualified_reference=true;
        native.rows=vec![(100,1000,EvidenceKind::Owner),(1200,2400,EvidenceKind::Other),(2500,3500,EvidenceKind::Owner),(3800,4000,EvidenceKind::Unknown)];
        native.raw_activity.note_presence(span(0,4160,EvidenceKind::Unknown));
        native.raw_activity.note_presence(span(100,1000,EvidenceKind::Owner));
        native.raw_activity.note_presence(span(2500,3500,EvidenceKind::Owner));
        let mut ledger=ConservativeSpeakerLedger::default();
        ledger.set_native_word_identity(native.clone());
        let projected=ledger.project_full_capture(&source,&local,"");
        assert_eq!(projected.result["text"],"Own one. Own two. Short video.");
        assert_eq!(projected.owner_ids,vec!["1"]);
        assert_eq!(projected.activity_protected_ids,vec!["1"]);
        assert_eq!(projected.excluded_rows,1);
        // A genuine detector positive on the foreign source still vetoes
        // deletion. Neither the new profile nor native labels override it.
        native.raw_activity.note_presence(span(1500,1660,EvidenceKind::Owner));
        ledger.set_native_word_identity(native);
        assert_eq!(ledger.project_full_capture(&source,&local,"").result["text"],"Own one. Video one. Own two. Short video.");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn source_identity_preserves_weak_owner_aliases_overlap_and_unobserved_audio() {
        use super::super::volcengine_native_identity::NativeWordIdentity;
        let source=packet(vec![row("Owner.","1",100,1000),row("Quiet owner.","1",1200,2400),
            row("Uncertain.","2",2500,3500),row("Overlap.","3",900,1300),row("Unavailable.","4",3800,4500)]);
        let mut native=NativeWordIdentity::default();
        native.qualified_reference=true;
        native.rows=vec![(100,1000,EvidenceKind::Owner),(1200,2400,EvidenceKind::Other),
            (2500,3500,EvidenceKind::Unknown),(900,1300,EvidenceKind::Other),(3800,4500,EvidenceKind::Other)];
        native.raw_activity.note_presence(span(0,4000,EvidenceKind::Unknown));
        let old_bad_negative=LocalSpeakerTimeline { identity:vec![span(1000,4500,EvidenceKind::Other)],
            presence:vec![span(1000,4500,EvidenceKind::Other)] };
        let mut ledger=ConservativeSpeakerLedger::default();
        ledger.set_native_word_identity(native);
        assert_eq!(ledger.project_full_capture(&source,&old_bad_negative,"").result["text"],"Owner. Quiet owner. Uncertain. Overlap. Unavailable.");
        // A provider relabelled Owner uses the current source identity; the
        // previous anonymous ID cannot grant new foreign words protection.
        let relabelled=packet(vec![row("Owner.","9",100,1000),row("Foreign.","1",2500,3500)]);
        let mut native=NativeWordIdentity::default();
        native.qualified_reference=true;
        native.rows=vec![(100,1000,EvidenceKind::Owner),(2500,3500,EvidenceKind::Other)];
        native.raw_activity.note_presence(span(0,4000,EvidenceKind::Unknown));
        ledger.set_native_word_identity(native);
        let projected=ledger.project_full_capture(&relabelled,&old_bad_negative,"");
        assert_eq!(projected.owner_ids,vec!["9"]);
        assert_eq!(projected.result["text"],"Owner.");
    }

    #[test]
    fn pending_frontier_recedes_only_across_complete_dual_other_evidence() {
        let mut timeline=LocalSpeakerTimeline {
            identity:vec![span(1000,3000,EvidenceKind::Other)],
            presence:vec![span(1000,3000,EvidenceKind::Unknown)],
        };
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),Some(3000));
        timeline.note_presence(span(2000,3000,EvidenceKind::Other));
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),Some(2000));
        timeline.note_presence(span(1000,2000,EvidenceKind::Other));
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),None);
        timeline.note_presence(span(1450,1460,EvidenceKind::Owner));
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),Some(1460));
        timeline.identity=vec![span(1000,1800,EvidenceKind::Other),span(2000,3000,EvidenceKind::Other)];
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),Some(2000),"unobserved identity gap survives");
        timeline.identity.clear();
        assert_eq!(timeline.unresolved_activity_end_ms_after(1000),Some(3000));
    }

    fn row(text: &str, id: &str, start: u64, end: u64) -> Value {
        json!({"text":text,"definite":true,"additions":{"source":"two_pass","speaker_id":id},
            "words":[{"text":text,"start_time":start,"end_time":end}]})
    }
    fn packet(rows: Vec<Value>) -> Value {
        let text: String = rows
            .iter()
            .map(|row| row["text"].as_str().unwrap())
            .collect();
        json!({"text":text,"utterances":rows})
    }
    fn span(start_ms: u64, end_ms: u64, kind: EvidenceKind) -> SpeakerSpan {
        SpeakerSpan {
            start_ms,
            end_ms,
            kind,
        }
    }
    fn local() -> LocalSpeakerTimeline {
        LocalSpeakerTimeline {
            identity: vec![
                span(0, 1000, EvidenceKind::Owner),
                span(2000, 5000, EvidenceKind::Other),
            ],
            presence: vec![
                span(0, 1000, EvidenceKind::Owner),
                span(2000, 5000, EvidenceKind::Other),
            ],
        }
    }
    fn text(view: &SpeakerProjection) -> &str {
        view.result["text"].as_str().unwrap()
    }

    #[test]
    fn only_independently_bound_other_tail_is_excluded() {
        let source = packet(vec![
            row("明天开会。", "owner", 0, 1000),
            row("广告台词。", "video", 2000, 3000),
        ]);
        let view = ConservativeSpeakerLedger::default().project(&source, &local(), "");
        assert_eq!(text(&view), "明天开会。");
        assert_eq!(view.excluded_rows, 1);
        assert_eq!(view.owner_id.as_deref(), Some("owner"));
        let mut only_presence = local();
        only_presence.identity.clear();
        let view = ConservativeSpeakerLedger::default().project(&source, &only_presence, "");
        assert_eq!(text(&view), source["text"]);
        assert_eq!(view.excluded_rows, 0);
    }

    #[test]
    fn activity_protection_does_not_require_naming_the_owner_and_does_not_grant_identity() {
        let source = packet(vec![row("本人正文。", "a", 0, 1000), row("外放尾句。", "b", 2000, 3000)]);
        let mut evidence = local();
        evidence.identity[0].kind = EvidenceKind::Unknown;
        let view = ConservativeSpeakerLedger::default().project_full_capture(&source, &evidence, "");
        assert!(view.owner_ids.is_empty(), "activity must not become identity");
        assert_eq!(view.activity_protected_ids, vec!["a"]);
        assert_eq!(text(&view), "本人正文。");
        assert_eq!(view.excluded_rows, 1);
        let same_track = packet(vec![row("本人正文。", "a", 0, 1000), row("轻声尾句。", "a", 2000, 3000)]);
        assert_eq!(text(&ConservativeSpeakerLedger::default().project_full_capture(&same_track, &evidence, "")), same_track["text"], "a protected track survives later model disagreement");
        for missing in ["activity", "identity", "anchor"] {
            let mut uncertain = evidence.clone();
            match missing {
                "activity" => uncertain.presence[1].kind = EvidenceKind::Unknown,
                "identity" => uncertain.identity[1].kind = EvidenceKind::Unknown,
                _ => uncertain.presence[0].kind = EvidenceKind::Unknown,
            }
            assert_eq!(text(&ConservativeSpeakerLedger::default().project_full_capture(&source, &uncertain, "")), source["text"], "missing {missing} must preserve words");
        }
        evidence.presence.push(span(2400, 2410, EvidenceKind::Owner));
        assert_eq!(text(&ConservativeSpeakerLedger::default().project_full_capture(&source, &evidence, "")), source["text"], "one owner overlap protects a returning/split owner track");
    }

    #[test]
    fn native_confirmed_empty_occurrence_keeps_the_spoken_source_partition() {
        let source = packet(vec![row("本人。", "a", 0, 1000), row("外放。", "b", 2000, 3000)]);
        let empty = json!({"start_time":3000,"end_time":3400,"definite":true,
            "additions":{"source":"two_pass","two_pass_empty":"true"}});
        let mut sealed = source.clone();
        sealed["utterances"].as_array_mut().unwrap().push(empty);
        let view = ConservativeSpeakerLedger::default().project_full_capture(&sealed, &local(), "");
        assert!(view.coordinates_aligned);
        assert_eq!(text(&view), "本人。");
        for failure in ["unmarked", "provisional", "bad_time", "contradiction"] {
            let mut bad = sealed.clone();
            let last = &mut bad["utterances"][2];
            match failure {
                "unmarked" => { last["additions"].as_object_mut().unwrap().remove("two_pass_empty"); },
                "provisional" => last["definite"] = json!(false),
                "bad_time" => last["end_time"] = json!(2999),
                _ => last["words"] = json!([{"text":"没有资格删除","start_time":3100,"end_time":3300}]),
            }
            let view = ConservativeSpeakerLedger::default().project_full_capture(&bad, &local(), "");
            assert!(!view.coordinates_aligned, "invalid {failure} cannot authorize removal");
            assert_eq!(text(&view), bad["text"]);
        }
    }

    #[test]
    fn short_quiet_owner_unknown_and_mixed_speech_are_preserved() {
        let source = packet(vec![
            row("明天开会。", "owner", 0, 1000),
            row("轻声尾句。", "split", 2000, 3000),
        ]);
        for state in [EvidenceKind::Owner, EvidenceKind::Unknown] {
            let mut evidence = local();
            evidence.presence[1].kind = state;
            let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
            assert_eq!(text(&view), source["text"]);
        }
        let mut evidence = local();
        evidence
            .presence
            .push(span(2400, 2600, EvidenceKind::Owner));
        let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
        assert_eq!(text(&view), source["text"]);
        let source = packet(vec![
            row("本人正文。", "owner", 0, 3000),
            row("重叠声音。", "video", 2000, 3000),
        ]);
        let view = ConservativeSpeakerLedger::default().project(&source, &local(), "");
        assert_eq!(view.excluded_rows, 0);
    }

    #[test]
    fn missing_gap_lagging_or_stale_coordinates_do_not_exclude() {
        let source = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("尾句。", "video", 2000, 3000),
        ]);
        let mut evidence = local();
        evidence.presence = vec![
            span(0, 1000, EvidenceKind::Owner),
            span(2000, 2300, EvidenceKind::Other),
            span(2400, 3000, EvidenceKind::Other),
        ];
        assert_eq!(
            ConservativeSpeakerLedger::default()
                .project(&source, &evidence, "")
                .excluded_rows,
            0
        );
        evidence.presence.truncate(2);
        assert_eq!(
            ConservativeSpeakerLedger::default()
                .project(&source, &evidence, "")
                .excluded_rows,
            0
        );
        let mut stale = source.clone();
        stale["text"] = json!("本人。这句被二遍重新改了。");
        let view = ConservativeSpeakerLedger::default().project(&stale, &local(), "");
        assert!(!view.coordinates_aligned);
        assert_eq!(text(&view), stale["text"]);
    }

    #[test]
    fn a_foreign_sentence_can_retain_uncertain_front_and_exclude_confirmed_word_suffix() {
        let mut source = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("可能本人。确定旁人。", "video", 1500, 3000),
        ]);
        source["utterances"][1]["words"] = json!([
            {"text":"可能本人。","start_time":1500,"end_time":1900},
            {"text":"确定旁人。","start_time":2100,"end_time":3000}]);
        let view = ConservativeSpeakerLedger::default().project(&source, &local(), "");
        assert_eq!(text(&view), "本人。可能本人。");
        assert_eq!(
            view.result["utterances"][1]["words"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(view.result["utterances"][1]["end_time"], 1900);
    }

    #[test]
    fn rolling_later_video_does_not_reintroduce_the_excluded_occurrence() {
        let mut ledger = ConservativeSpeakerLedger::default();
        let first = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("广告。", "video", 2000, 3000),
        ]);
        let view = ledger.project(&first, &local(), "");
        let later = packet(vec![row("继续广告。", "video", 3000, 4000)]);
        let view = ledger.project(&later, &local(), text(&view));
        assert_eq!(text(&view), "本人。");
        let resumed = packet(vec![row("继续本人。", "owner", 6000, 7000)]);
        let mut evidence = local();
        evidence
            .identity
            .push(span(6000, 7000, EvidenceKind::Owner));
        evidence
            .presence
            .push(span(6000, 7000, EvidenceKind::Owner));
        let view = ledger.project(&resumed, &evidence, text(&view));
        assert_eq!(text(&view), "本人。继续本人。");
    }

    #[test]
    fn equal_length_two_pass_rewrite_replaces_audio_occurrence_without_repeating_the_header() {
        // These are explicit synthetic source coordinates, not recovered
        // timestamps from the text-only 17:46 private incident trace.
        let old = "现在遇到这个问题，怎么处理？就是。哦，就是以前没有办法，因为云端不能控制，然后不想付钱做生成视频。";
        let new = "现在遇到这个问题，怎么处理？就是。呃，就是以前没有办法，因为云端不能控制，然后不想付钱做声纹识别。";
        let mut ledger = ConservativeSpeakerLedger::default();
        let first = ledger.project(
            &packet(vec![row(old, "0", 4000, 22000)]),
            &Default::default(),
            "",
        );
        let revised = ledger.project(
            &packet(vec![row(new, "0", 4000, 22000)]),
            &Default::default(),
            text(&first),
        );
        assert_eq!(text(&revised), new);
        let tail = format!("{new}就是本地能做什么，请想一下。");
        let final_view = ledger.project(
            &packet(vec![row(&tail, "0", 4000, 28000)]),
            &Default::default(),
            text(&revised),
        );
        assert_eq!(text(&final_view), tail);
    }

    #[test]
    fn genuine_repeated_words_and_shared_wake_in_separate_audio_are_not_collapsed() {
        let mut ledger = ConservativeSpeakerLedger::default();
        let first = packet(vec![
            row("开始录音。", "0", 0, 500),
            row("真的真的。", "0", 1000, 2000),
        ]);
        let view = ledger.project(&first, &Default::default(), "");
        let second = packet(vec![
            row("开始录音。", "0", 0, 500),
            row("真的真的。", "0", 3000, 4000),
        ]);
        let view = ledger.project(&second, &Default::default(), text(&view));
        assert_eq!(text(&view), "开始录音。真的真的。真的真的。");
        let full = packet(vec![
            row("开始录音。", "0", 0, 500),
            row("真的真的。", "0", 1000, 2000),
            row("真的真的。", "0", 3000, 4000),
        ]);
        let view = ledger.project(&full, &Default::default(), text(&view));
        assert_eq!(text(&view), full["text"]);
    }

    #[test]
    fn unseen_preview_words_partial_overlap_and_new_capture_keep_their_boundaries() {
        let mut ledger = ConservativeSpeakerLedger::default();
        ledger.project(
            &packet(vec![row("原来的话。", "0", 0, 2000)]),
            &Default::default(),
            "",
        );
        let source = packet(vec![row("后面的话。", "0", 1500, 3000)]);
        let view = ledger.project(&source, &Default::default(), "原来的话。");
        assert!(view.revised_ledger.is_none());
        let view = ledger.project(
            &packet(vec![row("原来的话。", "0", 0, 2000)]),
            &Default::default(),
            "原来的话。还有没对上时间的新字。",
        );
        assert!(view.revised_ledger.is_none());
        ledger.reset();
        let view = ledger.project(&packet(vec![row("新会话。", "1", 0, 1000)]), &local(), "");
        assert_eq!(text(&view), "新会话。");
    }

    #[test]
    fn english_spacing_asian_words_and_speaker_ids_do_not_depend_on_language_text() {
        let mut ledger = ConservativeSpeakerLedger::default();
        let mut source = packet(vec![
            row("Hello.", "0", 0, 1000),
            row("How are you?", "0", 2000, 3000),
        ]);
        source["text"] = json!("Hello. How are you?");
        let view = ledger.project(&source, &Default::default(), "");
        assert_eq!(text(&view), "Hello. How are you?");
        let view = ledger.project(
            &packet(vec![row("ありがとうございます。", "0", 4000, 5000)]),
            &Default::default(),
            text(&view),
        );
        assert_eq!(text(&view), "Hello. How are you?ありがとうございます。");
    }

    #[test]
    fn wake_body_aliases_verified_in_separate_windows_are_both_owner() {
        let source = packet(vec![
            row("开始录音。", "wake", 0, 1000),
            row("本人正文。", "body", 2000, 3000),
            row("视频台词。", "video", 4000, 5000),
        ]);
        let evidence = LocalSpeakerTimeline {
            identity: vec![
                span(0, 1000, EvidenceKind::Owner),
                span(2000, 3000, EvidenceKind::Owner),
                span(4000, 5000, EvidenceKind::Other),
            ],
            presence: vec![
                span(0, 1000, EvidenceKind::Owner),
                span(2000, 3000, EvidenceKind::Owner),
                span(4000, 5000, EvidenceKind::Other),
            ],
        };
        let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
        assert_eq!(text(&view), "开始录音。本人正文。");
        assert_eq!(view.owner_ids, vec!["body", "wake"]);
        assert_eq!(view.excluded_rows, 1);
    }

    #[test]
    fn qualified_identity_with_partial_activity_binds_without_weakening_other_word_coverage() {
        let source = packet(vec![
            row("本人轻声正文。", "owner", 0, 1000),
            row("确定旁人。", "video", 2000, 3000),
        ]);
        let mut evidence = local();
        evidence.presence = vec![
            span(0, 160, EvidenceKind::Unknown),
            span(160, 320, EvidenceKind::Owner),
            span(320, 1000, EvidenceKind::Unknown),
            span(2000, 3000, EvidenceKind::Other),
        ];
        let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
        assert_eq!(view.owner_ids, vec!["owner"]);
        assert_eq!(text(&view), "本人轻声正文。");
        evidence.presence[3].end_ms = 2500;
        let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
        assert_eq!(view.owner_ids, vec!["owner"]);
        assert_eq!(text(&view), source["text"]);
        assert_eq!(
            view.excluded_rows, 0,
            "partial negative activity cannot exclude a whole word"
        );
    }

    #[test]
    fn binding_needs_identity_and_positive_activity_in_the_same_native_word_range() {
        let mut source = packet(vec![row("前半后半。", "owner", 0, 1000)]);
        source["utterances"][0]["words"] = json!([
            {"text":"前半","start_time":0,"end_time":200},
            {"text":"后半。","start_time":800,"end_time":1000},
        ]);
        for presence in [
            span(0, 1000, EvidenceKind::Unknown),
            span(0, 1000, EvidenceKind::Other),
            span(1000, 1160, EvidenceKind::Owner),
            span(400, 560, EvidenceKind::Owner), // Within the row, outside its words.
            span(800, 960, EvidenceKind::Owner), // Native word, outside verified identity.
        ] {
            let evidence = LocalSpeakerTimeline {
                identity: vec![span(0, 700, EvidenceKind::Owner)],
                presence: vec![presence],
            };
            let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
            assert!(view.owner_ids.is_empty());
            assert_eq!(text(&view), source["text"]);
        }
        for kind in [EvidenceKind::Other, EvidenceKind::Unknown] {
            let evidence = LocalSpeakerTimeline {
                identity: vec![span(0, 700, kind)],
                presence: vec![span(0, 160, EvidenceKind::Owner)],
            };
            let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
            assert!(view.owner_ids.is_empty());
            assert_eq!(text(&view), source["text"]);
        }
    }

    #[test]
    fn partial_positive_cannot_name_mixed_pending_or_unlabelled_source_windows() {
        let evidence = LocalSpeakerTimeline {
            identity: vec![span(0, 1000, EvidenceKind::Owner)],
            presence: vec![span(0, 160, EvidenceKind::Owner)],
        };
        let source = packet(vec![
            row("第一段。", "a", 0, 500),
            row("第二段。", "b", 500, 1000),
        ]);
        let mut pending = packet(vec![row("第一段。", "a", 0, 1000)]);
        pending["utterances"][0]["definite"] = json!(false);
        let mut unlabelled = pending.clone();
        unlabelled["utterances"][0]["definite"] = json!(true);
        unlabelled["utterances"][0]["additions"] = json!({});
        for source in [source, pending, unlabelled] {
            let view = ConservativeSpeakerLedger::default().project(&source, &evidence, "");
            assert!(view.owner_ids.is_empty());
            assert_eq!(text(&view), source["text"]);
        }
    }

    #[test]
    fn provider_relabelling_rebuilds_partial_positive_identity_aliases() {
        let evidence = LocalSpeakerTimeline {
            identity: vec![span(0, 1000, EvidenceKind::Owner)],
            presence: vec![span(0, 160, EvidenceKind::Owner)],
        };
        let mut ledger = ConservativeSpeakerLedger::default();
        let first = ledger.project(
            &packet(vec![row("本人正文。", "old", 0, 1000)]),
            &evidence,
            "",
        );
        assert_eq!(first.owner_ids, vec!["old"]);
        let next = ledger.project(
            &packet(vec![row("本人正文。", "new", 0, 1000)]),
            &evidence,
            text(&first),
        );
        assert_eq!(next.owner_ids, vec!["new"]);
        assert_eq!(text(&next), "本人正文。");
    }

    #[test]
    fn full_capture_contract_accepts_shorter_word_clock_and_spelling_revisions() {
        let old = packet(vec![row(
            "明天9点开会，请把说明整理好。",
            "owner",
            280,
            7300,
        )]);
        let corrected = packet(vec![row(
            "明天九点开会，请把说明整理好。",
            "owner",
            320,
            7280,
        )]);
        let mut full = ConservativeSpeakerLedger::default();
        let first = full.project_full_capture(&old, &Default::default(), "");
        let next = full.project_full_capture(&corrected, &Default::default(), text(&first));
        assert_eq!(text(&next), corrected["text"]);
        assert!(next.revised_ledger.is_some());
        let next = full.project_full_capture(
            &corrected,
            &Default::default(),
            "旧合并路径拼入了重复片段。",
        );
        assert_eq!(text(&next), corrected["text"]);
        assert!(next.revised_ledger.is_some());
        let mut window = ConservativeSpeakerLedger::default();
        let first = window.project(&old, &Default::default(), "");
        assert!(
            window
                .project(&corrected, &Default::default(), text(&first))
                .revised_ledger
                .is_none(),
            "a window response has no authority over omitted earlier audio"
        );
        let mut unmappable = corrected.clone();
        unmappable["text"] = json!("没有这一帧的字词坐标。");
        let next = full.project_full_capture(&unmappable, &local(), text(&next));
        assert!(!next.coordinates_aligned);
        assert!(next.revised_ledger.is_none());
        assert_eq!(text(&next), unmappable["text"]);
    }

    #[test]
    fn a_full_pending_tail_preserves_prior_exclusion_and_all_unknown_owner_words() {
        let source = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("已确认旁人。", "video", 2000, 3000),
        ]);
        let mut ledger = ConservativeSpeakerLedger::default();
        let first = ledger.project_full_capture(&source, &local(), "");
        assert_eq!(text(&first), "本人。");
        let mut pending = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("已确认旁人。", "video", 2000, 3000),
            row("可能是轻声本人。", "pending", 4000, 5000),
        ]);
        pending["utterances"][2]["definite"] = json!(false);
        let next = ledger.project_full_capture(&pending, &local(), text(&first));
        assert_eq!(text(&next), "本人。可能是轻声本人。");
        assert_eq!(next.excluded_rows, 1);
        assert!(next.revised_ledger.is_some());
    }

    #[test]
    fn genuine_repetitions_in_full_capture_remain_separate_source_occurrences() {
        let source = packet(vec![
            row("再确认一次。", "owner", 0, 1000),
            row("再确认一次。", "owner", 2000, 3000),
        ]);
        let view = ConservativeSpeakerLedger::default().project_full_capture(
            &source,
            &Default::default(),
            "",
        );
        assert_eq!(text(&view), source["text"]);
        assert_eq!(view.result["utterances"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn late_contextual_owner_positive_revokes_the_old_word_exclusion_at_its_source_time() {
        let source = packet(vec![
            row("本人。", "owner", 0, 1000),
            row("不能误删。", "split", 2000, 3000),
        ]);
        let mut ledger = ConservativeSpeakerLedger::default();
        let mut evidence = local();
        let first = ledger.project(&source, &evidence, "");
        assert_eq!(first.excluded_rows, 1);
        evidence.note_presence(span(2400, 2600, EvidenceKind::Owner));
        let next = ledger.project(&source, &evidence, text(&first));
        assert_eq!(text(&next), source["text"]);
        assert_eq!(next.excluded_rows, 0);
        assert!(!evidence.owner_vetoes(4000, 5000));
    }
}
