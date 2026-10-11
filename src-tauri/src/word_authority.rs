//! Shared live/final permission for words in one captured audio occurrence.
//! Provider speaker ids and recognition stability never create these proofs.

use crate::transcript_boundary::{consumed_prefix_end, content_key, TimedTranscript};
use std::time::Instant;

#[path = "word_authority_source.rs"]
mod captured_source;
pub(crate) use captured_source::{CapturedWordAuthority, SourceWordBinding};

#[cfg(test)]
#[path = "word_authority_recorded_source.rs"]
mod recorded_source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordIdentity {
    Owner,
    Other,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AudioSource {
    pub stream: u64,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct IdentityCell {
    /// This cell, not the complete model context, supplies text permission.
    pub range: (u64, u64),
    pub context: (u64, u64),
    pub identity: WordIdentity,
    pub observed_at: Instant,
    pub speech_end_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct WordAuthority {
    pub source: AudioSource,
    pub cells: Vec<IdentityCell>,
    pub complete: bool,
    /// Both identity and its required neighboring observations are resolved.
    /// A processed PCM counter alone cannot advance this permission boundary.
    pub settled_through_ms: u64,
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    pub timing: TimedTranscript,
    pub unresolved: bool,
    pub excluded_other: bool,
    pub updated_at: Option<Instant>,
}

#[derive(Clone, Copy)]
struct WordDecision {
    identity: WordIdentity,
    observed_at: Option<Instant>,
}

impl WordDecision {
    fn pending() -> Self {
        Self {
            identity: WordIdentity::Pending,
            observed_at: None,
        }
    }
}

impl WordAuthority {
    pub(crate) fn new(source: AudioSource) -> Self {
        Self {
            source,
            cells: Vec::new(),
            complete: false,
            settled_through_ms: 0,
            unavailable: None,
        }
    }

    pub(crate) fn identity(&self, source: AudioSource, start: u64, end: u64) -> WordIdentity {
        if source != self.source
            || start >= end
            || end > self.settled_through_ms
            || self.unavailable.is_some()
        {
            return WordIdentity::Pending;
        }
        let midpoint = start + (end - start) / 2;
        // Confirmed foreign overlap vetoes a new word. A nearby cell cannot
        // lend its owner label across an unobserved midpoint.
        if self.cells.iter().any(|cell| {
            cell.identity == WordIdentity::Other && cell.range.0 < end && start < cell.range.1
        }) {
            return WordIdentity::Other;
        }
        self.cells
            .iter()
            .find(|cell| cell.range.0 <= midpoint && midpoint < cell.range.1)
            .map_or(WordIdentity::Pending, |cell| cell.identity)
    }

    pub(crate) fn project(
        &self,
        source: AudioSource,
        timing: &TimedTranscript,
        visual: bool,
    ) -> Projection {
        project_timing(timing, visual, |start, end| {
            let identity = self.identity(source, start, end);
            let midpoint = start + (end - start) / 2;
            let observed_at = (identity == WordIdentity::Owner)
                .then(|| {
                    self.cells
                        .iter()
                        .find(|cell| cell.range.0 <= midpoint && midpoint < cell.range.1)
                        .map(|cell| cell.observed_at)
                })
                .flatten();
            WordDecision {
                identity,
                observed_at,
            }
        })
    }
}

/// The same text/timing projection for legacy diagnostic cells and exact
/// captured-source certificates. Pending is reversible only; live commit and
/// final insertion share the irreversible path. Evidence time is not an editor
/// delivery receipt and cannot renew the3-second endpoint window.
fn project_timing(
    timing: &TimedTranscript,
    visual: bool,
    mut decide: impl FnMut(u64, u64) -> WordDecision,
) -> Projection {
    let key = content_key(&timing.text);
    let mut result = Projection {
        timing: TimedTranscript::default(),
        unresolved: false,
        excluded_other: false,
        updated_at: None,
    };
    let mut offset = 0;
    let mut previous = (0, 0);
    for word in &timing.words {
        let word_key = content_key(&word.text);
        if word_key.is_empty() {
            continue;
        }
        let Some(remaining) = key.get(offset..) else {
            result.unresolved = true;
            break;
        };
        if !remaining.starts_with(&word_key) {
            result.unresolved = true;
            break;
        }
        let through = offset + word_key.len();
        let (Some(start), Some(end)) = (word.start_ms, word.end_ms) else {
            result.unresolved = true;
            break;
        };
        if start >= end || start < previous.0 || end < previous.1 {
            result.unresolved = true;
            break;
        }
        previous = (start, end);
        let decision = decide(start, end);
        if decision.identity == WordIdentity::Pending {
            result.unresolved = true;
            if !visual {
                break;
            }
        }
        if decision.identity == WordIdentity::Other {
            result.excluded_other = true;
        } else {
            let from_byte = consumed_prefix_end(&timing.text, &key[..offset]).unwrap_or(0);
            let through_byte =
                consumed_prefix_end(&timing.text, &key[..through]).unwrap_or(timing.text.len());
            let rendered = &timing.text[from_byte..through_byte];
            result.timing.text.push_str(rendered);
            let mut retained = word.clone();
            retained.text = rendered.to_string();
            result.timing.words.push(retained);
        }
        if let Some(at) = decision.observed_at {
            result.updated_at = Some(result.updated_at.map_or(at, |previous| previous.max(at)));
        }
        offset = through;
    }
    result.unresolved |= offset < key.len();
    if visual && result.unresolved && offset < key.len() {
        let from = consumed_prefix_end(&timing.text, &key[..offset]).unwrap_or(0);
        result.timing.text.push_str(&timing.text[from..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript_boundary::TimedTranscriptWord;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn source() -> AudioSource {
        AudioSource {
            stream: 1,
            generation: 1,
        }
    }
    fn timing() -> TimedTranscript {
        TimedTranscript {
            text: "本人前句。白菜长得水灵。后边查询。".into(),
            words: vec![
                TimedTranscriptWord {
                    text: "本人前句".into(),
                    start_ms: Some(43000),
                    end_ms: Some(45000),
                },
                TimedTranscriptWord {
                    text: "白菜长得水灵".into(),
                    start_ms: Some(46022),
                    end_ms: Some(46782),
                },
                TimedTranscriptWord {
                    text: "后边查询".into(),
                    start_ms: Some(48000),
                    end_ms: Some(51000),
                },
            ],
        }
    }
    fn cell(range: (u64, u64), identity: WordIdentity) -> IdentityCell {
        IdentityCell {
            range,
            context: range,
            identity,
            observed_at: Instant::now(),
            speech_end_ms: Some(range.1),
        }
    }

    fn timed(text: &str, words: &[(&str, u64, u64)]) -> TimedTranscript {
        TimedTranscript {
            text: text.into(),
            words: words
                .iter()
                .map(|&(text, start_ms, end_ms)| TimedTranscriptWord {
                    text: text.into(),
                    start_ms: Some(start_ms),
                    end_ms: Some(end_ms),
                })
                .collect(),
        }
    }

    #[test]
    fn background_cannot_enter_live_or_final_before_or_after_analysis() {
        let mut ledger = WordAuthority::new(source());
        ledger.cells.push(cell((42000, 45500), WordIdentity::Owner));
        ledger.settled_through_ms = 45500;
        assert_eq!(
            ledger.project(source(), &timing(), false).timing.text,
            "本人前句。"
        );
        assert!(ledger.project(source(), &timing(), false).unresolved);
        assert_eq!(
            ledger.project(source(), &timing(), true).timing.text,
            timing().text
        );
        ledger.cells.push(cell((46000, 46800), WordIdentity::Other));
        ledger.cells.push(cell((48000, 51500), WordIdentity::Owner));
        ledger.complete = true;
        ledger.settled_through_ms = 51500;
        let final_view = ledger.project(source(), &timing(), false);
        assert_eq!(final_view.timing.text, "本人前句。后边查询。");
        assert!(!final_view.unresolved);
        assert_eq!(
            ledger.project(source(), &timing(), true).timing.text,
            final_view.timing.text
        );
    }
    #[test]
    fn same_text_on_another_capture_and_unavailable_identity_remain_pending() {
        let mut ledger = WordAuthority::new(source());
        ledger.cells.push(cell((0, 60000), WordIdentity::Owner));
        ledger.settled_through_ms = 60000;
        assert_eq!(
            ledger
                .project(
                    AudioSource {
                        stream: 1,
                        generation: 2
                    },
                    &timing(),
                    false
                )
                .timing
                .text,
            ""
        );
        ledger.unavailable = Some("model failure".into());
        assert_eq!(ledger.project(source(), &timing(), false).timing.text, "");
        assert!(ledger.project(source(), &timing(), true).unresolved);
    }
    #[test]
    fn missing_coordinates_and_new_occurrences_cannot_borrow_an_old_owner_prefix() {
        let mut ledger = WordAuthority::new(source());
        ledger.cells.push(cell((42000, 45500), WordIdentity::Owner));
        ledger.settled_through_ms = 45500;
        let mut revised = timing();
        revised.words[1].start_ms = None;
        assert_eq!(
            ledger.project(source(), &revised, false).timing.text,
            "本人前句。"
        );
        revised.words[0].start_ms = Some(65000);
        revised.words[0].end_ms = Some(66000);
        assert!(ledger.project(source(), &revised, false).unresolved);
        assert_eq!(ledger.project(source(), &revised, false).timing.text, "");
    }

    #[test]
    fn foreign_overlap_inside_its_first_token_vetoes_the_whole_middle_and_owner_returns() {
        let words = [
            ("本人前句", 43_000, 45_900),
            ("白", 46_022, 46_142),
            ("菜", 46_142, 46_262),
            ("长", 46_302, 46_422),
            ("得", 46_422, 46_502),
            ("水", 46_542, 46_702),
            ("灵", 46_702, 46_782),
            ("本人返回", 47_621, 51_021),
            ("本人尾字", 59_541, 60_781),
        ];
        let timing = timed("本人前句。白菜长得水灵。本人返回。本人尾字。", &words);
        let mut authority = WordAuthority::new(source());
        authority.cells = vec![
            cell((42_000, 45_960), WordIdentity::Owner),
            // The foreign proof begins 17 ms inside the first provider token.
            // Permission is occurrence based: any overlap vetoes the complete
            // indivisible token, without a majority-coverage exception.
            cell((46_039, 47_420), WordIdentity::Other),
            cell((47_600, 51_100), WordIdentity::Owner),
            cell((59_500, 60_900), WordIdentity::Owner),
        ];
        authority.complete = true;
        authority.settled_through_ms = 60_900;

        for &(text, start, end) in &words[1..7] {
            assert_eq!(
                authority.identity(source(), start, end),
                WordIdentity::Other,
                "{text}"
            );
        }
        let projection = authority.project(source(), &timing, false);
        assert_eq!(projection.timing.text, "本人前句。本人返回。本人尾字。");
        assert_eq!(
            projection
                .timing
                .words
                .iter()
                .map(|word| content_key(&word.text))
                .collect::<String>(),
            content_key("本人前句本人返回本人尾字")
        );
        assert!(projection.excluded_other);
        assert!(
            !projection.unresolved,
            "known Other is skipped; it is not a prefix stop"
        );
    }

    #[test]
    fn pending_is_reversible_only_and_future_owner_cannot_bridge_it() {
        let timing = timed(
            "本人前句。尚未观察。本人返回。",
            &[
                ("本人前句", 100, 700),
                ("尚未观察", 900, 1_500),
                ("本人返回", 1_700, 2_300),
            ],
        );
        let mut authority = WordAuthority::new(source());
        authority.cells = vec![
            cell((0, 800), WordIdentity::Owner),
            cell((1_600, 2_400), WordIdentity::Owner),
        ];
        authority.settled_through_ms = 2_400;

        let irreversible = authority.project(source(), &timing, false);
        assert_eq!(irreversible.timing.text, "本人前句。");
        assert!(irreversible.unresolved);
        assert!(
            !irreversible.excluded_other,
            "missing identity is Pending, not Other"
        );
        let reversible = authority.project(source(), &timing, true);
        assert_eq!(reversible.timing.text, timing.text);
        assert!(reversible.unresolved);

        authority
            .cells
            .push(cell((950, 1_450), WordIdentity::Other));
        authority.complete = true;
        let resolved = authority.project(source(), &timing, false);
        assert_eq!(resolved.timing.text, "本人前句。本人返回。");
        assert!(resolved.excluded_other);
        assert!(!resolved.unresolved);
    }

    fn value_source(value: &Value) -> AudioSource {
        AudioSource {
            stream: value["stream"].as_u64().unwrap(),
            generation: value["generation"].as_u64().unwrap(),
        }
    }

    fn value_timing(value: &Value) -> TimedTranscript {
        TimedTranscript {
            text: value["text"].as_str().unwrap().into(),
            words: value["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|word| TimedTranscriptWord {
                    text: word["text"].as_str().unwrap().into(),
                    start_ms: word["start_ms"].as_u64(),
                    end_ms: word["end_ms"].as_u64(),
                })
                .collect(),
        }
    }

    fn value_ranges(value: &Value) -> Vec<(u64, u64)> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|range| (range[0].as_u64().unwrap(), range[1].as_u64().unwrap()))
            .collect()
    }

    fn sha256_file(path: &Path) -> String {
        format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
    }

    fn assert_file_hash(value: &Value, path_key: &str, hash_key: &str) {
        let path = Path::new(value[path_key].as_str().unwrap());
        assert_eq!(
            sha256_file(path),
            value[hash_key].as_str().unwrap(),
            "{}",
            path.display()
        );
    }

    fn report_words_at(fixture_case: &Value) -> Vec<(String, u64, u64)> {
        let replay = &fixture_case["provider_replay"];
        assert_eq!(
            replay["kind"],
            "fresh_provider_replay_on_identical_archived_pcm_not_original_live_packet"
        );
        assert_file_hash(replay, "report_path", "report_sha256");
        let report: Value = serde_json::from_slice(
            &std::fs::read(replay["report_path"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let response = &report["primary_provider_responses"]
            [replay["response_index"].as_u64().unwrap() as usize];
        assert_eq!(response["result"]["additions"]["log_id"], replay["log_id"]);
        response["result"]["utterances"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row["words"].as_array().into_iter().flatten())
            .map(|word| {
                (
                    word["text"].as_str().unwrap().to_string(),
                    word["start_time"].as_u64().unwrap(),
                    word["end_time"].as_u64().unwrap(),
                )
            })
            .collect()
    }

    /// Replays only saved timing and identity evidence. This intentionally does
    /// not run ASR or a speaker model. The provider packet is a fresh replay on
    /// identical archived PCM, not the dropped packet from the failed session.
    #[test]
    #[ignore = "requires fixed saved Nemotron/CAM evidence and fresh provider replay reports"]
    fn recorded_nemotron_cam_word_authority_replay() {
        let input =
            std::env::var_os("LISTENER_WORD_AUTHORITY_REPLAY_INPUT").expect("fixed replay input");
        let output =
            std::env::var_os("LISTENER_WORD_AUTHORITY_REPLAY_OUTPUT").expect("replay output");
        let fixture: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
        assert_eq!(
            fixture["expectations"]["original_live_packet_available"],
            false
        );
        assert_eq!(
            fixture["prediction_provenance"]["core_activity"]
                ["presentation_padding_used_as_identity"],
            false
        );
        for evidence in ["core_activity", "owner_channel_binding", "foreign_gaps"] {
            assert_file_hash(
                &fixture["prediction_provenance"][evidence],
                "path",
                "sha256",
            );
        }
        assert_eq!(
            fixture["prediction_provenance"]["owner_channel_binding"]["quality_target_count"],
            5
        );

        let core: Value = serde_json::from_slice(
            &std::fs::read(
                fixture["prediction_provenance"]["core_activity"]["path"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let bindings: Value = serde_json::from_slice(
            &std::fs::read(
                fixture["prediction_provenance"]["owner_channel_binding"]["path"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let gaps: Value = serde_json::from_slice(
            &std::fs::read(
                fixture["prediction_provenance"]["foreign_gaps"]["path"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();

        let mut total_owner = 0usize;
        let mut total_other = 0usize;
        let mut total_owner_unbound_core_overlaps = 0usize;
        let mut reports = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            assert_file_hash(&case["pcm"], "path", "sha256");
            let source = value_source(&case["source"]);
            let timing = value_timing(&case["timing"]);
            let provider_words = report_words_at(case);
            for word in &timing.words {
                assert!(
                    provider_words.contains(&(
                        word.text.clone(),
                        word.start_ms.unwrap(),
                        word.end_ms.unwrap()
                    )),
                    "fixture word is absent from fresh provider response: {}",
                    word.text
                );
            }

            let core_stream = core["streams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stream| stream["name"] == case["id"])
                .unwrap();
            let case_id = case["id"].as_str().unwrap();
            let qualified_bindings = bindings
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| {
                    row["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with(&format!("{case_id}-")))
                        && row["classification"]
                            .as_str()
                            .is_some_and(|value| value.starts_with("Target"))
                        && row["signal_quality_sufficient"] == true
                })
                .collect::<Vec<_>>();
            let mut binding_channels = qualified_bindings
                .iter()
                .map(|row| row["channel"].as_u64().unwrap())
                .collect::<Vec<_>>();
            binding_channels.sort_unstable();
            binding_channels.dedup();
            assert_eq!(
                binding_channels.len(),
                1,
                "exactly one anonymous channel must have quality Target evidence"
            );
            let bound_channel = binding_channels[0];
            assert_eq!(case["bound_channel"].as_u64().unwrap(), bound_channel);
            let binding_ids = qualified_bindings
                .iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                case["binding_row_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap())
                    .collect::<Vec<_>>(),
                binding_ids
            );
            let native_owner_ranges = core_stream["core_segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|segment| segment["speaker"].as_u64() == Some(bound_channel))
                .map(|segment| {
                    (
                        segment["start_ms"].as_f64().unwrap().round() as u64,
                        segment["end_ms"].as_f64().unwrap().round() as u64,
                    )
                })
                .collect::<Vec<_>>();
            let owner_ranges = value_ranges(&case["owner_core_ranges_ms"]);
            assert_eq!(
                owner_ranges, native_owner_ranges,
                "fixture must use unpadded native core activity"
            );
            let other_ranges = value_ranges(&case["other_ranges_ms"]);
            let qualified_gap_rows = gaps
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| {
                    row["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with(&format!("{case_id}-")))
                        && row["classification"]
                            .as_str()
                            .is_some_and(|value| value.starts_with("NonTarget"))
                        && row["signal_quality_sufficient"] == true
                        && row["transcript_speaker_evidence"] == "hard_non_target"
                })
                .collect::<Vec<_>>();
            assert_eq!(
                other_ranges,
                qualified_gap_rows
                    .iter()
                    .map(|row| {
                        (
                            row["start_ms"].as_u64().unwrap(),
                            row["end_ms"].as_u64().unwrap(),
                        )
                    })
                    .collect::<Vec<_>>(),
                "fixture must include every gap selected by the uniform CAM rule"
            );
            assert_eq!(
                case["other_row_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap())
                    .collect::<Vec<_>>(),
                qualified_gap_rows
                    .iter()
                    .map(|row| row["id"].as_str().unwrap())
                    .collect::<Vec<_>>()
            );

            let native_unbound_ranges = core_stream["core_segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|segment| segment["speaker"].as_u64() != Some(bound_channel))
                .map(|segment| {
                    (
                        segment["start_ms"].as_f64().unwrap().round() as u64,
                        segment["end_ms"].as_f64().unwrap().round() as u64,
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                native_unbound_ranges,
                case["unbound_core_ranges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        (
                            item["range_ms"][0].as_u64().unwrap(),
                            item["range_ms"][1].as_u64().unwrap(),
                        )
                    })
                    .collect::<Vec<_>>()
            );
            assert!(
                case["marked_owner_unbound_core_overlaps"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "concurrent unbound activity is unresolved, not automatically Owner or Other"
            );

            // Do not backdate final prediction. Only ranges closed by the saved
            // live boundary may create irreversible text at this checkpoint.
            let live_through = case["live_settled_through_ms"].as_u64().unwrap();
            let mut live = WordAuthority::new(source);
            live.cells = owner_ranges
                .iter()
                .copied()
                .filter(|range| range.1 <= live_through)
                .map(|range| cell(range, WordIdentity::Owner))
                .collect();
            live.settled_through_ms = live_through;
            let live_projection = live.project(source, &timing, false);
            assert_eq!(
                live_projection.timing.text,
                case["expected_live_irreversible_text"].as_str().unwrap()
            );
            assert!(
                live_projection.unresolved,
                "future final evidence must remain Pending live"
            );
            assert!(
                !live_projection.excluded_other,
                "offline final Other was not yet live evidence"
            );

            let mut final_authority = WordAuthority::new(source);
            final_authority.cells.extend(
                owner_ranges
                    .iter()
                    .copied()
                    .map(|range| cell(range, WordIdentity::Owner)),
            );
            final_authority.cells.extend(
                other_ranges
                    .iter()
                    .copied()
                    .map(|range| cell(range, WordIdentity::Other)),
            );
            final_authority.complete = true;
            final_authority.settled_through_ms = case["final_settled_through_ms"].as_u64().unwrap();
            let decisions = timing
                .words
                .iter()
                .map(|word| {
                    final_authority.identity(source, word.start_ms.unwrap(), word.end_ms.unwrap())
                })
                .collect::<Vec<_>>();
            let owner = decisions
                .iter()
                .filter(|&&decision| decision == WordIdentity::Owner)
                .count();
            let other = decisions
                .iter()
                .filter(|&&decision| decision == WordIdentity::Other)
                .count();
            let pending = decisions
                .iter()
                .filter(|&&decision| decision == WordIdentity::Pending)
                .count();
            let owner_unbound_core_overlaps = timing
                .words
                .iter()
                .zip(&decisions)
                .filter(|(_, decision)| **decision == WordIdentity::Owner)
                .filter(|(word, _)| {
                    let range = (word.start_ms.unwrap(), word.end_ms.unwrap());
                    native_unbound_ranges
                        .iter()
                        .any(|other| other.0 < range.1 && range.0 < other.1)
                })
                .count();
            assert_eq!(
                owner_unbound_core_overlaps, 0,
                "a bound-channel midpoint cannot certify a single Owner source word while an unbound core channel overlaps it"
            );
            assert_eq!(
                owner,
                case["expected_owner_words"].as_u64().unwrap() as usize
            );
            assert_eq!(
                other,
                case["expected_other_words"].as_u64().unwrap() as usize
            );
            assert_eq!(
                pending, 0,
                "every labelled fixture word must have final evidence"
            );
            let final_projection = final_authority.project(source, &timing, false);
            assert_eq!(
                final_projection.timing.text,
                case["expected_final_text"].as_str().unwrap()
            );
            assert_eq!(final_projection.timing.words.len(), owner);
            assert!(!final_projection.unresolved);
            assert_eq!(final_projection.excluded_other, other > 0);
            // Same text/timing on a different generation has no permission.
            let wrong_source = AudioSource {
                stream: source.stream,
                generation: source.generation + 1,
            };
            assert!(final_authority
                .project(wrong_source, &timing, false)
                .timing
                .text
                .is_empty());

            total_owner += owner;
            total_other += other;
            total_owner_unbound_core_overlaps += owner_unbound_core_overlaps;
            reports.push(serde_json::json!({"id":case["id"],"live_irreversible_text":live_projection.timing.text,
                "final_text":final_projection.timing.text,"owner_words":owner,"other_words":other,
                "pending_words":pending,"owner_words_overlapping_unbound_core":owner_unbound_core_overlaps,
                "qualified_other_gap_count":other_ranges.len(),
                "fresh_provider_response_index":case["provider_replay"]["response_index"]}));
        }
        assert_eq!(
            total_owner,
            fixture["expectations"]["marked_owner_words_preserved"]
                .as_u64()
                .unwrap() as usize
        );
        assert_eq!(
            total_other,
            fixture["expectations"]["foreign_words_excluded"]
                .as_u64()
                .unwrap() as usize
        );
        let result = serde_json::json!({"status":"PASS","scope":fixture["scope"],
            "input_sha256":sha256_file(Path::new(&input)),"owner_words_preserved":total_owner,
            "foreign_words_excluded":total_other,
            "owner_words_overlapping_unbound_core":total_owner_unbound_core_overlaps,"cases":reports,
            "limits":["fresh provider replay is not the dropped original packet",
                "final saved evidence is not backdated into live authority",
                "quiet qualifying CAM gaps overlap no marked word and may be environment; they do not prove a human Other",
                "anonymous diarization channels are never automatically Owner or Other",
                "source-level projection only; no production integration, editor, device, or installed runtime acceptance"]});
        std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}
