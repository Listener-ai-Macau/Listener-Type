//! Preserve confirmed source ownership when two-pass ASR merges speaker rows.
//! A provider cluster labels a row; it cannot reassign earlier foreign PCM.

use super::*;
use crate::transcript_boundary::{consumed_prefix_end, content_key};

fn overlaps((start, end): (u64, u64), (other_start, other_end): (u64, u64)) -> bool {
    start < other_end && other_start < end
}

fn range(row: &Value) -> Option<(u64, u64)> {
    let (start, end) = utterance_start_ms(row).zip(utterance_end_ms(row))?;
    (start < end).then_some((start, end))
}

fn note_foreign_ranges(state: &mut SyncState, raw: &Value, filtered: &SpeakerFilteredResult) {
    if !state.local_speaker_tracking_enabled
        || !(filtered.stable_other_speaker_present || filtered.stable_non_target_utterance_present)
    {
        return;
    }
    let Some(target) = state.target_speaker_id.as_deref() else { return; };
    let owner_rows = filtered.result.get("utterances").and_then(Value::as_array);
    for row in raw.get("utterances").and_then(Value::as_array).into_iter().flatten() {
        if !utterance_is_stable(row) || utterance_speaker_id(row).is_none()
            || owner_rows.is_some_and(|rows| rows.contains(row))
        {
            continue;
        }
        let Some(audio_range) = range(row) else { continue; };
        let corroborated = state.local_speaker_evidence.iter()
            .chain(&state.local_transcript_foreign_evidence)
            .chain(&state.local_advisory_foreign_evidence)
            .any(|sample| {
                let center = sample.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
                center >= audio_range.0 && center <= audio_range.1
                    && matches!(sample.classification,
                        crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
            });
        let explicit_other = utterance_speaker_id(row).as_deref() != Some(target)
            || local_evidence_confirms_owner_absence(row, &provider_owner_evidence(state), None);
        // Overlapping speakers require separation; a foreign row's time span
        // alone cannot erase owner words spoken simultaneously in that span.
        let overlaps_owner = owner_rows.into_iter().flatten().filter_map(range)
            .any(|owned| overlaps(audio_range, owned));
        if explicit_other && corroborated && !overlaps_owner {
            state.provider_non_owner_audio_ranges.push(audio_range);
            if utterance_speaker_id(row).as_deref() != Some(target) {
                state.provider_distinct_foreign_audio_ranges.push(audio_range);
            }
        }
    }
    merge_ranges(&mut state.provider_non_owner_audio_ranges);
    merge_ranges(&mut state.provider_distinct_foreign_audio_ranges);
}

fn merge_ranges(ranges: &mut Vec<(u64, u64)>) {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for &(start, end) in ranges.iter() {
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    *ranges = merged;
}

pub(super) fn retained_distinct_foreign_track_applies(state: &SyncState, raw: &Value) -> bool {
    raw.get("utterances").and_then(Value::as_array).into_iter().flatten().filter_map(range)
        .any(|row_range| state.provider_distinct_foreign_audio_ranges.iter()
            .any(|&foreign| overlaps(row_range, foreign)))
}

/// Render only retained word occurrences. Prefer the provider's formatting;
/// when excluded text is rewritten (e.g. 10 -> 十), align the retained span
/// independently. If that span is ambiguous, use its timed ASR words, never
/// borrow an equal-looking occurrence from different audio.
fn render_word_span(row: &Value, words: &[Value], first: usize, last: usize) -> String {
    let source = row.get("text").and_then(Value::as_str).unwrap_or_default();
    let word_text = |word: &Value| word.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
    let keys: Vec<String> = words.iter().map(|word| content_key(&word_text(word))).collect();
    let kept_key = keys[first..last].concat();
    let source_key = content_key(source);
    let full_key = keys.concat();
    let key_start = if full_key == source_key {
        Some(keys[..first].iter().map(String::len).sum::<usize>())
    } else {
        let mut matches = source_key.match_indices(&kept_key);
        matches.next().and_then(|(offset, _)| matches.next().is_none().then_some(offset))
    };
    if let Some(start) = key_start {
        if let Some((from, through)) = consumed_prefix_end(source, &source_key[..start])
            .zip(consumed_prefix_end(source, &source_key[..start + kept_key.len()]))
        {
            return source[from..through].trim().to_string();
        }
    }
    let mut text: String = words[first..last].iter().map(word_text).collect();
    if last == words.len() {
        if let Some((offset, last_content)) = source.char_indices().rev()
            .find(|(_, ch)| !crate::transcript_boundary::is_decorative(*ch))
        {
            text.push_str(&source[offset + last_content.len_utf8()..]);
        }
    }
    text
}

pub(super) fn project_row(state: &SyncState, row: &Value) -> (Vec<Value>, bool) {
    let Some(audio_range) = range(row) else { return (vec![row.clone()], false); };
    if !state.provider_non_owner_audio_ranges.iter().any(|&foreign| overlaps(audio_range, foreign)) {
        return (vec![row.clone()], false);
    }
    let Some(words) = row.get("words").and_then(Value::as_array).filter(|words| !words.is_empty()) else {
        // Without word coordinates, reuse only prior owner segments in this
        // row's same PCM range. No new text gains owner authority here.
        let retained = state.live_owner_delivery.as_ref().into_iter()
            .flat_map(|candidate| &candidate.timed_segments)
            .filter(|segment| segment.start_ms >= audio_range.0 as i64
                && segment.end_ms.is_some_and(|end| end <= audio_range.1 as i64)
                && !state.provider_non_owner_audio_ranges.iter().any(|&foreign|
                    overlaps((segment.start_ms as u64, segment.end_ms.unwrap() as u64), foreign)))
            .map(|segment| {
                let mut owned = row.clone();
                owned["text"] = json!(segment.text);
                owned["start_time"] = json!(segment.start_ms);
                owned["end_time"] = json!(segment.end_ms);
                owned
            }).collect();
        return (retained, true);
    };
    let supported = |word: &Value| range(word).is_some_and(|word_range|
        !state.provider_non_owner_audio_ranges.iter().any(|&foreign| overlaps(word_range, foreign)));
    if words.iter().all(supported) { return (vec![row.clone()], false); }
    let mut projected = Vec::new();
    let mut first = 0;
    while first < words.len() {
        if !supported(&words[first]) { first += 1; continue; }
        let mut last = first + 1;
        while last < words.len() && supported(&words[last]) { last += 1; }
        let mut owned = row.clone();
        owned["text"] = json!(render_word_span(row, words, first, last));
        owned["words"] = json!(&words[first..last]);
        owned["start_time"] = json!(utterance_start_ms(&words[first]));
        owned["end_time"] = json!(utterance_end_ms(&words[last - 1]));
        projected.push(owned);
        first = last;
    }
    (projected, true)
}

pub(super) fn preserve_source_partition(state: &mut SyncState, raw: &Value, filtered: &mut SpeakerFilteredResult) {
    note_foreign_ranges(state, raw, filtered);
    let mut changed = false;
    let mut unobserved_source = false;
    for result in [&mut filtered.result, &mut filtered.optimistic_result] {
        let Some(rows) = result.get("utterances").and_then(Value::as_array) else { continue; };
        let mut projected = Vec::new();
        let mut result_changed = false;
        for row in rows {
            let (owned, clipped) = project_row(state, row);
            unobserved_source |= clipped && !row.get("words").and_then(Value::as_array)
                .is_some_and(|words| !words.is_empty() && words.iter().all(|word| range(word).is_some()));
            projected.extend(owned);
            result_changed |= clipped;
        }
        if result_changed {
            result["text"] = json!(projected.iter()
                .filter_map(|row| row.get("text").and_then(Value::as_str)).collect::<String>());
            result["utterances"] = json!(projected);
            changed = true;
        }
    }
    state.provider_source_exclusion_applied = changed;
    if changed {
        filtered.stable_other_speaker_present = true;
        filtered.stable_unresolved_speaker_present |= unobserved_source;
        filtered.target_speech_end_ms = filtered.result.get("utterances")
            .and_then(Value::as_array).into_iter().flatten()
            .filter(|row| utterance_is_stable(row)).filter_map(utterance_end_ms).max();
        if !filtered.pending_unattributed_text.is_empty() {
            filtered.pending_unattributed_text = filtered.optimistic_result.get("text")
                .and_then(Value::as_str).unwrap_or_default().to_string();
        }
        log::info!("[asr] preserved source speaker partition across provider row revision foreign_ranges={} owner_chars={}",
            state.provider_non_owner_audio_ranges.len(),
            filtered.result.get("text").and_then(Value::as_str).unwrap_or_default().chars().count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_words_remain_bound_to_their_original_audio_occurrences() {
        let state = SyncState {
            provider_non_owner_audio_ranges: vec![(100, 900), (1900, 2700)],
            ..SyncState::default()
        };
        let row = json!({"text":"相同的话。相同的话。相同的话。新的续句！",
            "start_time":100,"end_time":3600,"words":[
                {"text":"相同的话","start_time":100,"end_time":900},
                {"text":"相同的话","start_time":1000,"end_time":1800},
                {"text":"相同的话","start_time":1900,"end_time":2700},
                {"text":"新的续句","start_time":2800,"end_time":3600}
            ]});
        let (rows, changed) = project_row(&state, &row);
        assert!(changed);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["text"], "相同的话。");
        assert_eq!(rows[0]["start_time"], 1000);
        assert_eq!(rows[1]["text"], "新的续句！");
        assert_eq!(rows[1]["start_time"], 2800);
    }

    #[test]
    fn owner_final_revision_without_foreign_audio_keeps_new_words_and_punctuation() {
        let state = SyncState { provider_non_owner_audio_ranges: vec![(100, 900)], ..SyncState::default() };
        let row = json!({"text":"本人纠正后的十分钟，新的一句？","start_time":1000,"end_time":3600,
            "words":[{"text":"本人纠正后的10分钟新的一句","start_time":1000,"end_time":3600}]});
        let (rows, changed) = project_row(&state, &row);
        assert!(!changed);
        assert_eq!(rows, vec![row]);
    }

    #[test]
    fn ambiguous_rendering_uses_timed_owner_words_instead_of_a_foreign_text_match() {
        let state = SyncState { provider_non_owner_audio_ranges: vec![(100, 900)], ..SyncState::default() };
        let row = json!({"text":"相同的话十分钟相同的话！","start_time":100,"end_time":3600,
            "words":[
                {"text":"相同的话10分钟","start_time":100,"end_time":900},
                {"text":"相同的话","start_time":1000,"end_time":3600}
            ]});
        let (rows, changed) = project_row(&state, &row);
        assert!(changed);
        assert_eq!(rows[0]["text"], "相同的话！");
        assert_eq!(rows[0]["start_time"], 1000);
    }

    #[test]
    fn missing_word_coordinates_preserve_only_previously_owned_source_segments() {
        let mut state = SyncState {
            provider_non_owner_audio_ranges: vec![(100, 900)],
            provider_distinct_foreign_audio_ranges: vec![(100, 900)],
            live_owner_delivery: Some(transcript_candidate_from_result(&json!({"text":"本人正文。","utterances":[
                {"text":"本人正文。","start_time":1000,"end_time":2800}
            ]}))),
            ..SyncState::default()
        };
        let row = json!({"text":"视频对白本人正文未知的新尾句","start_time":100,"end_time":3600,
            "definite":true,"additions":{"speaker_id":"1"}});
        let (rows, changed) = project_row(&state, &row);
        assert!(changed);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["text"], "本人正文。");
        assert_eq!(rows[0]["start_time"], 1000);
        assert_eq!(rows[0]["end_time"], 2800);
        let raw = json!({"text":row["text"],"utterances":[row]});
        let mut target = Some("1".into());
        let mut filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &raw, &mut target, false, &[], None, None, false, false, None, None);
        preserve_source_partition(&mut state, &raw, &mut filtered);
        assert_eq!(filtered.result["text"], "本人正文。");
        assert!(filtered.stable_unresolved_speaker_present,
            "unmapped new tail stays pending instead of certifying a complete owner final");
    }

    #[test]
    fn pending_identity_and_overlapping_owner_audio_do_not_create_foreign_exclusions() {
        use crate::speaker_verification::SessionSpeakerClassification;
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            target_speaker_id: Some("1".into()),
            local_speaker_evidence: vec![LocalSpeakerEvidence {
                audio_end_ms: 1200, classification: SessionSpeakerClassification::Uncertain { score: 0.3 }, stable_target: true,
            }],
            ..SyncState::default()
        };
        let raw = json!({"text":"未观察的同一人","utterances":[
            {"text":"未观察的同一人","start_time":100,"end_time":1800,"definite":true,"additions":{"speaker_id":"1"}}
        ]});
        let mut target = state.target_speaker_id.clone();
        let pending = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &raw, &mut target, true, &[], None, None, false, false, None, None);
        note_foreign_ranges(&mut state, &raw, &pending);
        assert!(state.provider_non_owner_audio_ranges.is_empty());

        state.local_speaker_evidence[0].classification = SessionSpeakerClassification::NonTarget { score: 0.1 };
        let owner = json!({"text":"开始录音本人正文","start_time":1000,"end_time":4000,"definite":true,"additions":{"speaker_id":"1"}});
        let raw = json!({"text":"视频对白开始录音本人正文","utterances":[
            {"text":"视频对白","start_time":100,"end_time":1800,"definite":true,"additions":{"speaker_id":"2"}}, owner
        ]});
        let mut filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &raw, &mut target, true, &[], Some("开始录音"), None, true, false, None, None);
        filtered.result = json!({"text":"开始录音本人正文","utterances":[owner]});
        assert!(filtered.stable_other_speaker_present);
        note_foreign_ranges(&mut state, &raw, &filtered);
        assert!(state.provider_non_owner_audio_ranges.is_empty(), "simultaneous owner audio needs separation, not a time-only deletion");
    }

    #[test]
    fn source_partition_survives_transport_reset_and_clears_for_a_new_recording() {
        let state = SyncState {
            local_speaker_tracking_enabled: true,
            provider_non_owner_audio_ranges: vec![(100, 900)],
            provider_distinct_foreign_audio_ranges: vec![(100, 900)],
            ..SyncState::default()
        };
        let mut restored = SyncState::default();
        OwnerContinuitySnapshot::capture(&state).restore(&mut restored);
        assert_eq!(restored.provider_non_owner_audio_ranges, vec![(100, 900)]);
        assert_eq!(restored.provider_distinct_foreign_audio_ranges, vec![(100, 900)]);
        let asr = VolcengineStreamingASR::new(VolcengineCredentials {
            app_id:"app".into(), access_token:"token".into(), resource_id:VolcengineCredentials::default_resource_id().into(),
        }, Vec::new());
        *asr.state.lock() = restored;
        asr.note_local_speaker_tracking_started_with_owner("开始录音", true);
        assert!(asr.state.lock().provider_non_owner_audio_ranges.is_empty());
        assert!(asr.state.lock().provider_distinct_foreign_audio_ranges.is_empty());
    }
}
