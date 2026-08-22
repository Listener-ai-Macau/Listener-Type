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
        || !(is_same_prefix_streaming_revision(previous_window, current_window)
            || is_probable_growing_cumulative_revision(previous_window, current_window))
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
        || current.len() <= previous.len()
        || current.len().saturating_sub(previous.len()) > 12
    {
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
