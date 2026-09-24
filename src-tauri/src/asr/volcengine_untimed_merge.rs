//! Stateful merge for provider responses without utterance timestamps.
//!
//! Volcengine can move from a cumulative hypothesis to a rolling text window.
//! The first moved window must be appended by overlap; later revisions of that
//! same window must replace its suffix instead of being appended again.

use super::volcengine_transcript::{
    authoritative_owner_prefix_covers_ledger, char_edit_distance, merge_streaming_candidate,
    TranscriptCandidate, TranscriptSegment,
};

/// The optimistic speaker view contains the settled opening plus its current
/// provisional tail. Its cumulative growth supersedes the older view; passing
/// it to the rolling-window merger can append the same clause twice.
pub(super) fn merge_optimistic_cumulative_view(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    previous_untimed_window: &str,
    candidate: TranscriptCandidate,
    provider_coverage: Option<&TranscriptCandidate>,
) -> (String, Vec<TranscriptSegment>, String) {
    let previous = compact_segment_text(previous_text);
    let current = compact_segment_text(&candidate.text);
    if previous.chars().count() >= 8
        && current.chars().count() > previous.chars().count()
        && current.starts_with(&previous)
    {
        let old_start = previous_segments.iter().map(|segment| segment.start_ms).min();
        let new_start = candidate.timed_segments.iter().map(|segment| segment.start_ms).min();
        // Speaker filtering may omit the settled opening's timed row.
        let segments = if old_start.zip(new_start)
            .is_some_and(|(old, new)| new > old + 150)
        {
            previous_segments.to_vec()
        } else {
            candidate.timed_segments
        };
        let text = candidate.text;
        return (text.clone(), segments, text);
    }
    merge_filtered_streaming_candidate_with_untimed_window(
        previous_text,
        previous_segments,
        previous_untimed_window,
        candidate,
        provider_coverage,
    )
}

pub(super) fn merge_filtered_streaming_candidate_with_untimed_window(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    previous_untimed_window: &str,
    candidate: TranscriptCandidate,
    provider_coverage: Option<&TranscriptCandidate>,
) -> (String, Vec<TranscriptSegment>, String) {
    if provider_coverage.is_some_and(|provider| {
        authoritative_owner_prefix_covers_ledger(previous_segments, &candidate, provider)
    }) {
        log::info!("[asr] corrected owner prefix replaced covered streaming ledger previous_chars={} corrected_chars={}",
            previous_text.chars().count(), candidate.text.chars().count());
        return (candidate.text, candidate.timed_segments, String::new());
    }
    merge_streaming_candidate_with_untimed_window(
        previous_text,
        previous_segments,
        previous_untimed_window,
        candidate,
    )
}

