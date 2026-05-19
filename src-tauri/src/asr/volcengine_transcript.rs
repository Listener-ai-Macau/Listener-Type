//! Volcengine ASR streaming transcript merge logic.
//!
//! Pure functions for parsing and merging streaming ASR responses.
//! Extracted from volcengine.rs for testability.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TranscriptSegment {
    start_ms: i64,
    end_ms: Option<i64>,
    text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TranscriptCandidate {
    pub(super) text: String,
    pub(super) timed_segments: Vec<TranscriptSegment>,
}

pub(super) fn normalized_result(json: &Value) -> Option<&Value> {
    if let Some(obj) = json.get("result") {
        if obj.is_object() {
            return Some(obj);
        }
        if let Some(arr) = obj.as_array() {
            if let Some(first) = arr.first() {
                return Some(first);
            }
        }
    }
    if json.get("text").and_then(|v| v.as_str()).is_some() {
        return Some(json);
    }
    None
}

pub(super) fn transcript_text_from_result(result: &Value) -> String {
    transcript_candidate_from_result(result).text
}

pub(super) fn transcript_candidate_from_result(result: &Value) -> TranscriptCandidate {
    let result_text = result.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let (utterance_text, timed_segments) =
        if let Some(utterances) = result.get("utterances").and_then(|v| v.as_array()) {
            let mut pieces: Vec<&str> = Vec::new();
            let mut timed_segments = Vec::new();
            for utterance in utterances {
                if let Some(text) = utterance.get("text").and_then(|t| t.as_str()) {
                    pieces.push(text);
                }
                if let Some(segment) = transcript_segment_from_utterance(utterance) {
                    timed_segments.push(segment);
                }
            }
            (pieces.join(""), timed_segments)
        } else {
            (String::new(), Vec::new())
        };

    TranscriptCandidate {
        text: choose_transcript_text(result_text, &utterance_text),
        timed_segments,
    }
}

fn transcript_segment_from_utterance(utterance: &Value) -> Option<TranscriptSegment> {
    let text = utterance.get("text").and_then(|t| t.as_str())?.trim();
    if text.is_empty() {
        return None;
    }
    let start_ms = first_word_millis_field(utterance, &["start_time", "startTime", "start_ms"])
        .or_else(|| value_millis_field(utterance, &["start_time", "startTime", "start_ms"]))?;
    let end_ms = last_word_millis_field(utterance, &["end_time", "endTime", "end_ms"])
        .or_else(|| value_millis_field(utterance, &["end_time", "endTime", "end_ms"]));
    Some(TranscriptSegment {
        start_ms,
        end_ms,
        text: text.to_string(),
    })
}

fn first_word_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    value
        .get("words")
        .and_then(|words| words.as_array())
        .and_then(|words| words.iter().find_map(|word| value_millis_field(word, keys)))
}

fn last_word_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    value
        .get("words")
        .and_then(|words| words.as_array())
        .and_then(|words| {
            words
                .iter()
                .rev()
                .find_map(|word| value_millis_field(word, keys))
        })
}

fn value_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(value_as_i64)
}

fn value_as_i64(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok();
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() {
            return Some(n.round() as i64);
        }
    }
    value.as_str()?.trim().parse::<i64>().ok()
}

fn choose_transcript_text(result_text: &str, utterance_text: &str) -> String {
    let result_text = result_text.trim();
    let utterance_text = utterance_text.trim();
    if result_text.is_empty() {
        return utterance_text.to_string();
    }
    if utterance_text.is_empty() {
        return result_text.to_string();
    }
    if has_duplicate_prefix_before_suffix(result_text, utterance_text) {
        return utterance_text.to_string();
    }
    if has_duplicate_prefix_before_suffix(utterance_text, result_text) {
        return result_text.to_string();
    }
    if has_duplicate_tail_after_full_revision(utterance_text, result_text) {
        return result_text.to_string();
    }
    if has_duplicate_tail_after_full_revision(result_text, utterance_text) {
        return utterance_text.to_string();
    }
    if result_text.contains(utterance_text) {
        return result_text.to_string();
    }
    if utterance_text.contains(result_text) {
        return utterance_text.to_string();
    }
    if utterance_text.chars().count() > result_text.chars().count() {
        utterance_text.to_string()
    } else {
        result_text.to_string()
    }
}

