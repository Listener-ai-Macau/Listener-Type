// The final decoder owns its words and timing. Online and two-pass spans are
// different estimates, not interchangeable certificates for word occurrences.
// The caller binds the exact whole PCM prefix to the current capture/session.
fn snapshot_owned_common_prefix(
    parent_signature: &SourceOwnerProjectionSignature,
    parent_candidate_text: &str,
    child_final: &crate::transcript_boundary::TimedTranscript,
    captured_pcm_range_ms: (u64, u64),
    owner_ranges: &[(u64, u64)],
    foreign_ranges: &[(u64, u64)],
) -> Option<crate::transcript_boundary::TimedTranscript> {
    // Cropped clips require an explicit audio-coordinate mapping. Text search
    // must never be used to recover a missing capture origin.
    if captured_pcm_range_ms.0 != 0 || captured_pcm_range_ms.1 == 0 { return None; }
    snapshot_owned_aligned_prefix(parent_signature, parent_candidate_text, child_final,
        captured_pcm_range_ms, owner_ranges, foreign_ranges)
}

// Both inputs here are already in original capture coordinates. A cropped
// decoder must be explicitly shifted and cut at the submitted audio seam first.
fn snapshot_owned_aligned_prefix(
    parent_signature: &SourceOwnerProjectionSignature,
    parent_candidate_text: &str,
    child_final: &crate::transcript_boundary::TimedTranscript,
    captured_pcm_range_ms: (u64, u64),
    owner_ranges: &[(u64, u64)],
    foreign_ranges: &[(u64, u64)],
) -> Option<crate::transcript_boundary::TimedTranscript> {
    use crate::transcript_boundary::content_key;
    use std::collections::BTreeSet;
    if captured_pcm_range_ms.0 >= captured_pcm_range_ms.1 { return None; }
    let parent_key = content_key(parent_candidate_text);
    let child_key = content_key(&child_final.text);
    if parent_key.is_empty() || parent_key != parent_signature.key || child_key.is_empty() { return None; }
    let mut parent_covered = String::new();
    let mut parent_boundaries = BTreeSet::new();
    for (text, _, _) in &parent_signature.words {
        let key = content_key(text);
        if key.is_empty() { continue; }
        if !parent_key.get(parent_covered.len()..)?.starts_with(&key) { return None; }
        parent_covered.push_str(&key);
        parent_boundaries.insert(parent_covered.len());
    }
    if parent_covered != parent_key { return None; }
    // Validate every child token before projection: malformed or missing
    // coordinates cannot inherit parent permission or hide behind truncation.
    let mut child_covered = String::new();
    let mut child_boundaries = BTreeSet::new();
    let mut previous = None;
    for word in &child_final.words {
        let key = content_key(&word.text);
        if key.is_empty() { continue; }
        let (start, end) = (word.start_ms?, word.end_ms?);
        if start < captured_pcm_range_ms.0 || start >= end || end > captured_pcm_range_ms.1
            || previous.is_some_and(|(s, e)| start < s || end < e)
            || !child_key.get(child_covered.len()..)?.starts_with(&key)
        { return None; }
        previous = Some((start, end));
        child_covered.push_str(&key);
        child_boundaries.insert(child_covered.len());
    }
    if child_covered != child_key { return None; }
    // No accepted text anchor: every child word must be owned in its own
    // coordinates, including words which the parent had already admitted.
    let child_owned = child_final.owner_supported_prefix("", owner_ranges, foreign_ranges);
    let child_owned_key = content_key(&child_owned);
    let common: String = parent_key.chars().zip(child_owned_key.chars())
        .take_while(|(parent, child)| parent == child).map(|(ch, _)| ch).collect();
    // Matching starts at character zero; never skip filler or resync a repeat.
    // Both decoders must have a whole-token boundary at the selected frontier.
    let end = parent_boundaries.intersection(&child_boundaries)
        .copied().filter(|end| *end <= common.len()).max()?;
    snapshot_timed_prefix(child_final, &common[..end])
}