pub(super) fn merge_streaming_candidate_with_untimed_window(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    previous_untimed_window: &str,
    candidate: TranscriptCandidate,
) -> (String, Vec<TranscriptSegment>, String) {
    let current_window = candidate.text.trim().to_string();
    // The first word-timed row may cover the same opening audio that an
    // untimed preview already put in the ledger. A sentence-ending mark on
    // that preview must not make the growing cumulative row look like a new
    // utterance. The 2026-09-24 device trace grew 18 -> 19 characters here
    // but the merge produced 37, repeating the entire opening sentence.
    if previous_segments.is_empty()
        && authoritative_timed_revision_starts_near_session_start(previous_text, &candidate)
        && !previous_text.trim().is_empty()
    {
        let previous_spoken = compact_segment_text(previous_text);
        let current_spoken = compact_segment_text(&current_window);
        if current_spoken.len() > previous_spoken.len()
            && current_spoken.starts_with(&previous_spoken)
        {
            return (current_window, candidate.timed_segments, String::new());
        }
    }
    // A provisional untimed tail can reach the ledger before diarization
    // publishes the same words as a timed, session-wide row. At that handoff
    // the text may be *equal* to the ledger, not longer. Appending the timed
    // row then duplicates the entire new clause (live session b141e32a,
    // frames 14-16). Session-start timing plus complete spoken-prefix
    // coverage identifies a cumulative revision; a genuinely later repeated
    // utterance has a later start and continues through the append path.
    if authoritative_timed_revision_starts_near_session_start(previous_text, &candidate)
        && !previous_text.trim().is_empty()
    {
        let previous_spoken = compact_segment_text(previous_text);
        let current_spoken = compact_segment_text(&current_window);
        if current_spoken.starts_with(&previous_spoken) {
            return (current_window, candidate.timed_segments, String::new());
        }
    }
    // Once timed rows exist, the optimized provider can still send a full
    // cumulative growth row. In session dc9deee9 frame 33 the provider grew
    // the existing opening with "然后呢？做完以后", but the rolling-window
    // merge appended that ending twice and pause-early paste made it visible.
    // A complete spoken prefix proves this row already covers the ledger.
    if candidate.authoritative_cumulative && !previous_text.trim().is_empty() {
        let previous_spoken = compact_segment_text(previous_text);
        let current_spoken = compact_segment_text(&current_window);
        if previous_spoken.chars().count() >= 8
            && current_spoken.chars().count() > previous_spoken.chars().count()
            && current_spoken.starts_with(&previous_spoken)
        {
            let segments = if candidate.timed_segments.is_empty() {
                previous_segments.to_vec()
            } else {
                candidate.timed_segments
            };
            return (current_window, segments, String::new());
        }
    }
    // A two-pass cumulative row can correct one word in the old prefix while
    // growing it. Exact prefix matching then fails and the rolling-window path
    // appends the already-covered new clause a second time. Compare the old
    // ledger against the same-length head of this session-start revision;
    // timing establishes identity, while a small edit distance permits the
    // provider's spelling correction without hiding a real later utterance.
    if candidate.authoritative_cumulative
        && authoritative_timed_revision_starts_near_session_start(previous_text, &candidate)
        && candidate_covers_growing_ledger_with_small_correction(previous_text, &current_window)
    {
        return (current_window, candidate.timed_segments, String::new());
    }
    // Optimized-bidirectional streaming can spend most of a long utterance in
    // untimed rolling windows, then publish its first word-timed two-pass
    // result for the whole session. That response replaces the provisional
    // ledger. Appending it would repeat nearly the entire dictation (installed
    // session dedf4aa7-c104-4733-a16f-a2044d929645). Require an authoritative
    // correction that starts near the beginning and closely covers the old
    // wording; a later timed utterance still follows the ordinary append path.
    if previous_segments.is_empty()
        && candidate.authoritative_cumulative
        && authoritative_timed_revision_starts_near_session_start(previous_text, &candidate)
    {
        if authoritative_timed_revision_covers_untimed_ledger(previous_text, &candidate) {
            return (current_window, candidate.timed_segments, String::new());
        }

        // A disjoint early timed utterance is still a legitimate new sentence;
        // only an overlapping candidate is held for a later full correction.
        // This avoids treating the wake row's early timestamp as proof that a
        // new utterance is a session-wide replacement.
        if authoritative_timed_candidate_overlaps_old_ledger(previous_text, &candidate) {
            log::warn!(
                "[asr] rejected incomplete first timed full revision previous_chars={} candidate_chars={}",
                previous_text.chars().count(),
                current_window.chars().count()
            );
            return (
                previous_text.trim().to_string(),
                previous_segments.to_vec(),
                previous_untimed_window.trim().to_string(),
            );
        }
    }
    // A completed opening sentence can be re-delivered after the next
    // clause's preview already streamed into the ledger. Installed session
    // 4c3b4b52 (2026-09-19): the punctuated final "开始录音。…通用的话。"
    // landed while "就帮" was already appended, the wake row's timed segment
    // did not match the active window, and the retain-both branch appended
    // the whole opening a second time — the user saw one spoken sentence
    // delivered twice. A candidate whose timed coverage starts at the
    // session opening and whose wording merely restates the ledger's own
    // beginning is a late re-publication of already-covered audio: keep the
    // ledger unchanged. Genuinely later re-statements carry later timestamps
    // and still take the retain-both append path below.
    if authoritative_timed_revision_starts_near_session_start(previous_text, &candidate)
        && candidate_restates_ledger_opening(previous_text, &current_window)
    {
        log::info!(
            "[asr] dropped late re-publication of already-ledgered opening previous_chars={} candidate_chars={}",
            previous_text.chars().count(),
            current_window.chars().count()
        );
        return (
            previous_text.trim().to_string(),
            previous_segments.to_vec(),
            previous_untimed_window.trim().to_string(),
        );
    }
    // The optimized stream may reuse a completed prefix (often the wake
    // phrase) while advancing to the next spoken clause without timestamps.
    // Those windows are sequential body text, not revisions of the same body
    // window. Preserve the accumulated ledger and append only the new suffix;
    // same-window growth/rewrite still follows the replacement logic below.
    // Early accelerated text has no word timestamps. The first word-timed
    // response can revise punctuation in the same rolling window. Carry that
    // window's identity through the transition instead of appending it again
    // to the untimed prefix. Existing timed segments still own later merges.
    let replacement = (candidate.timed_segments.is_empty() || previous_segments.is_empty())
        .then(|| {
            replace_revised_untimed_suffix(previous_text, previous_untimed_window, &current_window)
        })
        .flatten();
    let anchored_replacement =
        replace_anchored_active_window(previous_text, previous_untimed_window, &current_window);
    let active_window_replacement = anchored_replacement.or(replacement.clone());

    let previous_spoken = compact_segment_text(previous_untimed_window);
    let current_spoken = compact_segment_text(&current_window);
    let cumulative_completed_window_shape = previous_window_ends_sentence(previous_untimed_window)
        && current_spoken.chars().count() > previous_spoken.chars().count()
        && current_spoken.starts_with(&previous_spoken)
        && spoken_prefix_boundary(&current_window, &previous_spoken).is_some();
    let timed_window_overlap = timed_candidate_overlaps_active_window(
        previous_segments,
        previous_untimed_window,
        &candidate,
    );

    // If timing proves that this is a later, separate utterance, retain both
    // occurrences even when the provider repeats the earlier wording in its
    // cumulative response. Generic overlap merging would otherwise collapse
    // the second utterance back into the first one.
    let repeated_completed_window_shape = previous_window_ends_sentence(previous_untimed_window)
        && (same_spoken_content(previous_untimed_window, &current_window)
            || cumulative_completed_window_shape);
    if !candidate.timed_segments.is_empty()
        && !timed_window_overlap
        && repeated_completed_window_shape
    {
        let mut segments = previous_segments.to_vec();
        segments.extend(candidate.timed_segments.iter().cloned());
        segments.sort_by_key(|segment| segment.start_ms);
        return (
            format!("{}{}", previous_text.trim_end(), current_window),
            segments,
            candidate
                .authoritative_cumulative
                .then_some(current_window.clone())
                .unwrap_or_default(),
        );
    }

    // When the new cumulative window retains the old completed spoken
    // content and carries a punctuation boundary into a new clause, replace
    // the old window as a whole. This covers both an unchanged `。` and a
    // provider correction such as `。` -> `，`; appending after the wake
    // boundary would replay the completed body.
    if cumulative_completed_window_shape && timed_window_overlap {
        if let Some(stable_prefix) = previous_text
            .trim_end()
            .strip_suffix(previous_untimed_window.trim())
        {
            let replacement_text = format!("{stable_prefix}{current_window}");
            if candidate.timed_segments.is_empty() {
                return (replacement_text, previous_segments.to_vec(), current_window);
            }
            let suffix = spoken_prefix_boundary(&current_window, &previous_spoken)
                .map(|(boundary, _)| current_window.chars().skip(boundary).collect::<String>())
                .unwrap_or_default();
            let segments = merge_timed_continuation_segments(
                previous_segments,
                &candidate.timed_segments,
                &suffix,
            );
            let next_window = candidate
                .authoritative_cumulative
                .then_some(current_window.clone())
                .unwrap_or_default();
            return (replacement_text, segments, next_window);
        }
    }

    // When the new cumulative window retains the old completed spoken
    // content and carries a punctuation boundary into a new clause, replace
    // the old window as a whole. This covers both an unchanged `。` and a
    // provider correction such as `。` -> `，`; appending after the wake
    // boundary would replay the completed body.
    if cumulative_completed_window_shape {
        if current_spoken.chars().count() > previous_spoken.chars().count()
            && current_spoken.starts_with(&previous_spoken)
            && spoken_prefix_boundary(&current_window, &previous_spoken).is_some()
        {
            if let Some(stable_prefix) = previous_text
                .trim_end()
                .strip_suffix(previous_untimed_window.trim())
            {
                let replacement_text = format!("{stable_prefix}{current_window}");
                if candidate.timed_segments.is_empty() {
                    return (replacement_text, previous_segments.to_vec(), current_window);
                }
                let suffix = spoken_prefix_boundary(&current_window, &previous_spoken)
                    .map(|(boundary, _)| current_window.chars().skip(boundary).collect::<String>())
                    .unwrap_or_default();
                let segments = merge_timed_continuation_segments(
                    previous_segments,
                    &candidate.timed_segments,
                    &suffix,
                );
                let next_window = candidate
                    .authoritative_cumulative
                    .then_some(current_window.clone())
                    .unwrap_or_default();
                return (replacement_text, segments, next_window);
            }
        }
    }

    // A growing revision of the current rolling window may share a completed
    // sentence prefix with the previous window.  It must replace that active
    // window before the sequential-window path is considered; otherwise a
    // sentence boundary makes the same window look like a new clause and the
    // old window is appended a second time.  This is the exact shape seen in
    // the 2026-09-14 physical trace ("...今晚" -> "...今晚吃").
    let active_window_is_growing = !previous_untimed_window.trim().is_empty()
        && current_window
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .count()
            > previous_untimed_window
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .count();
    if active_window_is_growing && candidate.timed_segments.is_empty() {
        if let Some(replacement_text) = replacement.clone() {
            return (replacement_text, previous_segments.to_vec(), current_window);
        }
    }

    // A provider may revise only the terminal punctuation of the active
    // window. It shares a completed sentence prefix with the old window, but
    // it is not a new sequential clause; sending it through the continuation
    // path would append the entire sentence a second time.
    if previous_window_ends_sentence(previous_untimed_window)
        && same_spoken_content(previous_untimed_window, &current_window)
        && (candidate.timed_segments.is_empty() || timed_window_overlap)
    {
        if let Some(replacement_text) = active_window_replacement {
            if candidate.timed_segments.is_empty() {
                return (replacement_text, previous_segments.to_vec(), current_window);
            }
            if let Some((previous_body, current_body)) =
                anchored_active_window_bodies(previous_untimed_window, &current_window)
            {
                let segments = replace_timed_active_window_segments(
                    previous_segments,
                    &candidate.timed_segments,
                    &previous_body,
                    &current_body,
                );
                let next_window = candidate
                    .authoritative_cumulative
                    .then_some(current_window.clone())
                    .unwrap_or_default();
                return (replacement_text, segments, next_window);
            }
            return (
                replacement_text,
                candidate.timed_segments,
                candidate
                    .authoritative_cumulative
                    .then_some(current_window.clone())
                    .unwrap_or_default(),
            );
        }
    }

    let terminal_prefix_continuation = append_after_shared_terminal_prefix(
        previous_text,
        previous_untimed_window,
        &current_window,
    );
    if let Some(text) = terminal_prefix_continuation {
        // A timed response can still be a rolling next-clause window. Treat
        // that response as a revision of the active window for segmentation:
        // an authoritative flag on the provider row must not replace the
        // earlier timed clause when the text itself proves a shared terminal
        // prefix. Keep the merged timing history and retain the window marker
        // so another sequential clause can be accumulated.
        let suffix = shared_terminal_prefix_suffix(previous_untimed_window, &current_window)
            .expect("terminal continuation must have a non-empty suffix");
        let segments = merge_timed_continuation_segments(
            previous_segments,
            &candidate.timed_segments,
            &suffix,
        );
        return (text, segments, current_window);
    }
    if !candidate.timed_segments.is_empty() {
        let authoritative_cumulative = candidate.authoritative_cumulative;
        if let (Some((previous_body, current_body)), Some(replacement_text)) = (
            anchored_active_window_bodies(previous_untimed_window, &current_window),
            active_window_replacement.clone(),
        ) {
            let segments = replace_timed_active_window_segments(
                previous_segments,
                &candidate.timed_segments,
                &previous_body,
                &current_body,
            );
            let next_window = if authoritative_cumulative {
                current_window.clone()
            } else {
                String::new()
            };
            return (replacement_text, segments, next_window);
        }
        let (text, segments) =
            merge_streaming_candidate(previous_text, previous_segments, candidate);
        let next_window = if authoritative_cumulative {
            current_window
        } else {
            String::new()
        };
        return (
            active_window_replacement.unwrap_or(text),
            segments,
            next_window,
        );
    }

    let (text, segments) = active_window_replacement.map_or_else(
        || merge_streaming_candidate(previous_text, previous_segments, candidate),
        |text| (text, previous_segments.to_vec()),
    );
    (text, segments, current_window)
}

fn append_after_shared_terminal_prefix(
    previous_text: &str,
    previous_window: &str,
    current_window: &str,
) -> Option<String> {
    let suffix = shared_terminal_prefix_suffix(previous_window, current_window)?;
    let previous_last = previous_text.trim_end().chars().next_back();
    let suffix_first = suffix.chars().next();
    let separator = if previous_last.is_some_and(is_clause_separator)
        || suffix_first.is_some_and(is_clause_separator)
    {
        ""
    } else {
        // Untimed rolling windows often omit the pause punctuation at their
        // boundary. Match the adjacent text so an English revision does not
        // acquire Chinese punctuation.
        match (previous_last, suffix_first) {
            (Some(left), Some(right)) if is_cjk(left) || is_cjk(right) => "，",
            _ => " ",
        }
    };
    Some(format!(
        "{}{}{}",
        previous_text.trim_end(),
        separator,
        suffix
    ))
}

fn shared_terminal_prefix_suffix(previous_window: &str, current_window: &str) -> Option<String> {
    let previous_window = previous_window.trim();
    let current_window = current_window.trim();
    if previous_window.is_empty() || current_window.is_empty() {
        return None;
    }
    let common_len = previous_window
        .chars()
        .zip(current_window.chars())
        .take_while(|(left, right)| left == right)
        .count();
    let common_prefix = previous_window.chars().take(common_len).collect::<Vec<_>>();
    let anchor_end = if previous_window_ends_sentence(previous_window) {
        let previous_spoken = compact_segment_text(previous_window);
        let current_spoken = compact_segment_text(current_window);
        if current_spoken.chars().count() > previous_spoken.chars().count()
            && current_spoken.starts_with(&previous_spoken)
        {
            // Recognition may replace the old sentence-ending punctuation
            // with a comma when it continues speaking. Raw prefix matching
            // then stops one character early; align by spoken content and
            // consume the new punctuation before taking the actual suffix.
            if let Some((anchor_end, separator)) =
                spoken_prefix_boundary(current_window, &previous_spoken)
            {
                let suffix = current_window.chars().skip(anchor_end).collect::<String>();
                let punctuation = separator
                    .chars()
                    .filter(|ch| !ch.is_whitespace())
                    .collect::<String>();
                let previous_terminal = previous_window.trim_end().chars().next_back();
                let prefix = punctuation
                    .chars()
                    .next()
                    .filter(|punctuation| Some(*punctuation) != previous_terminal)
                    .map(|_| punctuation)
                    .unwrap_or_default();
                return (!suffix.trim().is_empty()).then(|| format!("{prefix}{suffix}"));
            }
        }
        if common_len == previous_window.chars().count() {
            // The old window is a complete prefix of the new cumulative
            // window. Continue after that whole window; anchoring at its last
            // punctuation would replay the completed body.
            common_len
        } else {
            common_prefix
                .iter()
                .rposition(|ch| is_sentence_terminal(*ch))
                .map(|index| index + 1)?
        }
    } else {
        let terminal_index = common_prefix
            .iter()
            .rposition(|ch| is_sentence_terminal(*ch))?;
        if !common_prefix[terminal_index + 1..]
            .iter()
            .all(|ch| ch.is_whitespace())
        {
            return None;
        }
        common_len
    };
    let suffix = current_window.chars().skip(anchor_end).collect::<String>();
    (!suffix.trim().is_empty()).then(|| suffix.trim().to_string())
}

