//! Content-only, append-only corroboration at the shared final boundary.
//!
//! Anonymous channels are not owner identities. This module owns no clock,
//! preview, provider or editor state. Failure always preserves the primary.
use pinyin::ToPinyin;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub(crate) const MAX_CONTENT_SAMPLES: usize = 12 * 16_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AnonymousContentSegment {
    pub channel: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AnonymousActivity {
    pub channel: Option<u32>,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ContentSupplement {
    pub locale: String,
    pub source_pcm_sha256: String,
    pub decoded_pcm_sha256: String,
    pub input_samples: usize,
    /// Set only after checking the worker request, complete derived PCM and
    /// the still-current verified capture reference. Not a speaker permission.
    pub derivation_bound: bool,
    pub auxiliary_cloud_text: String,
    pub segments: Vec<AnonymousContentSegment>,
    pub activity: Vec<AnonymousActivity>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContentDecision {
    pub text: String,
    pub appended_channels: Vec<u32>,
    pub reason: &'static str,
}

fn units(text: &str) -> (Vec<String>, Vec<(usize, usize)>) {
    let mut tokens = Vec::new();
    let mut positions = Vec::new();
    for (start, ch) in text.char_indices().filter(|(_, ch)| ch.is_alphanumeric()) {
        let token = ch
            .to_pinyin()
            .map(|pinyin| {
                let mut value = pinyin.with_tone_num_end().to_string();
                if !value.as_bytes().last().is_some_and(u8::is_ascii_digit) {
                    value.push('5');
                }
                value
            })
            .unwrap_or_else(|| ch.to_lowercase().collect());
        tokens.push(token);
        positions.push((start, start + ch.len_utf8()));
    }
    (tokens, positions)
}

fn find_units(haystack: &[String], needle: &[String]) -> Option<usize> {
    (!needle.is_empty() && needle.len() <= haystack.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

fn lcs_size(first: &[String], second: &[String]) -> usize {
    let mut previous = vec![0; second.len() + 1];
    for token in first {
        let mut current = vec![0; second.len() + 1];
        for (index, other) in second.iter().enumerate() {
            current[index + 1] = if token == other {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        previous = current;
    }
    previous[second.len()]
}

pub(crate) fn append_consensus(primary: &str, supplement: &ContentSupplement) -> ContentDecision {
    let mut decision = ContentDecision {
        text: primary.to_string(),
        appended_channels: Vec::new(),
        reason: "no_corroborated_missing_content",
    };
    macro_rules! skip {
        ($reason:literal) => {{
            decision.reason = $reason;
            return decision;
        }};
    }
    if supplement.locale != "zh-CN"
        || !supplement.derivation_bound
        || supplement.source_pcm_sha256.len() != 64
        || supplement.source_pcm_sha256 != supplement.decoded_pcm_sha256
        || !(513..=MAX_CONTENT_SAMPLES).contains(&supplement.input_samples)
    {
        skip!("unsupported_locale_or_missing_same_capture_binding");
    }
    if primary.len() > 32_768
        || supplement.auxiliary_cloud_text.len() > 32_768
        || supplement.segments.len() > 32
        || supplement.activity.len() > 4096
    {
        skip!("unbounded_transcript");
    }
    let (primary_units, _) = units(primary);
    let (shadow_units, shadow_positions) = units(&supplement.auxiliary_cloud_text);
    if primary_units.is_empty()
        || shadow_units.is_empty()
        || primary_units.len().max(shadow_units.len()) > 2048
    {
        skip!("empty_or_unbounded_transcript");
    }
    if primary_units == shadow_units {
        skip!("already_present");
    }
    let mut texts = BTreeMap::new();
    let mut total_units = 0;
    for segment in &supplement.segments {
        let Some(channel) = segment.channel else {
            skip!("anonymous_channel_missing");
        };
        if segment.text.len() > 32_768 {
            skip!("unbounded_auxiliary_transcript");
        }
        let words = units(&segment.text).0;
        total_units += words.len();
        if total_units > 4096 {
            skip!("unbounded_auxiliary_transcript");
        }
        if texts.insert(channel, words).is_some() {
            skip!("no_independent_single_segment_channel");
        }
    }
    if texts.len() < 2 {
        skip!("no_independent_single_segment_channel");
    }
    let mut ranked: Vec<_> = texts
        .iter()
        .map(|(channel, words)| (lcs_size(&primary_units, words), *channel))
        .collect();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    if ranked[0].0 == 0 || ranked[0].0 == ranked[1].0 {
        skip!("primary_channel_ambiguous");
    }
    let primary_channel = ranked[0].1;
    let mut activity: BTreeMap<u32, Vec<(i64, i64)>> = BTreeMap::new();
    for span in &supplement.activity {
        let Some(channel) = span.channel else {
            skip!("anonymous_channel_missing");
        };
        if span.start_ms < 0
            || span.end_ms <= span.start_ms
            || span.end_ms as u64 * 16 > supplement.input_samples as u64
        {
            skip!("out_of_source_activity_clock");
        }
        activity
            .entry(channel)
            .or_default()
            .push((span.start_ms, span.end_ms));
    }
    for (channel, words) in texts {
        if channel == primary_channel
            || words.is_empty()
            || find_units(&primary_units, &words).is_some()
        {
            continue;
        }
        let overlaps = activity.get(&primary_channel).is_some_and(|first| {
            activity.get(&channel).is_some_and(|second| {
                first
                    .iter()
                    .any(|(a, b)| second.iter().any(|(c, d)| (*a).max(*c) < (*b).min(*d)))
            })
        });
        if !overlaps {
            continue;
        }
        let Some(position) = find_units(&shadow_units, &words) else {
            continue;
        };
        let existing = units(&decision.text).0;
        if find_units(&existing, &words).is_some() {
            continue;
        }
        let covered = (1..=existing.len().min(words.len()))
            .rev()
            .find(|size| existing[existing.len() - size..] == words[..*size])
            .unwrap_or(0);
        if covered == words.len() {
            continue;
        }
        // A channel can concatenate an already-transcribed opening with a
        // later missing clause. Its internal primary occurrence supplies no
        // certified seam. Do not duplicate that opening at the final cursor.
        if covered == 0 && find_units(&existing, &words[..1]).is_some() {
            continue;
        }
        let start = shadow_positions[position + covered].0;
        let end = shadow_positions[position + words.len() - 1].1;
        if !decision
            .text
            .chars()
            .last()
            .is_some_and(char::is_whitespace)
        {
            decision.text.push(' ');
        }
        decision
            .text
            .push_str(&supplement.auxiliary_cloud_text[start..end]);
        decision.appended_channels.push(channel);
    }
    if !decision.appended_channels.is_empty() {
        decision.reason = "independent_content_consensus";
    }
    debug_assert!(decision.text.starts_with(primary));
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence() -> ContentSupplement {
        ContentSupplement {
            locale: "zh-CN".into(),
            source_pcm_sha256: "f".repeat(64),
            decoded_pcm_sha256: "f".repeat(64),
            input_samples: 16000,
            derivation_bound: true,
            auxiliary_cloud_text: "明天回家。".into(),
            segments: vec![
                AnonymousContentSegment {
                    channel: Some(1),
                    text: "今天下雨".into(),
                },
                AnonymousContentSegment {
                    channel: Some(2),
                    text: "明天回家".into(),
                },
            ],
            activity: vec![
                AnonymousActivity {
                    channel: Some(1),
                    start_ms: 0,
                    end_ms: 900,
                },
                AnonymousActivity {
                    channel: Some(2),
                    start_ms: 200,
                    end_ms: 700,
                },
            ],
        }
    }
    #[test]
    fn corroborated_overlap_appends_without_changing_any_primary_byte() {
        let e = evidence();
        assert_eq!(
            append_consensus("今天下雨。", &e).text,
            "今天下雨。 明天回家"
        );
        assert_eq!(
            append_consensus("今天下雨。明天", &e).text,
            "今天下雨。明天 回家"
        );
        let mut corrected = e.clone();
        corrected.auxiliary_cloud_text = "今天夏雨。明天回家".into();
        assert_eq!(
            append_consensus("今天下雨。", &corrected).text,
            "今天下雨。 明天回家"
        );
    }
    #[test]
    fn repetition_counts_and_same_tone_homophones_are_preserved() {
        let mut e = evidence();
        e.segments[0].text = "甲方说开车".into();
        e.segments[1].text = "开车开车".into();
        e.auxiliary_cloud_text = "开车开车".into();
        assert_eq!(append_consensus("甲方说开车", &e).text, "甲方说开车 开车");
        e.segments[0].text = "明天回家".into();
        e.segments[1].text = "晚晚香".into();
        e.auxiliary_cloud_text = "明天回家晚晚香".into();
        assert_eq!(
            append_consensus("明天回家婉婉香", &e).text,
            "明天回家婉婉香"
        );
    }
    #[test]
    fn locale_derivation_and_capture_mismatch_preserve_primary() {
        for change in 0..4 {
            let mut e = evidence();
            match change {
                0 => e.locale = "auto".into(),
                1 => e.derivation_bound = false,
                2 => e.decoded_pcm_sha256 = "e".repeat(64),
                _ => e.input_samples = MAX_CONTENT_SAMPLES + 1,
            }
            assert_eq!(append_consensus("今天下雨", &e).text, "今天下雨");
        }
    }
    #[test]
    fn missing_disjoint_or_out_of_bounds_activity_is_not_overlap() {
        for change in 0..3 {
            let mut e = evidence();
            match change {
                0 => e.activity.clear(),
                1 => e.activity[0].end_ms = 200,
                _ => e.activity[0].end_ms = 1500,
            }
            assert_eq!(append_consensus("今天下雨", &e).text, "今天下雨");
        }
    }
    #[test]
    fn disagreement_tone_change_and_single_voice_correction_do_not_append() {
        let mut e = evidence();
        e.auxiliary_cloud_text = "明天开车".into();
        assert_eq!(append_consensus("今天下雨", &e).text, "今天下雨");
        e.auxiliary_cloud_text = "明天回家".into();
        e.segments[1].text = "明田回家".into();
        assert_eq!(append_consensus("今天下雨", &e).text, "今天下雨");
        e.segments.truncate(1);
        e.segments[0].text = "今天夏雨".into();
        e.auxiliary_cloud_text = "今天夏雨".into();
        assert_eq!(append_consensus("今天下雨。", &e).text, "今天下雨。");
    }
    #[test]
    fn ambiguous_channels_missing_channels_multiple_rows_and_oversize_keep_primary() {
        for change in 0..4 {
            let mut e = evidence();
            match change {
                0 => e.segments[1].text = e.segments[0].text.clone(),
                1 => e.segments[1].channel = None,
                2 => e.segments.push(e.segments[1].clone()),
                _ => e.segments[1].text = "明".repeat(4100),
            }
            assert_eq!(append_consensus("今天下雨", &e).text, "今天下雨");
        }
    }
    #[test]
    fn equivalent_text_keeps_primary_punctuation() {
        let mut e = evidence();
        e.auxiliary_cloud_text = "今天下雨".into();
        assert_eq!(append_consensus("今天下雨。", &e).text, "今天下雨。");
    }
    #[test]
    fn internal_covered_opening_cannot_be_appended_as_a_second_occurrence() {
        let mut e = evidence();
        e.segments[0].text = "今天下雨".into();
        e.segments[1].text = "明天回家".into();
        assert_eq!(
            append_consensus("明天再说。今天下雨", &e).text,
            "明天再说。今天下雨"
        );
    }

    #[test]
    fn six_frozen_public_provider_controls_match_the_qualified_prototype() {
        let controls: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/content-consensus-20261006.json")).unwrap();
        assert_eq!(controls["cases"].as_array().unwrap().len(), 6);
        for row in controls["cases"].as_array().unwrap() {
            let evidence: ContentSupplement =
                serde_json::from_value(row["supplement"].clone()).unwrap();
            let primary = row["primary"].as_str().unwrap();
            let decision = append_consensus(primary, &evidence);
            assert_eq!(
                decision.text,
                row["expected"].as_str().unwrap(),
                "{}",
                row["id"]
            );
            assert!(decision.text.starts_with(primary));
        }
    }
}