#[cfg(test)]
mod snapshot_confirmation_tests {
    use super::*;
    use crate::transcript_boundary::{content_key, TimedTranscript, TimedTranscriptWord};
    fn parent(text: &str, tokens: &[&str]) -> SourceOwnerProjectionSignature {
        SourceOwnerProjectionSignature { independent_anchor_key:None, key:content_key(text),
            words:tokens.iter().enumerate().map(|(i, text)|
                (content_key(text),Some(i as u64 * 100),Some(i as u64 * 100 + 80))).collect() }
    }
    fn child(tokens: &[(&str,u64,u64)]) -> TimedTranscript {
        TimedTranscript { text:tokens.iter().map(|(text,_,_)|*text).collect(),
            words:tokens.iter().map(|(text,start,end)|TimedTranscriptWord {
                text:(*text).into(),start_ms:Some(*start),end_ms:Some(*end),
            }).collect() }
    }
    fn confirm(parent: &SourceOwnerProjectionSignature, child: &TimedTranscript,
        owners: &[(u64,u64)], foreign: &[(u64,u64)]) -> Option<TimedTranscript> {
        snapshot_owned_common_prefix(parent,&parent.key,child,(0,3000),owners,foreign)
    }
    #[test]
    fn final_decoder_keeps_own_timing_when_stream_tokenization_changes() {
        let p=parent("开始录音", &["开始","录音"]);
        let c=child(&[("开",280,520),("始",520,640),("录",640,840),("音。",840,1000)]);
        let candidate=confirm(&p,&c,&[(0,1100)],&[]).unwrap();
        assert_eq!(candidate.text,"开始录音。");
        assert_eq!(candidate.words[0].start_ms,Some(280));
    }
    #[test]
    fn repeated_text_is_not_resynced_past_an_unowned_first_occurrence() {
        let p=parent("继续继续", &["继续","继续"]);
        let c=child(&[("继续",0,400),("继续",1000,1400)]);
        assert!(confirm(&p,&c,&[(1000,1600)],&[]).is_none());
        assert!(confirm(&p,&c,&[(0,1600)],&[(0,400)]).is_none());
        assert_eq!(confirm(&p,&c,&[(0,1600)],&[]).unwrap().text,"继续继续");
    }
    #[test]
    fn child_coordinates_cannot_inherit_parent_permission() {
        let p=parent("本人正文", &["本人","正文"]);
        let c=child(&[("本人",1000,1300),("正文",1300,1600)]);
        assert!(confirm(&p,&c,&[(0,500)],&[]).is_none());
        assert!(confirm(&p,&c,&[(0,2000)],&[(1100,1200)]).is_none());
    }
    #[test]
    fn malformed_child_timing_or_incomplete_text_fails_closed() {
        let p=parent("正文", &["正文"]);
        for c in [child(&[("正文",500,400)]), child(&[("正文",0,3001)]),
            TimedTranscript {text:"正文".into(),words:vec![TimedTranscriptWord {
                text:"正文".into(),start_ms:Some(0),end_ms:None,
            }]}, child(&[("正",0,100)])] {
            assert!(confirm(&p,&c,&[(0,4000)],&[]).is_none());
        }
        let mut incomplete=child(&[("正文",0,100)]);
        incomplete.text.push_str("尾句");
        assert!(confirm(&p,&incomplete,&[(0,4000)],&[]).is_none());
    }
    #[test]
    fn shifted_clip_origin_and_leading_mismatch_do_not_search_for_a_match() {
        let p=parent("正文", &["正文"]);
        let c=child(&[("正文",0,400)]);
        assert!(snapshot_owned_common_prefix(&p,&p.key,&c,(1000,2000),&[(0,2000)],&[]).is_none());
        let filler=child(&[("嗯",0,100),("正文",200,400)]);
        assert!(confirm(&p,&filler,&[(0,2000)],&[]).is_none());
    }
    #[test]
    fn foreign_tail_and_wording_revision_stop_at_whole_common_tokens() {
        let p=parent("今天安排结束", &["今天","安排","结束"]);
        let c=child(&[("今天。",0,400),("计划",500,900),("结束",1000,1400)]);
        assert_eq!(confirm(&p,&c,&[(0,2000)],&[]).unwrap().text,"今天。");
        let p=parent("本人正文", &["本人","正文"]);
        let c=child(&[("本人",0,400),("正文。",400,800),("旁人尾巴",900,1600)]);
        assert_eq!(confirm(&p,&c,&[(0,800)],&[(900,1600)]).unwrap().text,"本人正文。");
    }
    #[test]
    fn common_prefix_cannot_split_either_decoders_indivisible_token() {
        let p=parent("本人正文", &["本人正文"]);
        let c=child(&[("本人",0,400),("尾句",500,900)]);
        assert!(confirm(&p,&c,&[(0,2000)],&[]).is_none());
    }
}