fn anchored_active_window_bodies(
    previous_window: &str,
    current_window: &str,
) -> Option<(String, String)> {
    let previous_window = previous_window.trim();
    let current_window = current_window.trim();
    if previous_window.is_empty() || current_window.is_empty() {
        return None;
    }
    let common_len = previous_window
        .chars()
        .zip(current_window.chars())
        .take_while(|(left, right)| left == right)
        .count();
    let common_prefix = previous_window.chars().take(common_len).collect::<Vec<_>>();
    let anchor_end = common_prefix
        .iter()
        .rposition(|ch| is_sentence_terminal(*ch))
        .map(|index| index + 1)?;
    let previous_body = previous_window
        .chars()
        .skip(anchor_end)
        .collect::<String>()
        .trim()
        .to_string();
    let current_body = current_window
        .chars()
        .skip(anchor_end)
        .collect::<String>()
        .trim()
        .to_string();
    if previous_body.is_empty() || current_body.is_empty() || previous_body == current_body {
        return None;
    }
    Some((previous_body, current_body))
}

fn replace_anchored_active_window(
    previous_text: &str,
    previous_window: &str,
    current_window: &str,
) -> Option<String> {
    let (previous_body, current_body) =
        anchored_active_window_bodies(previous_window, current_window)?;
    let previous_text = previous_text.trim_end();
    let stable_prefix = previous_text.strip_suffix(&previous_body)?;
    Some(format!("{stable_prefix}{current_body}"))
}

fn is_sentence_terminal(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '.' | '!' | '?' | '…')
}

fn is_clause_separator(ch: char) -> bool {
    is_sentence_terminal(ch) || matches!(ch, ',' | '，' | '、' | ';' | '；' | ':' | '：')
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

fn merge_timed_continuation_segments(
    previous_segments: &[TranscriptSegment],
    incoming_segments: &[TranscriptSegment],
    suffix: &str,
) -> Vec<TranscriptSegment> {
    let mut merged = previous_segments.to_vec();
    let previous_end = previous_segments
        .iter()
        .filter_map(|segment| segment.end_ms)
        .max();
    let suffix_compact = compact_segment_text(suffix);
    for segment in incoming_segments {
        if merged.iter().any(|existing| existing == segment) {
            continue;
        }
        // The real provider keeps the body row's start at the end of the wake
        // row while extending its end. Update that active row with the newly
        // recognized clause; do not append each rolling replacement as a
        // duplicate segment.
        if let Some(index) = merged.iter().position(|existing| {
            existing.start_ms == segment.start_ms
                && !suffix_compact.is_empty()
                && compact_segment_text(&existing.text) != suffix_compact
        }) {
            if compact_segment_text(&segment.text).contains(&suffix_compact) {
                let existing = &mut merged[index];
                let separator = if existing
                    .text
                    .trim_end()
                    .chars()
                    .next_back()
                    .is_some_and(is_clause_separator)
                    || suffix.chars().next().is_some_and(is_clause_separator)
                {
                    ""
                } else if existing.text.chars().chain(suffix.chars()).any(is_cjk) {
                    "，"
                } else {
                    " "
                };
                existing.text = format!("{}{}{}", existing.text.trim_end(), separator, suffix);
                existing.end_ms = existing.end_ms.max(segment.end_ms);
            }
            continue;
        }
        if previous_end.is_some_and(|end| segment.start_ms < end) {
            continue;
        }
        merged.push(segment.clone());
    }
    merged.sort_by_key(|segment| segment.start_ms);
    merged
}

fn compact_segment_text(text: &str) -> String {
    text.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

fn candidate_covers_growing_ledger_with_small_correction(
    previous_text: &str,
    candidate_text: &str,
) -> bool {
    const MAX_PREFIX_EDIT_PERCENT: usize = 10;
    let previous = compact_segment_text(previous_text);
    let current = compact_segment_text(candidate_text);
    let previous_len = previous.chars().count();
    if previous_len < 12 || current.chars().count() <= previous_len {
        return false;
    }
    let current_head = current.chars().take(previous_len).collect::<String>();
    let edits = char_edit_distance(&previous, &current_head);
    edits <= 2 || edits.saturating_mul(100) <= previous_len * MAX_PREFIX_EDIT_PERCENT
}

fn same_spoken_content(previous: &str, current: &str) -> bool {
    let previous = compact_segment_text(previous);
    let current = compact_segment_text(current);
    !previous.is_empty() && previous == current
}

fn timed_candidate_overlaps_active_window(
    previous_segments: &[TranscriptSegment],
    previous_window: &str,
    candidate: &TranscriptCandidate,
) -> bool {
    const SESSION_START_TOLERANCE_MS: i64 = 1_500;

    let Some(candidate_start) = candidate
        .timed_segments
        .iter()
        .map(|segment| segment.start_ms)
        .min()
    else {
        return true;
    };
    if previous_segments.is_empty() {
        return candidate_start <= SESSION_START_TOLERANCE_MS;
    }

    let active_content = active_window_spoken_content(previous_window);
    let active_segments = previous_segments
        .iter()
        .filter(|segment| timed_segment_matches_active_content(&segment.text, &active_content));
    active_segments.clone().any(|previous| {
        let Some(previous_end) = previous.end_ms else {
            return false;
        };
        candidate.timed_segments.iter().any(|incoming| {
            let Some(incoming_end) = incoming.end_ms else {
                return false;
            };
            timed_segment_matches_active_content(&incoming.text, &active_content)
                && incoming.start_ms < previous_end
                && previous.start_ms < incoming_end
        })
    })
}

fn active_window_spoken_content(window: &str) -> String {
    let mut after_first_sentence = false;
    let mut body = String::new();
    for ch in window.chars() {
        if after_first_sentence && ch.is_alphanumeric() {
            body.push(ch);
        }
        if is_sentence_terminal(ch) && !after_first_sentence {
            after_first_sentence = true;
        }
    }
    if body.is_empty() {
        compact_segment_text(window)
    } else {
        body
    }
}

fn timed_segment_matches_active_content(segment_text: &str, active_content: &str) -> bool {
    let segment = compact_segment_text(segment_text);
    !segment.is_empty()
        && !active_content.is_empty()
        && (active_content.contains(&segment) || segment.contains(active_content))
}

fn authoritative_timed_candidate_overlaps_old_ledger(
    previous_text: &str,
    candidate: &TranscriptCandidate,
) -> bool {
    const MIN_OVERLAP_PERCENT: usize = 45;
    let previous = alphanumeric_chars(previous_text);
    let current = alphanumeric_chars(&candidate.text);
    if previous.len() < 12 || current.is_empty() {
        return false;
    }
    longest_common_subsequence_len(&previous, &current).saturating_mul(100)
        >= previous.len() * MIN_OVERLAP_PERCENT
        && full_revision_edges_are_covered(&previous, &current)
}

fn spoken_prefix_boundary(text: &str, spoken_prefix: &str) -> Option<(usize, String)> {
    let expected = spoken_prefix.chars().collect::<Vec<_>>();
    if expected.is_empty() {
        return None;
    }
    let mut matched = 0;
    let mut boundary = 0;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if expected.get(matched) != Some(&ch) {
                return None;
            }
            matched += 1;
        }
        boundary += 1;
        if matched == expected.len() {
            break;
        }
    }
    if matched != expected.len() {
        return None;
    }
    let separator_start = boundary;
    for ch in text.chars().skip(boundary) {
        if ch.is_alphanumeric() {
            break;
        }
        boundary += 1;
    }
    let separator: String = text
        .chars()
        .skip(separator_start)
        .take(boundary - separator_start)
        .collect();
    separator
        .chars()
        .any(|ch| !ch.is_alphanumeric() && !ch.is_whitespace())
        .then_some((boundary, separator))
}

fn replace_timed_active_window_segments(
    previous_segments: &[TranscriptSegment],
    incoming_segments: &[TranscriptSegment],
    previous_body: &str,
    current_body: &str,
) -> Vec<TranscriptSegment> {
    let mut merged = previous_segments.to_vec();
    let previous_end = previous_segments
        .iter()
        .filter_map(|segment| segment.end_ms)
        .max();
    let current_body_compact = compact_segment_text(current_body);
    for segment in incoming_segments {
        if let Some(index) = merged.iter().position(|existing| {
            existing.start_ms == segment.start_ms
                && compact_segment_text(&segment.text).contains(&current_body_compact)
        }) {
            let existing = &mut merged[index];
            let stable = existing
                .text
                .trim_end()
                .strip_suffix(previous_body)
                .unwrap_or(existing.text.trim_end());
            existing.text = format!("{stable}{current_body}");
            existing.end_ms = existing.end_ms.max(segment.end_ms);
            continue;
        }
        if merged.iter().any(|existing| existing == segment)
            || previous_end.is_some_and(|end| segment.start_ms < end)
        {
            continue;
        }
        merged.push(segment.clone());
    }
    merged.sort_by_key(|segment| segment.start_ms);
    merged
}