fn merge_streaming_transcript(previous: &str, current: &str) -> String {
    let previous = previous.trim();
    let current = current.trim();
    if previous.is_empty() {
        return current.to_string();
    }
    if current.is_empty() {
        return previous.to_string();
    }
    if has_duplicate_prefix_before_suffix(current, previous) {
        return previous.to_string();
    }
    if current.contains(previous) {
        return current.to_string();
    }
    if previous.contains(current) {
        return previous.to_string();
    }
    if compact_transcript_contains_window(previous, current) {
        return previous.to_string();
    }
    if compact_transcript_contains_window(current, previous) {
        return current.to_string();
    }

    if is_same_prefix_streaming_revision(previous, current) {
        return current.to_string();
    }

    let previous_chars: Vec<char> = previous.chars().collect();
    let current_chars: Vec<char> = current.chars().collect();
    let max_overlap = previous_chars.len().min(current_chars.len());
    for overlap in (2..=max_overlap).rev() {
        if previous_chars[previous_chars.len() - overlap..] == current_chars[..overlap] {
            let suffix: String = current_chars[overlap..].iter().collect();
            return format!("{previous}{suffix}");
        }
    }

    if current_chars.len() < 4 {
        return previous.to_string();
    }

    join_transcript_segments(previous, current)
}

fn has_duplicate_prefix_before_suffix(candidate: &str, stable_suffix: &str) -> bool {
    let Some(leading) = candidate.strip_suffix(stable_suffix) else {
        return false;
    };
    is_duplicate_transcript_prefix(stable_suffix, leading)
}

fn is_duplicate_transcript_prefix(full_text: &str, prefix: &str) -> bool {
    let full = compact_transcript_for_duplicate_check(full_text);
    let prefix = compact_transcript_for_duplicate_check(prefix);
    prefix.chars().count() >= 4 && full.starts_with(&prefix)
}

fn has_duplicate_tail_after_full_revision(candidate: &str, stable_full: &str) -> bool {
    const MIN_DUPLICATE_TAIL_CHARS: usize = 8;
    const MAX_DUPLICATE_TAIL_CER: f64 = 0.18;

    let candidate = compact_transcript_for_duplicate_check(candidate);
    let stable_full = compact_transcript_for_duplicate_check(stable_full);
    if candidate == stable_full || !candidate.starts_with(&stable_full) {
        return false;
    }

    let stable_len = stable_full.chars().count();
    let duplicate_tail: String = candidate.chars().skip(stable_len).collect();
    let duplicate_len = duplicate_tail.chars().count();
    if duplicate_len < MIN_DUPLICATE_TAIL_CHARS || duplicate_len > stable_len {
        return false;
    }

    let stable_suffix: String = stable_full
        .chars()
        .skip(stable_len - duplicate_len)
        .collect();
    let distance = char_edit_distance(&stable_suffix, &duplicate_tail);
    (distance as f64 / duplicate_len as f64) <= MAX_DUPLICATE_TAIL_CER
}

