//! Stateful merge for provider responses without utterance timestamps.
//!
//! Volcengine can move from a cumulative hypothesis to a rolling text window.
//! The first moved window must be appended by overlap; later revisions of that
//! same window must replace its suffix instead of being appended again.

use super::volcengine_transcript::{
    is_same_prefix_streaming_revision, merge_streaming_candidate, TranscriptCandidate,
    TranscriptSegment,
};

pub(super) fn merge_streaming_candidate_with_untimed_window(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    previous_untimed_window: &str,
    candidate: TranscriptCandidate,
) -> (String, Vec<TranscriptSegment>, String) {
    if !candidate.timed_segments.is_empty() {
        let (text, segments) =
            merge_streaming_candidate(previous_text, previous_segments, candidate);
        return (text, segments, String::new());
    }

    let current_window = candidate.text.trim().to_string();
    let replacement =
        replace_revised_untimed_suffix(previous_text, previous_untimed_window, &current_window);
    let (text, segments) = replacement.map_or_else(
        || merge_streaming_candidate(previous_text, previous_segments, candidate),
        |text| (text, previous_segments.to_vec()),
    );
    (text, segments, current_window)
}

fn replace_revised_untimed_suffix(
    previous_text: &str,
    previous_window: &str,
    current_window: &str,
) -> Option<String> {
    let previous_text = previous_text.trim();
    let previous_window = previous_window.trim();
    let current_window = current_window.trim();
    if previous_window.is_empty()
        || current_window.is_empty()
        || !is_same_prefix_streaming_revision(previous_window, current_window)
    {
        return None;
    }

    if previous_text == previous_window {
        return Some(current_window.to_string());
    }
    previous_text
        .strip_suffix(previous_window)
        .map(|stable_prefix| format!("{stable_prefix}{current_window}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn untimed(text: &str) -> TranscriptCandidate {
        TranscriptCandidate {
            text: text.into(),
            timed_segments: Vec::new(),
            authoritative_cumulative: false,
        }
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
}