fn authoritative_timed_revision_covers_untimed_ledger(
    previous_text: &str,
    candidate: &TranscriptCandidate,
) -> bool {
    const MIN_CONTENT_CHARS: usize = 12;

    if !authoritative_timed_revision_starts_near_session_start(previous_text, candidate) {
        return false;
    }

    let previous = alphanumeric_chars(previous_text);
    let current = alphanumeric_chars(&candidate.text);
    if previous.len() < MIN_CONTENT_CHARS || current.is_empty() {
        return false;
    }

    let coverage = longest_common_subsequence_len(&previous, &current);
    if coverage.saturating_mul(100) < previous.len().saturating_mul(90) {
        return false;
    }
    if sentence_boundary_count(&candidate.text) < sentence_boundary_count(previous_text) {
        return false;
    }

    // A high whole-text LCS can still be satisfied by a prefix-only result.
    // Require both edges of the old ledger to remain represented near the
    // corresponding edge of the new result, allowing small spelling or
    // punctuation corrections at either boundary.
    full_revision_edges_are_covered(&previous, &current)
        && full_revision_sentences_are_covered(previous_text, &candidate.text)
}

fn authoritative_timed_revision_starts_near_session_start(
    previous_text: &str,
    candidate: &TranscriptCandidate,
) -> bool {
    const SESSION_START_TOLERANCE_MS: i64 = 1_500;
    const MIN_CONTENT_CHARS: usize = 12;

    let Some(first_start_ms) = candidate
        .timed_segments
        .iter()
        .map(|segment| segment.start_ms)
        .min()
    else {
        return false;
    };
    first_start_ms <= SESSION_START_TOLERANCE_MS
        && alphanumeric_chars(previous_text).len() >= MIN_CONTENT_CHARS
        && !alphanumeric_chars(&candidate.text).is_empty()
}

/// The candidate's spoken wording merely restates the beginning of the
/// already-accumulated ledger (within a small correction tolerance, e.g. a
/// two-pass "怎么用" fixing a streaming "怎么弄"). That alignment against the
/// ledger's own head — rather than against its tail — is the signature of a
/// provider re-publishing completed opening audio, not a new utterance.
fn candidate_restates_ledger_opening(previous_text: &str, candidate_text: &str) -> bool {
    const MIN_OPENING_CHARS: usize = 8;
    const MAX_OPENING_PREFIX_CER: f64 = 0.15;

    let previous = alphanumeric_chars(previous_text);
    let current = alphanumeric_chars(candidate_text);
    if current.len() < MIN_OPENING_CHARS || current.len() > previous.len() {
        return false;
    }
    let aligned_head: String = previous[..current.len()].iter().collect();
    let candidate_compact: String = current.iter().collect();
    let distance = char_edit_distance(&aligned_head, &candidate_compact);
    (distance as f64 / current.len() as f64) <= MAX_OPENING_PREFIX_CER
}

fn alphanumeric_chars(text: &str) -> Vec<char> {
    text.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

fn longest_common_subsequence_len(left: &[char], right: &[char]) -> usize {
    let mut row = vec![0usize; right.len() + 1];
    for left_char in left {
        let mut diagonal = 0usize;
        for (index, right_char) in right.iter().enumerate() {
            let prior = row[index + 1];
            row[index + 1] = if left_char == right_char {
                diagonal + 1
            } else {
                row[index + 1].max(row[index])
            };
            diagonal = prior;
        }
    }
    row[right.len()]
}

fn sentence_boundary_count(text: &str) -> usize {
    text.chars()
        .filter(|ch| matches!(ch, '.' | '。' | '?' | '？' | '!' | '！' | '…'))
        .count()
}

fn full_revision_edges_are_covered(previous: &[char], current: &[char]) -> bool {
    const PREFIX_ANCHOR_CHARS: usize = 5;
    const SUFFIX_ANCHOR_CHARS: usize = 8;
    const EDGE_CONTEXT_CHARS: usize = 4;
    const MIN_EDGE_COVERAGE_PERCENT: usize = 80;

    let prefix_len = previous.len().min(PREFIX_ANCHOR_CHARS);
    let suffix_len = previous.len().min(SUFFIX_ANCHOR_CHARS);
    let current_prefix_len = current.len().min(prefix_len + EDGE_CONTEXT_CHARS);
    let current_suffix_start = current
        .len()
        .saturating_sub(suffix_len + EDGE_CONTEXT_CHARS);
    let prefix_coverage =
        longest_common_subsequence_len(&previous[..prefix_len], &current[..current_prefix_len]);
    let suffix_coverage = longest_common_subsequence_len(
        &previous[previous.len() - suffix_len..],
        &current[current_suffix_start..],
    );

    prefix_coverage.saturating_mul(100) >= prefix_len * MIN_EDGE_COVERAGE_PERCENT
        && suffix_coverage.saturating_mul(100) >= suffix_len * MIN_EDGE_COVERAGE_PERCENT
}

fn full_revision_sentences_are_covered(previous_text: &str, current_text: &str) -> bool {
    const MIN_SENTENCE_CHARS: usize = 2;
    const MIN_SENTENCE_COVERAGE_PERCENT: usize = 85;

    // Candidate punctuation is not a stable alignment boundary: the
    // provider may add, remove, or move sentence terminals while correcting
    // the same words. Align the complete old/new word streams once, then
    // count the matched old characters inside each *old* sentence range. This
    // keeps sentence ownership disjoint without letting an arbitrary
    // candidate prefix borrow the tail of the next sentence.
    let previous = alphanumeric_chars(previous_text);
    let current = alphanumeric_chars(current_text);
    let previous_ranges = spoken_sentence_char_ranges(previous_text);
    if previous_ranges.is_empty() || current.is_empty() {
        return false;
    }
    // With no old sentence boundary there is no sentence ownership ambiguity:
    // the global coverage/edge checks above already decide whether this is a
    // complete correction, regardless of how many terminals the candidate
    // added. For multiple old sentences, inspect every optimal alignment so
    // a single lucky backtracking path cannot hide a deleted short sentence.
    if previous_ranges.len() == 1 {
        return true;
    }
    for (start, end) in previous_ranges {
        let sentence_len = end - start;
        if sentence_len < MIN_SENTENCE_CHARS {
            continue;
        }
        let required_coverage = (sentence_len * MIN_SENTENCE_COVERAGE_PERCENT).div_ceil(100);
        let minimum_matches = minimum_lcs_matches_in_left_range(&previous, &current, start, end);
        if minimum_matches < required_coverage {
            return false;
        }
    }
    true
}

fn minimum_lcs_matches_in_left_range(
    left: &[char],
    right: &[char],
    target_start: usize,
    target_end: usize,
) -> usize {
    let width = right.len() + 1;
    // Each cell stores (maximum LCS length, minimum matches in the target
    // sentence among paths achieving that maximum). Considering both skip
    // branches even when the characters are equal is what exposes ambiguous
    // alignments that a deterministic backtrack would conceal.
    let mut table = vec![(0usize, 0usize); (left.len() + 1) * width];
    for left_index in (0..left.len()).rev() {
        for right_index in (0..right.len()).rev() {
            let index = left_index * width + right_index;
            let mut best = table[(left_index + 1) * width + right_index];
            best = prefer_longest_then_minimum_matches(
                best,
                table[left_index * width + right_index + 1],
            );
            if left[left_index] == right[right_index] {
                let diagonal = table[(left_index + 1) * width + right_index + 1];
                best = prefer_longest_then_minimum_matches(
                    best,
                    (
                        diagonal.0 + 1,
                        diagonal.1
                            + usize::from(left_index >= target_start && left_index < target_end),
                    ),
                );
            }
            table[index] = best;
        }
    }
    table[0].1
}

fn prefer_longest_then_minimum_matches(
    current: (usize, usize),
    candidate: (usize, usize),
) -> (usize, usize) {
    if candidate.0 > current.0 || (candidate.0 == current.0 && candidate.1 < current.1) {
        candidate
    } else {
        current
    }
}

fn spoken_sentence_char_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut length = 0usize;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            length += 1;
        }
        if is_sentence_terminal(ch) && length > start {
            ranges.push((start, length));
            start = length;
        }
    }
    if length > start {
        ranges.push((start, length));
    }
    ranges
}

fn spoken_sentence_chunks(text: &str) -> Vec<Vec<char>> {
    let mut sentences = Vec::new();
    let mut sentence = Vec::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            sentence.push(ch);
        }
        if is_sentence_terminal(ch) && !sentence.is_empty() {
            sentences.push(std::mem::take(&mut sentence));
        }
    }
    if !sentence.is_empty() {
        sentences.push(sentence);
    }
    sentences
}

fn replace_revised_untimed_suffix(
    previous_text: &str,
    previous_window: &str,
    current_window: &str,
) -> Option<String> {
    let previous_text = previous_text.trim();
    let previous_window = previous_window.trim();
    let current_window = current_window.trim();
    if previous_window.is_empty() || current_window.is_empty() {
        return None;
    }

    // An exact-prefix response is only a revision while the previous rolling
    // window is still an unfinished clause. Once it ends at a sentence
    // boundary, the provider may have started a new sequential window with a
    // shared wake/sentence prefix; replacing then would discard the completed
    // sentence.
    let exact_prefix_revision =
        is_strict_prefix_streaming_revision(previous_window, current_window)
            && !previous_window_ends_sentence(previous_window);
    let fuzzy_cumulative_revision =
        is_probable_growing_cumulative_revision(previous_window, current_window);
    if !exact_prefix_revision && !fuzzy_cumulative_revision {
        return None;
    }

    if previous_text == previous_window {
        return Some(current_window.to_string());
    }
    previous_text
        .strip_suffix(previous_window)
        .map(|stable_prefix| format!("{stable_prefix}{current_window}"))
}