fn char_edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (i, left_ch) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_ch) in right.iter().enumerate() {
            let substitution_cost = usize::from(left_ch != right_ch);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + substitution_cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn compact_transcript_for_duplicate_check(text: &str) -> String {
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

fn compact_transcript_contains_window(container: &str, window: &str) -> bool {
    const MIN_EXACT_WINDOW_CHARS: usize = 6;
    const MIN_FUZZY_WINDOW_CHARS: usize = 12;
    const MAX_FUZZY_WINDOW_CER: f64 = 0.12;

    let container = compact_transcript_for_duplicate_check(container);
    let window = compact_transcript_for_duplicate_check(window);
    let window_len = window.chars().count();
    if window_len < MIN_EXACT_WINDOW_CHARS {
        return false;
    }
    if container.contains(&window) {
        return true;
    }

    let container_chars: Vec<char> = container.chars().collect();
    if window_len < MIN_FUZZY_WINDOW_CHARS || container_chars.len() < window_len {
        return false;
    }

    let max_distance = ((window_len as f64) * MAX_FUZZY_WINDOW_CER).floor() as usize;
    if max_distance == 0 {
        return false;
    }
    for start in 0..=container_chars.len() - window_len {
        let candidate: String = container_chars[start..start + window_len].iter().collect();
        if char_edit_distance(&candidate, &window) <= max_distance {
            return true;
        }
    }
    false
}

pub(super) fn is_unstable_initial_partial(previous: &str, current: &str) -> bool {
    if !previous.trim().is_empty() {
        return false;
    }

    let compact = compact_transcript_for_duplicate_check(current);
    let char_count = compact.chars().count();
    if char_count == 0 {
        return true;
    }
    char_count <= 3
}

fn is_same_prefix_streaming_revision(previous: &str, current: &str) -> bool {
    const MIN_COMMON_PREFIX_CHARS: usize = 10;
    let previous_compact = compact_transcript_for_duplicate_check(previous);
    let current_compact = compact_transcript_for_duplicate_check(current);
    let previous_len = previous_compact.chars().count();
    let current_len = current_compact.chars().count();
    if current_len > previous_len && current_compact.starts_with(&previous_compact) {
        return true;
    }
    if previous_len < MIN_COMMON_PREFIX_CHARS || current_len < MIN_COMMON_PREFIX_CHARS {
        return false;
    }

    let common_prefix = common_prefix_char_count(&previous_compact, &current_compact);
    if common_prefix < MIN_COMMON_PREFIX_CHARS {
        return false;
    }

    if current_len + 4 >= previous_len {
        return true;
    }

    repeated_prefix_occurs(&previous_compact, &current_compact, MIN_COMMON_PREFIX_CHARS)
}

fn common_prefix_char_count(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn repeated_prefix_occurs(text: &str, source: &str, prefix_chars: usize) -> bool {
    let prefix: String = source.chars().take(prefix_chars).collect();
    if prefix.is_empty() {
        return false;
    }
    text.match_indices(&prefix).nth(1).is_some()
}

pub(super) fn merge_streaming_candidate(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    candidate: TranscriptCandidate,
) -> (String, Vec<TranscriptSegment>) {
    let candidate_text = candidate.text.trim().to_string();
    if candidate.timed_segments.is_empty() {
        return (
            merge_streaming_transcript(previous_text, &candidate_text),
            previous_segments.to_vec(),
        );
    }

    let previous_text = previous_text.trim();
    let mut incoming_segments = candidate.timed_segments;
    incoming_segments.sort_by_key(|segment| segment.start_ms);
    let old_segment_text = join_timed_segment_text(previous_segments);
    if let Some((merged_text, segments)) = merge_provider_segmentation_revision(
        previous_text,
        previous_segments,
        &incoming_segments,
        &candidate_text,
        &old_segment_text,
    ) {
        return (merged_text, segments);
    }

    let mut merged_segments = previous_segments.to_vec();
    let mut replaced_existing = false;
    for segment in incoming_segments {
        if let Some(index) = find_matching_timed_segment(&merged_segments, &segment) {
            replaced_existing = true;
            merged_segments[index] = merge_timed_segment(&merged_segments[index], &segment);
        } else {
            merged_segments.push(segment);
        }
    }
    merged_segments.sort_by_key(|segment| segment.start_ms);

    let new_segment_text = join_timed_segment_text(&merged_segments);
    if previous_text.is_empty() {
        return (
            choose_transcript_text(&candidate_text, &new_segment_text),
            merged_segments,
        );
    }

    if !old_segment_text.is_empty() {
        if let Some(index) = previous_text.find(&old_segment_text) {
            let mut merged_text = String::with_capacity(
                previous_text.len() - old_segment_text.len() + new_segment_text.len(),
            );
            merged_text.push_str(&previous_text[..index]);
            merged_text.push_str(&new_segment_text);
            merged_text.push_str(&previous_text[index + old_segment_text.len()..]);
            return (
                choose_transcript_text(&candidate_text, &merged_text),
                merged_segments,
            );
        }
    }

    if replaced_existing {
        return (
            choose_transcript_text(previous_text, &new_segment_text),
            merged_segments,
        );
    }

    let append_text = choose_transcript_text(&candidate_text, &new_segment_text);
    (
        merge_streaming_transcript(previous_text, &append_text),
        merged_segments,
    )
}

fn merge_provider_segmentation_revision(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    incoming_segments: &[TranscriptSegment],
    candidate_text: &str,
    old_segment_text: &str,
) -> Option<(String, Vec<TranscriptSegment>)> {
    if previous_text.is_empty()
        || incoming_segments.len() <= 1
        || candidate_text.chars().count() + 4 < previous_text.chars().count()
    {
        return None;
    }

    let splits_existing_segment = incoming_segments.iter().any(|incoming| {
        find_matching_timed_segment(previous_segments, incoming)
            .and_then(|index| previous_segments.get(index))
            .is_some_and(|existing| {
                existing.text.starts_with(&incoming.text)
                    && existing.text.chars().count() > incoming.text.chars().count() + 4
            })
    });
    if !splits_existing_segment {
        return None;
    }

    let new_segment_text = join_timed_segment_text(incoming_segments);
    let merged_text = if !old_segment_text.is_empty() {
        if let Some(index) = previous_text.find(old_segment_text) {
            let mut merged = String::with_capacity(
                previous_text.len() - old_segment_text.len() + new_segment_text.len(),
            );
            merged.push_str(&previous_text[..index]);
            merged.push_str(&new_segment_text);
            merged.push_str(&previous_text[index + old_segment_text.len()..]);
            merged
        } else {
            merge_streaming_transcript(previous_text, candidate_text)
        }
    } else {
        merge_streaming_transcript(previous_text, candidate_text)
    };

    Some((
        choose_transcript_text(candidate_text, &merged_text),
        incoming_segments.to_vec(),
    ))
}

fn find_matching_timed_segment(
    segments: &[TranscriptSegment],
    incoming: &TranscriptSegment,
) -> Option<usize> {
    const START_TIME_TOLERANCE_MS: i64 = 240;
    segments
        .iter()
        .position(|segment| (segment.start_ms - incoming.start_ms).abs() <= START_TIME_TOLERANCE_MS)
}

fn merge_timed_segment(
    existing: &TranscriptSegment,
    incoming: &TranscriptSegment,
) -> TranscriptSegment {
    let text = choose_transcript_text(&existing.text, &incoming.text);
    TranscriptSegment {
        start_ms: existing.start_ms.min(incoming.start_ms),
        end_ms: match (existing.end_ms, incoming.end_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        },
        text,
    }
}

fn join_timed_segment_text(segments: &[TranscriptSegment]) -> String {
    segments.iter().fold(String::new(), |merged, segment| {
        merge_streaming_transcript(&merged, &segment.text)
    })
}

fn join_transcript_segments(previous: &str, current: &str) -> String {
    let needs_space = previous
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && current
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if needs_space {
        format!("{previous} {current}")
    } else {
        format!("{previous}{current}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transcript_text_prefers_accumulated_result_when_utterances_are_suffix() {
        let result = json!({
        "text": "先导出数据。再导入数据库。最后检查汇总结果。",
        "utterances": [
        { "text": "最后检查汇总结果。" }
        ]
        });

        assert_eq!(
            transcript_text_from_result(&result),
            "先导出数据。再导入数据库。最后检查汇总结果。"
        );
    }

    #[test]
    fn transcript_text_uses_utterances_when_result_text_is_empty() {
        let result = json!({
        "text": "",
        "utterances": [
        { "text": "蓝牙音频" },
        { "text": "正在识别。" }
        ]
        });

        assert_eq!(transcript_text_from_result(&result), "蓝牙音频正在识别。");
    }

    #[test]
    fn transcript_text_uses_longer_utterances_when_they_have_full_context() {
        let result = json!({
        "text": "蓝牙音频",
        "utterances": [
        { "text": "蓝牙音频" },
        { "text": "正在发送到火山识别。" }
        ]
        });

        assert_eq!(
            transcript_text_from_result(&result),
            "蓝牙音频正在发送到火山识别。"
        );
    }

    #[test]
    fn transcript_candidate_parses_timed_utterances() {
        let result = json!({
        "text": "",
        "utterances": [
        { "text": "第一句。", "start_time": 0, "end_time": 900 },
        { "text": "第二句。", "start_time": "1200", "end_time": "2100" }
        ]
        });

        let candidate = transcript_candidate_from_result(&result);

        assert_eq!(candidate.text, "第一句。第二句。");
        assert_eq!(
            candidate.timed_segments,
            vec![
                TranscriptSegment {
                    start_ms: 0,
                    end_ms: Some(900),
                    text: "第一句。".into()
                },
                TranscriptSegment {
                    start_ms: 1200,
                    end_ms: Some(2100),
                    text: "第二句。".into()
                }
            ]
        );
    }

    #[test]
    fn transcript_candidate_prefers_word_timing_when_utterance_start_is_stale() {
        let result = json!({
        "text": "需要分开判断，测试报告里要记录蓝牙包数。",
        "utterances": [{
        "text": "需要分开判断，测试报告里要记录蓝牙包数。",
        "start_time": 1452,
        "end_time": 10402,
        "words": [
        { "text": "需要", "start_time": 6372, "end_time": 6452 },
        { "text": "包数", "start_time": 10100, "end_time": 10402 }
        ]
        }]
        });

        let candidate = transcript_candidate_from_result(&result);

        assert_eq!(candidate.timed_segments[0].start_ms, 6372);
        assert_eq!(candidate.timed_segments[0].end_ms, Some(10402));
    }

    #[test]
    fn merge_streaming_transcript_keeps_prefix_after_provider_reset() {
        assert_eq!(
            merge_streaming_transcript(
                "关闭所有打开的窗口，然后重启电脑，帮我查一下从北京到上海",
                "帮我查一下从北京到上海的高铁票最早的几班",
            ),
            "关闭所有打开的窗口，然后重启电脑，帮我查一下从北京到上海的高铁票最早的几班"
        );
    }

    #[test]
    fn merge_streaming_transcript_appends_disjoint_continuation_segments() {
        assert_eq!(
            merge_streaming_transcript(
                "前端胶囊只显示",
                "不能承载整段文字，最后把异常现象整理成清晰的结论。"
            ),
            "前端胶囊只显示不能承载整段文字，最后把异常现象整理成清晰的结论。"
        );
        assert_eq!(
            merge_streaming_transcript("已经包含后续内容", "后续"),
            "已经包含后续内容"
        );
        assert_eq!(
            merge_streaming_transcript("已经有稳定正文", "哎呀"),
            "已经有稳定正文"
        );
    }

    #[test]
    fn merge_streaming_transcript_ignores_restarted_middle_window() {
        let stable = "这段录音正在验证长时间语音输入，如果某一步失败就先修最基础的链路，再继续往后跑。后端需要把最终结果稳定地交给系统输入链路，最后把异常现象整理成清晰的结论。";
        let restarted_window = "如果某一步失败，就先修最基础的链路，再继续往后跑，后端需要把最终结果稳定的交给系统输入链路，最后把。";

        assert_eq!(merge_streaming_transcript(stable, restarted_window), stable);
    }

    #[test]
    fn merge_streaming_transcript_replaces_same_prefix_revision() {
        assert_eq!(
            merge_streaming_transcript(
                "现在开始进行一段完整的产品链路，长亭",
                "现在开始进行一段完整的产品链路，长听写",
            ),
            "现在开始进行一段完整的产品链路，长听写"
        );
    }

    #[test]
    fn merge_streaming_transcript_replaces_short_prefix_revision() {
        assert_eq!(
            merge_streaming_transcript("请。", "请把这段长录音完整转换成文字。"),
            "请把这段长录音完整转换成文字。"
        );
    }

    #[test]
    fn unstable_initial_partial_filters_short_interference_only_before_context() {
        assert!(is_unstable_initial_partial("", "哎呀"));
        assert!(is_unstable_initial_partial("", "嗯"));
        assert!(!is_unstable_initial_partial("", "蓝牙听写测试现在开始"));
        assert!(!is_unstable_initial_partial("已经有正文", "哎呀"));
    }

    #[test]
    fn merge_streaming_transcript_replaces_duplicate_with_clean_revision() {
        assert_eq!(
    merge_streaming_transcript(
    "现在开始进行一段完整的产品链路，长亭现在开始进行一段完整的产品链路，长听写现在开始进行一段完整的产品链路长听写测试报告",
    "现在开始进行一段完整的产品链路长听写，测试报告里要记录蓝牙包数和识别准确率。",
    ),
    "现在开始进行一段完整的产品链路长听写，测试报告里要记录蓝牙包数和识别准确率。"
    );
    }

    #[test]
    fn merge_streaming_transcript_ignores_duplicate_prefix_before_stable_full_text() {
        let stable = "我正在复盘上午的调试过程和后续安排，如果声音偏小，也要尽量保持句子结构清楚，测试报告里要记录蓝牙高速和识别准确率，最后继续执行下一项自动化回归。";
        let duplicate_prefix = "我正在复盘上午的调试过程和后续安排如果声音偏小也要尽量保持句子";

        assert_eq!(
            merge_streaming_transcript(stable, &format!("{duplicate_prefix}{stable}")),
            stable
        );
        assert_eq!(
            choose_transcript_text(&format!("{duplicate_prefix}{stable}"), stable),
            stable
        );
    }

    #[test]
    fn choose_transcript_text_prefers_final_over_duplicate_tail_revision() {
        let final_text = "现在开始进行一段完整的产品链路长听写，在观察识别完成后文字是否立即进入当前光标，遇到语速变快时需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论。";
        let duplicate_tail = "再观察识别完成后文字是否立即进入当前光标，遇到语速变快时，需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论";
        let duplicated = format!("{final_text}{duplicate_tail}");

        assert_eq!(choose_transcript_text(final_text, &duplicated), final_text);
    }

    #[test]
    fn merge_streaming_candidate_prefers_final_full_text_over_old_tail_segment() {
        let previous_tail = "再观察识别完成后文字是否立即进入当前光标，遇到语速变快时，需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论";
        let final_text = "现在开始进行一段完整的产品链路长听写，在观察识别完成后文字是否立即进入当前光标，遇到语速变快时需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论。";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 4840,
            end_ms: Some(14600),
            text: previous_tail.into(),
        }];
        let candidate = TranscriptCandidate {
            text: final_text.into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 720,
                end_ms: Some(15132),
                text: final_text.into(),
            }],
        };

        let (merged, _segments) =
            merge_streaming_candidate(previous_tail, &previous_segments, candidate);

        assert_eq!(merged, final_text);
    }

    #[test]
    fn merge_streaming_candidate_replaces_repeated_timed_suffix_instead_of_appending() {
        let previous_segments = vec![TranscriptSegment {
            start_ms: 7602,
            end_ms: Some(10300),
            text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。最后".into(),
        }];
        let candidate = TranscriptCandidate {
    text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。".into(),
    timed_segments: vec![TranscriptSegment {
    start_ms: 7602,
    end_ms: Some(14500),
    text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。".into(),
    }],
    };

        let (merged, segments) = merge_streaming_candidate(
            "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。最后",
            &previous_segments,
            candidate,
        );

        assert_eq!(
    merged,
    "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。"
    );
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn merge_streaming_candidate_preserves_prefix_when_timed_suffix_grows() {
        let previous_text = "这段长录音用来验证真实会议记录的输入体验，先确认胶囊里可以实时看到稳定的预览内容，同时检查历史记录。";
        let old_suffix = "看到稳定的预览内容，同时检查历史记录。";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 6200,
            end_ms: Some(9000),
            text: old_suffix.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 6200,
                end_ms: Some(11400),
                text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。".into(),
            }],
        };

        let (merged, _segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
    merged,
    "这段长录音用来验证真实会议记录的输入体验，先确认胶囊里可以实时看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。"
    );
    }

    #[test]
    fn merge_streaming_candidate_replaces_provider_split_without_duplication() {
        let previous_text = "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述备忘";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 912,
            end_ms: Some(8172),
            text: previous_text.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述、备忘和会议纪要"
                .into(),
            timed_segments: vec![
                TranscriptSegment {
                    start_ms: 912,
                    end_ms: Some(4192),
                    text: "我正在复盘上午的调试过程和后续安排。".into(),
                },
                TranscriptSegment {
                    start_ms: 5200,
                    end_ms: Some(9000),
                    text: "这类场景更接近日常口述、备忘和会议纪要".into(),
                },
            ],
        };

        let (merged, segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
            merged,
            "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述、备忘和会议纪要"
        );
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn merge_streaming_candidate_appends_disjoint_timed_segments_once() {
        let previous_segments = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: Some(900),
            text: "第一句。".into(),
        }];
        let candidate = TranscriptCandidate {
            text: "第二句。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 1300,
                end_ms: Some(2100),
                text: "第二句。".into(),
            }],
        };

        let (merged, segments) =
            merge_streaming_candidate("第一句。", &previous_segments, candidate);

        assert_eq!(merged, "第一句。第二句。");
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn merge_streaming_candidate_preserves_prefix_when_provider_streams_tail_segment() {
        let previous_text =
            "现在开始进行一段完整的产品链路，长听写短句回归和长段落回归需要分开判断";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 1812,
            end_ms: Some(7402),
            text: previous_text.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "需要分开判断，测试报告里要记录蓝牙包数和识别准确率".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 6372,
                end_ms: Some(10602),
                text: "需要分开判断，测试报告里要记录蓝牙包数和识别准确率".into(),
            }],
        };

        let (merged, segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
    merged,
    "现在开始进行一段完整的产品链路，长听写短句回归和长段落回归需要分开判断，测试报告里要记录蓝牙包数和识别准确率"
    );
        assert_eq!(segments.len(), 2);
    }

    // --- 2.2 table tests: punctuation, empty text, edge cases ---

    #[test]
    fn choose_transcript_text_tolerates_punctuation_differences() {
        // result_text has trailing period, utterance_text does not — should pick the longer
        // or the one with punctuation (not crash, not drop text).
        let result = choose_transcript_text("测试标点差异", "测试标点差异。");
        assert_eq!(result, "测试标点差异。", "should prefer punctuated version");

        // Both same content, different punctuation style
        let result = choose_transcript_text("你好世界！", "你好世界！");
        assert_eq!(result, "你好世界！");
    }

    #[test]
    fn choose_transcript_text_both_empty() {
        assert_eq!(choose_transcript_text("", ""), "");
    }

    #[test]
    fn choose_transcript_text_one_empty() {
        assert_eq!(choose_transcript_text("", "只有utterance"), "只有utterance");
        assert_eq!(choose_transcript_text("只有result", ""), "只有result");
    }

    #[test]
    fn merge_streaming_candidate_with_empty_previous() {
        let candidate = TranscriptCandidate {
            text: "第一句话。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 0,
                end_ms: Some(800),
                text: "第一句话。".into(),
            }],
        };
        let (merged, segments) = merge_streaming_candidate("", &[], candidate);
        assert_eq!(merged, "第一句话。");
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn merge_streaming_candidate_with_empty_candidate_text() {
        let candidate = TranscriptCandidate {
            text: "".into(),
            timed_segments: vec![],
        };
        let (merged, _segments) = merge_streaming_candidate("已有内容", &[], candidate);
        assert_eq!(
            merged, "已有内容",
            "empty candidate should preserve previous"
        );
    }

    #[test]
    fn merge_streaming_transcript_both_empty() {
        assert_eq!(merge_streaming_transcript("", ""), "");
    }

    #[test]
    fn merge_streaming_transcript_single_char_previous() {
        // Single char + longer current: should not crash on overlap logic
        let result = merge_streaming_transcript("好", "好的开始");
        assert!(!result.is_empty());
    }

    #[test]
    fn transcript_candidate_handles_result_as_array() {
        // normalized_result should unwrap array-of-one
        let result = json!({
            "result": [{ "text": "数组里的结果", "utterances": [] }]
        });
        let candidate = transcript_candidate_from_result(
            result
                .get("result")
                .and_then(|v| v.as_array())
                .unwrap()
                .first()
                .unwrap(),
        );
        assert_eq!(candidate.text, "数组里的结果");
    }

    #[test]
    fn transcript_candidate_empty_json_returns_empty() {
        let result = json!({});
        let candidate = transcript_candidate_from_result(&result);
        assert_eq!(candidate.text, "");
        assert!(candidate.timed_segments.is_empty());
    }

    #[test]
    fn merge_streaming_transcript_punctuation_only_difference_is_not_duplicate() {
        // "你好世界" vs "你好世界。" — not a duplicate prefix, should merge sensibly
        let result = merge_streaming_transcript("你好世界", "你好世界。");
        assert!(result.contains("你好世界"), "should contain the base text");
    }
}