fn is_strict_prefix_streaming_revision(previous: &str, current: &str) -> bool {
    let previous = compact_streaming_revision_text(previous);
    let current = compact_streaming_revision_text(current);
    current.chars().count() > previous.chars().count() && current.starts_with(&previous)
}

fn compact_streaming_revision_text(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    '，' | '。'
                        | '、'
                        | '；'
                        | '：'
                        | '！'
                        | '？'
                        | ','
                        | '.'
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                )
        })
        .collect()
}

fn previous_window_ends_sentence(window: &str) -> bool {
    window
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(is_sentence_terminal)
}

/// Early optimized-bidirectional packets often revise the beginning of the
/// same cumulative hypothesis before any utterance timestamps exist. Requiring
/// an identical prefix misclassified those packets as disjoint continuations:
/// installed session 230 grew provider text 16→17→18→23→25 chars while the
/// merged capsule inflated 19→36→37→60→62 chars, then appeared to lose its
/// first half when the authoritative 27-char final arrived.
///
/// Treat a bounded, growing hypothesis with substantial ordered overlap as a
/// revision. A genuinely disjoint next sentence has little ordered overlap and
/// continues through the normal append path below.
fn is_probable_growing_cumulative_revision(previous: &str, current: &str) -> bool {
    let previous = previous.chars().collect::<Vec<_>>();
    let current = current.chars().collect::<Vec<_>>();
    if previous.len() < 6
        || current.len() + 4 < previous.len()
        || current.len().saturating_sub(previous.len()) > 12
    {
        return false;
    }
    let common_prefix_len = previous
        .iter()
        .zip(current.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common_prefix_len < 3 {
        // A response beginning in the middle of the old rolling window is a
        // sequential suffix, not a rewrite of the whole window, regardless of
        // whether its byte/character count grew or shrank.
        // The normal overlap merge must retain the earlier prefix.
        return false;
    }
    if previous[..common_prefix_len]
        .iter()
        .copied()
        .any(is_sentence_terminal)
    {
        // A completed sentence is a hard boundary for this deliberately fuzzy
        // fallback. The active-window/terminal-continuation paths handle
        // sentence-prefixed sequential windows with explicit evidence.
        return false;
    }
    let mut row = vec![0usize; current.len() + 1];
    for previous_char in previous.iter() {
        let mut diagonal = 0usize;
        for (index, current_char) in current.iter().enumerate() {
            let prior = row[index + 1];
            row[index + 1] = if previous_char == current_char {
                diagonal + 1
            } else {
                row[index + 1].max(row[index])
            };
            diagonal = prior;
        }
    }
    row[current.len()] * 5 >= previous.len() * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimistic_cumulative_provider_growth_replaces_instead_of_repeating_clause() {
        let opening = "录音，所以什么意思？然后。";
        let settled = TranscriptSegment {
            start_ms: 0,
            end_ms: Some(4_102),
            text: opening.into(),
        };
        let first = TranscriptCandidate {
            text: format!("{opening}开始"),
            timed_segments: vec![TranscriptSegment {
                start_ms: 4_102,
                end_ms: None,
                text: "开始".into(),
            }],
            authoritative_cumulative: false,
        };
        let (preview, segments, window) = merge_optimistic_cumulative_view(
            opening, &[settled.clone()], opening, first, None,
        );
        assert_eq!(preview, format!("{opening}开始"));
        assert_eq!(segments, vec![settled]);
        let second = TranscriptCandidate {
            text: format!("{opening}开始录音，什么意思？"),
            timed_segments: vec![],
            authoritative_cumulative: false,
        };
        let (preview, _, _) = merge_optimistic_cumulative_view(
            &preview, &segments, &window, second, None,
        );
        assert_eq!(preview, format!("{opening}开始录音，什么意思？"));
    }

    #[test]
    fn first_timed_cumulative_growth_replaces_punctuated_untimed_opening() {
        let opening = "是因为你起床早了，以前你是8、7点。";
        let growth = "是因为你起床早了，以前你是8、7点，是";
        let candidate = TranscriptCandidate {
            text: growth.into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 0,
                end_ms: Some(2_100),
                text: growth.into(),
            }],
            authoritative_cumulative: false,
        };
        for window in [opening, ""] {
            let (merged, _, _) = merge_streaming_candidate_with_untimed_window(
                opening,
                &[],
                window,
                candidate.clone(),
            );
            assert_eq!(merged, growth, "first timed row must replace its own untimed opening");
        }
    }

    #[test]
    fn corrected_owner_prefix_does_not_append_the_superseded_streaming_branch() {
        for (early, owner, foreign) in [
            (
                "然后检查线路和版本等下",
                "先检查设备，然后检查线路和版本。",
                "旁人开始说话。",
            ),
            (
                "Then inspect the cabling and version wait",
                "Check the device, then inspect the cabling and version.",
                "Someone else starts talking.",
            ),
        ] {
            let old = vec![TranscriptSegment {
                start_ms: 5_000,
                end_ms: Some(21_200),
                text: early.into(),
            }];
            let owner_segment = TranscriptSegment {
                start_ms: 480,
                end_ms: Some(20_271),
                text: owner.into(),
            };
            let candidate = TranscriptCandidate {
                text: owner.into(),
                timed_segments: vec![owner_segment.clone()],
                authoritative_cumulative: true,
            };
            let provider = TranscriptCandidate {
                text: format!("{owner}{foreign}"),
                timed_segments: vec![
                    owner_segment,
                    TranscriptSegment {
                        start_ms: 20_271,
                        end_ms: Some(21_391),
                        text: foreign.into(),
                    },
                ],
                authoritative_cumulative: true,
            };
            let (text, segments, window) = merge_filtered_streaming_candidate_with_untimed_window(
                early,
                &old,
                "",
                candidate.clone(),
                Some(&provider),
            );
            assert_eq!(
                text, owner,
                "the full provider timeline supersedes the old rolling interval"
            );
            assert_eq!(segments, candidate.timed_segments);
            assert!(window.is_empty());

            // Missing or overlapping coverage must not authorize this replacement.
            assert!(!authoritative_owner_prefix_covers_ledger(
                &old,
                &candidate,
                &TranscriptCandidate {
                    authoritative_cumulative: false,
                    ..provider.clone()
                }
            ));
            let mut overlapping = provider.clone();
            overlapping.timed_segments[1].start_ms -= 1;
            assert!(!authoritative_owner_prefix_covers_ledger(
                &old,
                &candidate,
                &overlapping
            ));
            let mut missing_time = provider.clone();
            missing_time.timed_segments[1].end_ms = None;
            assert!(!authoritative_owner_prefix_covers_ledger(
                &old,
                &candidate,
                &missing_time
            ));
            let mut suffix_only = candidate.clone();
            suffix_only.text = foreign.into();
            suffix_only.timed_segments = provider.timed_segments[1..].to_vec();
            assert!(!authoritative_owner_prefix_covers_ledger(
                &old,
                &suffix_only,
                &provider
            ));

            // A later-only provider revision cannot discard an earlier sentence.
            let earlier = TranscriptSegment {
                start_ms: -2_000,
                end_ms: Some(-1_000),
                text: "Earlier. ".into(),
            };
            let previous = format!("Earlier. {early}");
            let history = vec![earlier, old[0].clone()];
            let (text, _, _) = merge_filtered_streaming_candidate_with_untimed_window(
                &previous,
                &history,
                "",
                candidate,
                Some(&provider),
            );
            assert!(
                text.contains("Earlier."),
                "uncovered earlier speech must survive"
            );
        }
    }

    fn untimed(text: &str) -> TranscriptCandidate {
        TranscriptCandidate {
            text: text.into(),
            timed_segments: Vec::new(),
            authoritative_cumulative: false,
        }
    }

    #[test]
    fn corrected_cumulative_growth_keeps_one_copy_of_the_active_clause() {
        // Installed session 5e6056fa, frames 16-19: the provider corrected
        // "他感觉" to "它，我感觉" and extended the same session-start row.
        // The old exact-prefix branch missed that one-word correction and
        // appended "这个我想说的话" twice before the early paste.
        let previous = "开始录音。然后还有些问题，就是他感觉有时候高有时候低。这个";
        let revisions = [
            "开始录音。然后还有些问题，就是它，我感觉有时候高有时候低。这个我",
            "开始录音。然后还有些问题，就是它，我感觉有时候高有时候低。这个我想说",
            "开始录音。然后还有些问题，就是它，我感觉有时候高有时候低。这个我想说的话",
        ];
        let mut ledger = previous.to_string();
        let mut segments = vec![TranscriptSegment {
            start_ms: 100,
            end_ms: Some(4_900),
            text: previous.into(),
        }];
        let mut window = previous.to_string();
        for (index, revision) in revisions.iter().enumerate() {
            let candidate = TranscriptCandidate {
                text: (*revision).into(),
                timed_segments: vec![TranscriptSegment {
                    start_ms: 100,
                    end_ms: Some(5_600 + index as i64 * 500),
                    text: (*revision).into(),
                }],
                authoritative_cumulative: true,
            };
            (ledger, segments, window) = merge_streaming_candidate_with_untimed_window(
                &ledger,
                &segments,
                &window,
                candidate,
            );
            assert_eq!(ledger, *revision);
            assert_eq!(ledger.matches("这个我想说的话").count(), usize::from(index == 2));
            assert_eq!(segments.len(), 1);
        }
    }

    #[test]
    fn cumulative_growth_after_timed_opening_replaces_instead_of_repeating_tail() {
        let previous = "开始录音。如果没有这种东西的话，就要做成规划。";
        let current = "开始录音。如果没有这种东西的话，就要做成规划。然后呢？做完以后";
        let opening = TranscriptSegment {
            start_ms: 0,
            end_ms: Some(10_000),
            text: previous.into(),
        };
        let growth = TranscriptSegment {
            start_ms: 10_000,
            end_ms: Some(13_000),
            text: "然后呢？做完以后".into(),
        };
        let (merged, segments, _) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[opening.clone()],
            "然后呢？",
            timed(current, vec![opening, growth]),
        );
        assert_eq!(merged, current);
        assert_eq!(merged.matches("然后呢").count(), 1);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn timed_handoff_of_equal_provisional_tail_does_not_repeat_clause() {
        let opening = "开始录音，就是机器的话，你就做出来让我现场安装就好。";
        let first_tail = "然后你看一下";
        let ledger = format!("{opening}{first_tail}");
        let opening_segment = TranscriptSegment {
            start_ms: 0,
            end_ms: Some(3_000),
            text: opening.into(),
        };
        let tail_segment = TranscriptSegment {
            start_ms: 3_000,
            end_ms: Some(4_500),
            text: first_tail.into(),
        };
        let (merged, segments, window) = merge_streaming_candidate_with_untimed_window(
            &ledger,
            &[opening_segment.clone()],
            first_tail,
            timed(&ledger, vec![opening_segment.clone(), tail_segment]),
        );
        assert_eq!(merged, ledger);
        assert_eq!(merged.matches(first_tail).count(), 1);
        assert_eq!(segments.len(), 2);

        let grown = format!("{ledger}你做出来需要我帮忙的东西");
        let (merged, _, _) = merge_streaming_candidate_with_untimed_window(
            &merged,
            &segments,
            &window,
            timed(
                &grown,
                vec![
                    opening_segment,
                    TranscriptSegment {
                        start_ms: 3_000,
                        end_ms: Some(6_000),
                        text: grown.strip_prefix(opening).unwrap().into(),
                    },
                ],
            ),
        );
        assert_eq!(merged, grown);
        assert_eq!(merged.matches(first_tail).count(), 1);
    }

    fn timed(text: &str, segments: Vec<TranscriptSegment>) -> TranscriptCandidate {
        TranscriptCandidate {
            text: text.into(),
            timed_segments: segments,
            authoritative_cumulative: true,
        }
    }

    #[test]
    fn late_opening_final_republication_does_not_double_in_ledger() {
        // Installed session 4c3b4b52 (2026-09-19 20:58): the punctuated final
        // of the opening sentence arrived after the next clause's preview
        // ("就帮") had already streamed into the ledger. The candidate
        // re-covers the session opening; appending it made the whole opening
        // sentence appear twice in the delivered text.
        let opening = "开始录音。你帮我看一下，就是你推荐我的执行器怎么用？如果是一种通用的话。";
        let ledger = format!("{opening}就帮");
        let corrected_ledger =
            "开始录音，你帮我看一下，就是你推荐我的执行器怎么弄？如果是一种通用的话。就帮";
        let wake_segment = TranscriptSegment {
            start_ms: 0,
            end_ms: Some(900),
            text: "开始录音。".into(),
        };
        let opening_segment = TranscriptSegment {
            start_ms: 0,
            end_ms: Some(9_000),
            text: opening.into(),
        };
        let late_segment = TranscriptSegment {
            start_ms: 9_500,
            end_ms: Some(18_000),
            text: opening.into(),
        };
        let candidate_of = |segment: TranscriptSegment| TranscriptCandidate {
            text: opening.into(),
            timed_segments: vec![segment],
            authoritative_cumulative: false,
        };
        let mut failures = Vec::new();
        for (label, ledger, segments, window, candidate) in [
            (
                "no-history/empty-window",
                ledger.as_str(),
                Vec::new(),
                "",
                candidate_of(opening_segment.clone()),
            ),
            (
                "no-history/window=completed-opening",
                ledger.as_str(),
                Vec::new(),
                opening,
                candidate_of(opening_segment.clone()),
            ),
            (
                "wake-history/empty-window",
                ledger.as_str(),
                vec![wake_segment.clone()],
                "",
                candidate_of(opening_segment.clone()),
            ),
            (
                "wake-history/window=completed-opening",
                ledger.as_str(),
                vec![wake_segment.clone()],
                opening,
                candidate_of(opening_segment.clone()),
            ),
            (
                "wake-history/streaming-wording-corrected",
                corrected_ledger,
                vec![wake_segment.clone()],
                opening,
                candidate_of(opening_segment.clone()),
            ),
        ] {
            let (text, _, _) = merge_streaming_candidate_with_untimed_window(
                ledger,
                &segments,
                window,
                candidate,
            );
            let spoken: String = text.chars().filter(|ch| ch.is_alphanumeric()).collect();
            let opening_spoken: String = opening
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .collect();
            let doubled = spoken.matches(&opening_spoken).count() > 1
                || spoken.chars().count() > ledger.chars().count() + 2;
            if doubled {
                failures.push(format!("{label}: {text}"));
            }
        }
        assert!(
            failures.is_empty(),
            "late opening re-publication doubled the ledger: {failures:?}"
        );

        // A genuine later re-statement carries later timed coverage and must
        // still append: dropping it would silently eat real dictation.
        let (text, _, _) = merge_streaming_candidate_with_untimed_window(
            &ledger,
            &[wake_segment, opening_segment],
            opening,
            candidate_of(late_segment),
        );
        let spoken: String = text.chars().filter(|ch| ch.is_alphanumeric()).collect();
        let opening_spoken: String = opening
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect();
        assert_eq!(
            spoken.matches(&opening_spoken).count(),
            2,
            "genuine later re-statement must be retained: {text}"
        );
    }

    #[test]
    fn rolling_window_revisions_replace_the_active_suffix_without_inflation() {
        let first = "前面的正文保持完整，然后未知的话就默认是";
        let moved = "然后未知的话就默认是百分之八十";
        let revised = "然后未知的话就默认是百分之八十或者百分之百";

        let (text, segments, window) =
            merge_streaming_candidate_with_untimed_window(first, &[], first, untimed(moved));
        assert_eq!(text, "前面的正文保持完整，然后未知的话就默认是百分之八十");

        let (text, _, _) = merge_streaming_candidate_with_untimed_window(
            &text,
            &segments,
            &window,
            untimed(revised),
        );
        assert_eq!(
            text,
            "前面的正文保持完整，然后未知的话就默认是百分之八十或者百分之百"
        );
        assert!(!text.contains("百分之八十然后未知"));
    }

    #[test]
    fn growing_window_after_terminal_prefix_is_replaced_without_replaying_old_tail() {
        // The physical 2026-09-14 trace had a completed prefix followed by a
        // rolling window that grew from "...今晚" to "...今晚吃".  Treating
        // the shared sentence prefix as a new sequential clause duplicated
        // the old window in fallback_partial_callback.
        let previous = "主句。旁边的人正在讨论今晚";
        let current = "主句。旁边的人正在讨论今晚吃";
        let (text, _, window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            untimed(current),
        );
        assert_eq!(text, current);
        assert_eq!(window, current);
        assert_eq!(text.matches("旁边的人正在讨论").count(), 1);
    }

    #[test]
    fn installed_session_style_revision_chain_stays_bounded_by_prefix_plus_latest_window() {
        let mut text = "正文开头，然后未知的话就默认是".to_string();
        let mut window = text.clone();
        let revisions = [
            "然后未知的话就默认是百分之八",
            "然后未知的话就默认是百分之八十或者",
            "然后未知的话就默认是80%或者",
            "然后未知的话就默认是80%或者100%就好",
        ];

        for revision in revisions {
            let (merged, _, current_window) = merge_streaming_candidate_with_untimed_window(
                &text,
                &[],
                &window,
                untimed(revision),
            );
            text = merged;
            window = current_window;
        }

        assert_eq!(text, "正文开头，然后未知的话就默认是80%或者100%就好");
        assert_eq!(text.matches("然后未知的话").count(), 1);
    }

    #[test]
    fn disjoint_untimed_continuation_is_still_appended() {
        let (text, _, _) = merge_streaming_candidate_with_untimed_window(
            "第一句已经结束。",
            &[],
            "第一句已经结束。",
            untimed("第二句继续说明。"),
        );
        assert_eq!(text, "第一句已经结束。第二句继续说明。");
    }

    #[test]
    fn sequential_windows_after_a_completed_prefix_accumulate_body_clauses() {
        assert_eq!(
            append_after_shared_terminal_prefix("锚点。甲段。", "锚点。甲段。", "锚点。乙段。",),
            Some("锚点。甲段。乙段。".into())
        );
        let mut text = "锚点。".to_string();
        let mut window = text.clone();
        for revision in ["锚点。甲段。", "锚点。乙段。", "锚点。丙段。"] {
            let (merged, _, current_window) = merge_streaming_candidate_with_untimed_window(
                &text,
                &[],
                &window,
                untimed(revision),
            );
            text = merged;
            window = current_window;
        }
        assert_eq!(text, "锚点。甲段。乙段。丙段。");

        let mut text = "开始录音。".to_string();
        let mut window = text.clone();
        for revision in [
            "开始录音。先检查键盘",
            "开始录音。再确认语音输入",
            "开始录音。然后结束这句话",
        ] {
            let (merged, _, current_window) = merge_streaming_candidate_with_untimed_window(
                &text,
                &[],
                &window,
                untimed(revision),
            );
            text = merged;
            window = current_window;
        }
        assert_eq!(text, "开始录音。先检查键盘，再确认语音输入，然后结束这句话");

        assert_eq!(
            append_after_shared_terminal_prefix(
                "Anchor. first clause",
                "Anchor. first clause",
                "Anchor. second clause",
            ),
            Some("Anchor. first clause second clause".into())
        );
    }

    #[test]
    fn completed_sentence_prefix_keeps_short_equal_long_and_similar_next_windows() {
        let previous = "开始录音。甲段。";
        for suffix in [
            "乙。",
            "乙段。",
            "甲段正在继续说明。",
            "乙段继续说明并记录结果。",
        ] {
            let current = format!("开始录音。{suffix}");
            let (text, _, _) = merge_streaming_candidate_with_untimed_window(
                previous,
                &[],
                previous,
                untimed(&current),
            );
            assert_eq!(
                text,
                format!("{previous}{suffix}"),
                "a completed sentence boundary must preserve the prior window"
            );
        }
    }

    #[test]
    fn completed_sentence_prefix_does_not_turn_a_different_unfinished_tail_into_revision() {
        let previous = "这是已经完成的共同句首。甲段";
        let current = "这是已经完成的共同句首。乙段继续";
        let (text, _, window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            untimed(current),
        );

        assert_eq!(text, "这是已经完成的共同句首。甲段，乙段继续");
        assert_eq!(window, current);
        assert_eq!(text.matches("这是已经完成的共同句首").count(), 1);
        assert!(
            text.contains("甲段"),
            "the earlier unfinished tail must survive"
        );
    }

    #[test]
    fn punctuation_only_revision_replaces_the_active_window_once() {
        let previous = "开始录音。今天去北京。";
        let current = "开始录音。今天去北京！";
        let (text, _, window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            untimed(current),
        );

        assert_eq!(text, current);
        assert_eq!(window, current);
        assert_eq!(text.matches("今天去北京").count(), 1);
    }

    #[test]
    fn completed_window_prefix_appends_only_the_new_cumulative_tail() {
        let previous = "开始录音。窗口不要报错，预览文字要及时稳定出现。";
        let current = "开始录音。窗口不要报错，预览文字要及时稳定出现，然后继续。";
        assert_eq!(
            shared_terminal_prefix_suffix(previous, current).as_deref(),
            Some("，然后继续。")
        );
        let (text, _, window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            untimed(current),
        );

        assert_eq!(text, current);
        assert_eq!(text.matches("窗口不要报错").count(), 1);
        assert_eq!(window, current);
    }

    #[test]
    fn timed_completed_window_revises_only_when_intervals_overlap() {
        let previous = "重复句。";
        let current = "重复句。然后继续。";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 1_000,
            end_ms: Some(3_000),
            text: previous.into(),
        }];

        let same_interval = timed(
            current,
            vec![TranscriptSegment {
                start_ms: 1_000,
                end_ms: Some(4_000),
                text: current.into(),
            }],
        );
        let (revised_text, revised_segments, _) = merge_streaming_candidate_with_untimed_window(
            previous,
            &previous_segments,
            previous,
            same_interval,
        );
        assert_eq!(revised_text, current);
        assert_eq!(revised_text.matches("重复句").count(), 1);
        assert_eq!(revised_segments.len(), 1);

        let later_interval = timed(
            current,
            vec![TranscriptSegment {
                start_ms: 5_000,
                end_ms: Some(7_000),
                text: current.into(),
            }],
        );
        let (repeated_text, repeated_segments, _) = merge_streaming_candidate_with_untimed_window(
            previous,
            &previous_segments,
            previous,
            later_interval,
        );
        assert_eq!(repeated_text.matches("重复句").count(), 2);
        assert_eq!(repeated_segments.len(), 2);
    }

    #[test]
    fn timed_punctuation_only_revision_updates_the_owned_segment_once() {
        let previous = "开始录音。今天去北京。";
        let current = "开始录音。今天去北京！";
        let previous_segments = vec![
            TranscriptSegment {
                start_ms: 120,
                end_ms: Some(2_000),
                text: "开始录音。".into(),
            },
            TranscriptSegment {
                start_ms: 2_000,
                end_ms: Some(4_000),
                text: "今天去北京。".into(),
            },
        ];
        let candidate = timed(
            current,
            vec![
                previous_segments[0].clone(),
                TranscriptSegment {
                    start_ms: 2_000,
                    end_ms: Some(4_100),
                    text: "今天去北京！".into(),
                },
            ],
        );
        let (text, segments, _) = merge_streaming_candidate_with_untimed_window(
            previous,
            &previous_segments,
            previous,
            candidate,
        );

        assert_eq!(text, current);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].text, "今天去北京！");
        assert_eq!(text.matches("今天去北京").count(), 1);
    }

    #[test]
    fn completed_sentence_prefix_keeps_untimed_to_timed_next_window_segments() {
        let previous = "开始录音。甲段。";
        let previous_segments = vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: Some(1_000),
                text: "开始录音。".into(),
            },
            TranscriptSegment {
                start_ms: 1_000,
                end_ms: Some(2_000),
                text: "甲段。".into(),
            },
        ];
        let candidate = timed(
            "开始录音。乙段继续说明。",
            vec![TranscriptSegment {
                start_ms: 2_000,
                end_ms: Some(4_000),
                text: "乙段继续说明。".into(),
            }],
        );

        let (text, segments, _) = merge_streaming_candidate_with_untimed_window(
            previous,
            &previous_segments,
            previous,
            candidate,
        );

        assert_eq!(text, "开始录音。甲段。乙段继续说明。");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[1].text, "甲段。");
        assert_eq!(segments[2].text, "乙段继续说明。");
    }

    #[test]
    fn timed_sequential_windows_keep_history_and_accumulate_each_new_clause() {
        let wake = TranscriptSegment {
            start_ms: 120,
            end_ms: Some(2_102),
            text: "开始录音。".into(),
        };
        let body = |end_ms, text: &str| TranscriptSegment {
            start_ms: 2_102,
            end_ms: Some(end_ms),
            text: text.into(),
        };
        let first = timed(
            "开始录音。先检查键盘",
            vec![wake.clone(), body(6_000, "先检查键盘")],
        );
        let (mut text, mut segments, mut window) =
            merge_streaming_candidate_with_untimed_window("", &[], "", first);
        assert_eq!(text, "开始录音。先检查键盘");
        assert_eq!(segments.len(), 2);

        for (clause, end_ms) in [("再确认语音输入", 8_000), ("然后结束这句话", 9_912)]
        {
            let candidate = timed(
                &format!("开始录音。{clause}"),
                vec![wake.clone(), body(end_ms, clause)],
            );
            let (merged, next_segments, next_window) =
                merge_streaming_candidate_with_untimed_window(&text, &segments, &window, candidate);
            text = merged;
            segments = next_segments;
            window = next_window;
        }

        assert_eq!(text, "开始录音。先检查键盘，再确认语音输入，然后结束这句话");
        assert_eq!(
            segments.len(),
            2,
            "wake and the growing body row remain unique"
        );
        assert_eq!(segments[1].start_ms, 2_102);
        assert_eq!(segments[1].end_ms, Some(9_912));
        assert_eq!(
            segments[1].text,
            "先检查键盘，再确认语音输入，然后结束这句话"
        );
    }

    #[test]
    fn timed_sequential_windows_with_disjoint_timestamps_append_segments() {
        let wake = |end_ms| TranscriptSegment {
            start_ms: 0,
            end_ms: Some(end_ms),
            text: "开始录音。".into(),
        };
        let first = timed(
            "开始录音。先检查键盘",
            vec![
                wake(1_000),
                TranscriptSegment {
                    start_ms: 1_000,
                    end_ms: Some(3_000),
                    text: "先检查键盘".into(),
                },
            ],
        );
        let (mut text, mut segments, mut window) =
            merge_streaming_candidate_with_untimed_window("", &[], "", first);
        assert_eq!(text, "开始录音。先检查键盘");
        assert_eq!(segments.len(), 2);

        for (clause, start_ms, end_ms) in [
            ("再确认语音输入", 3_000, 5_000),
            ("然后结束这句话", 5_000, 7_000),
        ] {
            let candidate = timed(
                &format!("开始录音。{clause}"),
                vec![
                    wake(1_000),
                    TranscriptSegment {
                        start_ms,
                        end_ms: Some(end_ms),
                        text: clause.into(),
                    },
                ],
            );
            let (merged, next_segments, next_window) =
                merge_streaming_candidate_with_untimed_window(&text, &segments, &window, candidate);
            text = merged;
            segments = next_segments;
            window = next_window;
        }

        assert_eq!(text, "开始录音。先检查键盘，再确认语音输入，然后结束这句话");
        assert_eq!(
            segments.len(),
            4,
            "wake, each owner clause, and history must remain"
        );
        assert_eq!(segments[1].text, "先检查键盘");
        assert_eq!(segments[2].text, "再确认语音输入");
        assert_eq!(segments[3].text, "然后结束这句话");
    }

    #[test]
    fn anchored_active_window_growth_preserves_accumulated_text_for_both_paths() {
        let mut text = "开始录音。先检查键盘，再确认语音输入".to_string();
        let mut window = "开始录音。再确认语音输入".to_string();
        for revision in [
            "开始录音。然后",
            "开始录音。然后结束",
            "开始录音。然后结束这句话",
        ] {
            let (merged, _, next_window) = merge_streaming_candidate_with_untimed_window(
                &text,
                &[],
                &window,
                untimed(revision),
            );
            text = merged;
            window = next_window;
        }
        assert_eq!(text, "开始录音。先检查键盘，再确认语音输入，然后结束这句话");

        let wake = TranscriptSegment {
            start_ms: 120,
            end_ms: Some(2_102),
            text: "开始录音。".into(),
        };
        let body = |end_ms, text: &str| TranscriptSegment {
            start_ms: 2_102,
            end_ms: Some(end_ms),
            text: text.into(),
        };
        let mut timed_text = "开始录音。先检查键盘，再确认语音输入".to_string();
        let mut timed_window = "开始录音。再确认语音输入".to_string();
        let mut timed_segments = vec![wake.clone(), body(6_000, "先检查键盘，再确认语音输入")];
        for (revision, body_text, end_ms) in [
            ("开始录音。然后", "然后", 7_000),
            ("开始录音。然后结束", "然后结束", 8_000),
            ("开始录音。然后结束这句话", "然后结束这句话", 9_912),
        ] {
            let (merged, next_segments, next_window) =
                merge_streaming_candidate_with_untimed_window(
                    &timed_text,
                    &timed_segments,
                    &timed_window,
                    timed(revision, vec![wake.clone(), body(end_ms, body_text)]),
                );
            timed_text = merged;
            timed_segments = next_segments;
            timed_window = next_window;
        }
        assert_eq!(timed_text, text);
        assert_eq!(timed_segments.len(), 2);
        assert_eq!(timed_segments[1].end_ms, Some(9_912));
        assert_eq!(
            timed_segments[1].text,
            "先检查键盘，再确认语音输入，然后结束这句话"
        );
    }

    #[test]
    fn first_word_timing_preserves_rolling_revision_and_unrelated_prefix() {
        for (prefix, early, timed) in [
            ("开始录音", "今天的工作顺利是先", "今天的工作顺利，是先完成"),
            ("前面一段。", "下周我们准备检查", "下周，我们准备检查线路"),
            (
                "Earlier sentence. ",
                "Next we will inspect",
                "Next, we will inspect the cable",
            ),
        ] {
            let previous = format!("{prefix}{early}");
            let candidate = TranscriptCandidate {
                text: timed.into(),
                timed_segments: vec![TranscriptSegment {
                    start_ms: 1080,
                    end_ms: Some(3399),
                    text: timed.into(),
                }],
                authoritative_cumulative: false,
            };
            let (text, segments, window) = merge_streaming_candidate_with_untimed_window(
                &previous,
                &[],
                early,
                candidate.clone(),
            );
            assert_eq!(text, format!("{prefix}{timed}"));
            assert_eq!(segments, candidate.timed_segments);
            assert!(window.is_empty());
        }
    }

    #[test]
    fn first_word_timing_still_appends_a_disjoint_sentence() {
        let candidate = TranscriptCandidate {
            text: "第二项检查线路。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 5000,
                end_ms: Some(7000),
                text: "第二项检查线路。".into(),
            }],
            authoritative_cumulative: false,
        };
        let (text, _, _) = merge_streaming_candidate_with_untimed_window(
            "第一项已经完成。",
            &[],
            "第一项已经完成。",
            candidate,
        );
        assert_eq!(text, "第一项已经完成。第二项检查线路。");
    }

    #[test]
    fn first_authoritative_timed_full_revision_replaces_untimed_session_ledger() {
        let previous = "开始录音今天做顺序是先完成设备配对和权限检查然后测试一段大约二20秒的自然中文语音最后再提交前确认预览已经逐步收敛到最终文本，任何失败都要保留时间点和可追溯原因。我";
        let corrected = "开始录音。经错顺序是先完成设备配对和权限检查，然后测试一段大约20秒的自然中文语音，最后在提交前确认预览已经逐步收敛到最终文本。任何失败都要保留时间点和可追溯原因。";
        let candidate = TranscriptCandidate {
            text: corrected.into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 480,
                end_ms: Some(20_271),
                text: corrected.into(),
            }],
            authoritative_cumulative: true,
        };

        let (text, segments, window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            "最后再提交前确认预览已经逐步收敛到最终文本，任何失败都要保留时间点和可追溯原因。我",
            candidate.clone(),
        );

        assert_eq!(text, corrected);
        assert_eq!(segments, candidate.timed_segments);
        assert!(window.is_empty());
        assert_eq!(text.matches("设备配对和权限检查").count(), 1);
    }

    #[test]
    fn first_timed_full_revision_requires_full_ledger_edges_and_sentence_coverage() {
        let previous = "开始录音。第一段是一个很长很长很长很长很长的正文。短句。最后一段也是一段很长很长很长很长很长的正文。";
        let candidates = [
            "开始录音。第一段是一个很长很长很长很长很长的正文。",
            "最后一段也是一段很长很长很长很长很长的正文。",
            "开始录音。第一段是一个很长很长很长很长很长的正文。最后一段也是一段很长很长很长很长很长的正文。",
            "开始录音。第一段是一个很长很长很长很长很长的正文。补充。最后一段也是一段很长很长很长很长很长的正文。",
        ];

        for text in candidates {
            let candidate = timed(
                text,
                vec![TranscriptSegment {
                    start_ms: 480,
                    end_ms: Some(12_000),
                    text: text.into(),
                }],
            );
            assert!(
                !authoritative_timed_revision_covers_untimed_ledger(previous, &candidate),
                "partial or sentence-dropping revision must not clear the old ledger: {text}"
            );
            let (merged, _, _) =
                merge_streaming_candidate_with_untimed_window(previous, &[], previous, candidate);
            assert_eq!(
                merged, previous,
                "an incomplete first timed revision must leave the full old ledger intact: {text}"
            );
        }

        let missing_short_sentence = alphanumeric_chars(candidates[2]);
        let previous_chars = alphanumeric_chars(previous);
        assert!(
            longest_common_subsequence_len(&previous_chars, &missing_short_sentence)
                .saturating_mul(100)
                >= previous_chars.len().saturating_mul(90),
            "the regression must exercise a high-LCS but incomplete revision"
        );
    }

    #[test]
    fn first_and_repeated_full_revision_never_borrow_an_old_sentence_tail() {
        let previous = "第一句保留完整的正文尾部。正文。第三句保留全部内容。";
        let incomplete = "第一句保留完整的。正文第三句保留全部内容。";
        let candidate = timed(
            incomplete,
            vec![TranscriptSegment {
                start_ms: 480,
                end_ms: Some(8_000),
                text: incomplete.into(),
            }],
        );

        let (first_text, _, first_window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            candidate.clone(),
        );
        assert_eq!(first_text, previous);
        assert_eq!(first_text.matches("正文").count(), 2);

        let (repeated_text, _, _) = merge_streaming_candidate_with_untimed_window(
            &first_text,
            &[],
            &first_window,
            candidate,
        );
        assert_eq!(repeated_text, previous);
        assert_eq!(repeated_text.matches("正文").count(), 2);
    }

    #[test]
    fn ambiguous_repeated_short_sentence_cannot_authorize_first_or_repeated_revision() {
        let previous = "开始录音。确认。确认设备配对权限检查全部完成然后记录测试结果。";
        let incomplete = "开始录音。确认设备配对权限检查。全部完成然后记录测试结果。";
        let candidate = timed(
            incomplete,
            vec![TranscriptSegment {
                start_ms: 480,
                end_ms: Some(8_000),
                text: incomplete.into(),
            }],
        );

        let (first_text, _, first_window) = merge_streaming_candidate_with_untimed_window(
            previous,
            &[],
            previous,
            candidate.clone(),
        );
        assert_eq!(first_text, previous);

        let (repeated_text, _, _) = merge_streaming_candidate_with_untimed_window(
            &first_text,
            &[],
            &first_window,
            candidate,
        );
        assert_eq!(repeated_text, previous);
        assert_eq!(repeated_text.matches("确认").count(), 2);
    }

    #[test]
    fn later_authoritative_timed_utterance_preserves_earlier_untimed_text() {
        let candidate = TranscriptCandidate {
            text: "第二项检查线路并记录结果。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 5_000,
                end_ms: Some(7_000),
                text: "第二项检查线路并记录结果。".into(),
            }],
            authoritative_cumulative: true,
        };

        let (text, _, _) = merge_streaming_candidate_with_untimed_window(
            "第一项已经完成。",
            &[],
            "第一项已经完成。",
            candidate,
        );

        assert_eq!(text, "第一项已经完成。第二项检查线路并记录结果。");
    }

    #[test]
    fn early_timed_disjoint_new_sentence_enters_the_ledger() {
        let previous = "开始录音。旧句需要继续保留在当前会话中。";
        let current = "开始录音。新的句子应该进入当前会话。";
        let candidate = timed(
            current,
            vec![TranscriptSegment {
                start_ms: 480,
                end_ms: Some(2_800),
                text: current.into(),
            }],
        );

        let (text, segments, _) =
            merge_streaming_candidate_with_untimed_window(previous, &[], previous, candidate);

        assert!(text.contains("旧句需要继续保留"));
        assert!(text.contains("新的句子应该进入"));
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn installed_session_230_growing_cumulative_revisions_never_inflate_preview() {
        let revisions = [
            "开始录音你上次的话吞了前半",
            "开始录音上次的话吞了我前半截",
            "开始录音你上次那句话吞了我前半截",
            "开始录音你上次那句话好像吞了我的前半截",
        ];
        let mut text = revisions[0].to_string();
        let mut window = text.clone();
        for revision in revisions.iter().skip(1) {
            let (merged, _, current_window) = merge_streaming_candidate_with_untimed_window(
                &text,
                &[],
                &window,
                untimed(revision),
            );
            assert!(
                merged.chars().count() <= revision.chars().count(),
                "cumulative revision must replace instead of append: {merged}"
            );
            text = merged;
            window = current_window;
        }
        assert_eq!(text, revisions[revisions.len() - 1]);
    }

    #[test]
    fn equal_length_front_rewrite_is_not_assumed_to_be_a_cumulative_revision() {
        assert!(!is_probable_growing_cumulative_revision(
            "甲方正在说明第一条",
            "乙方正在说明第二条"
        ));
    }
}
