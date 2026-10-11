// Embedded audio partial preview / wake-phrase filtering helpers.
// Included into `coordinator::dictation` via `include!`.

#[derive(Debug)]
struct ProductFinalCandidates {
    provider_primary: RawTranscript,
    verified_provider_owner: Option<RawTranscript>,
    separated_owner: Option<RawTranscript>,
    retained_audio_replay: Option<RawTranscript>,
    debug_override: Option<RawTranscript>,
    partial_preview: Option<RawTranscript>,
    local_shadow: Option<String>,
    local_shadow_owner_end_aligned: bool,
    content_supplement: Option<crate::asr::content_consensus::ContentSupplement>,
    target_filter_required: bool,
    primary_mixture_rejected: bool,
    /// The exact primary rendering is fully covered by completed source-owner
    /// permissions, independently of its provider filtering label.
    primary_speaker_filtered_certified: bool,
    /// The removed suffix has explicit Other word intervals. A mere shorter
    /// rendering or unfinished identity analysis does not establish this.
    separated_owner_exclusion_confirmed: bool,
}

#[derive(Debug)]
struct ProductFinalDecision {
    transcript: RawTranscript,
    authority: crate::speech_decision_kernel::ProductFinalAuthority,
    local_shadow_recovered: bool,
    content_supplement_recovered: bool,
}

fn nonempty_transcript(candidate: &Option<RawTranscript>) -> bool {
    candidate
        .as_ref()
        .is_some_and(|candidate| !candidate.text.trim().is_empty())
}

fn spoken_content_units(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

/// needle 的说出字符是否按顺序整体出现在 haystack 里（紧凑子序列）。用于区分
/// "主终稿只是比分离稿多一段尾巴"（分离轨在做排除工作）与"两轨内容分叉"
/// （分离稿被提取伪影劣化，r24）。
fn compact_subsequence_of(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<std::collections::VecDeque<_>>();
    for needle_ch in needle.chars().filter(|ch| ch.is_alphanumeric()) {
        loop {
            match haystack.pop_front() {
                Some(haystack_ch) if haystack_ch == needle_ch => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// The only product-boundary text selector. Every upstream result is supplied
/// as an immutable candidate, one authority is selected once, and optional
/// omission/hotword/filler normalization happens before the returned content
/// is sealed by the caller. Downstream recovery may not invent new sources.
/// A later cloud final still must not shrink an already-visible owner prefix.
fn arbitrate_product_final_transcript(
    candidates: ProductFinalCandidates,
    hotwords: &[DictionaryHotword],
    remove_filler_words: bool,
) -> ProductFinalDecision {
    use crate::speech_decision_kernel::{
        arbitrate_product_final, ProductFinalAuthority, ProductFinalEvidence,
    };

    // Extraction can damage owner words or truncate a valid continuation.
    // A shorter prefix alone cannot distinguish that failure from successful
    // foreign-tail removal. Keep the covering primary rendering unless the
    // omitted words have explicit Other evidence or the mixture was rejected.
    // This fidelity choice does not grant the primary an owner certificate;
    // the ordinary-primary fallback below retains its separate policy.
    let mut candidates = candidates;
    let separated_units = candidates
        .separated_owner
        .as_ref()
        .map(|separated| spoken_content_units(&separated.text));
    let primary_units = if candidates.provider_primary.text.trim().is_empty() {
        None
    } else {
        Some(spoken_content_units(&candidates.provider_primary.text))
    };
    if let (Some(primary), Some(separated)) = (primary_units, separated_units) {
        let separated_still_excluding = !candidates.primary_speaker_filtered_certified
            && candidates.separated_owner_exclusion_confirmed
            && primary > separated
            && {
                let separated_text = candidates
                    .separated_owner
                    .as_ref()
                    .map(|candidate| candidate.text.as_str())
                    .unwrap_or_default();
                compact_subsequence_of(separated_text, &candidates.provider_primary.text)
            };
        if !candidates.primary_mixture_rejected && primary >= separated && !separated_still_excluding {
            log::info!(
                "[target-speaker] separated owner final demoted: {} (primary_spoken={primary} separated_spoken={separated}); keeping primary rendering",
                if candidates.primary_speaker_filtered_certified {
                    "certified speaker-filtered primary covers truncated/degraded separated decode"
                } else {
                    "primary covers diverged separated decode"
                }
            );
            candidates.separated_owner = None;
        }
    }

    let authority = arbitrate_product_final(ProductFinalEvidence {
        verified_provider_owner_available: nonempty_transcript(&candidates.verified_provider_owner),
        target_filter_required: candidates.target_filter_required,
        primary_mixture_rejected: candidates.primary_mixture_rejected,
        separated_owner_available: nonempty_transcript(&candidates.separated_owner),
        provider_primary_available: !candidates.provider_primary.text.trim().is_empty(),
        retained_audio_replay_available: nonempty_transcript(&candidates.retained_audio_replay),
        debug_override_available: nonempty_transcript(&candidates.debug_override),
        partial_preview_available: nonempty_transcript(&candidates.partial_preview),
    });

    // A preview can contribute hotword spelling only when it is the selected
    // final source. A rejected preview must not influence a provider or owner
    // candidate through a later normalization pass.
    let preview_for_hotwords = matches!(
        authority,
        ProductFinalAuthority::PartialPreviewRecovery
    )
    .then(|| candidates.partial_preview.as_ref().map(|preview| preview.text.clone()))
    .flatten();

    let mut transcript = match authority {
        ProductFinalAuthority::VerifiedProviderOwner => candidates
            .verified_provider_owner.expect("certified distinct provider owner candidate"),
        ProductFinalAuthority::SeparatedOwner => candidates
            .separated_owner
            .expect("available separated-owner candidate"),
        ProductFinalAuthority::ProviderPrimary => candidates.provider_primary,
        ProductFinalAuthority::RetainedAudioReplay => candidates
            .retained_audio_replay
            .expect("available retained-audio candidate"),
        ProductFinalAuthority::DebugOverride => candidates
            .debug_override
            .expect("available debug transcript candidate"),
        ProductFinalAuthority::PartialPreviewRecovery => candidates
            .partial_preview
            .expect("available partial-preview candidate"),
        ProductFinalAuthority::Empty => RawTranscript {
            text: String::new(),
            duration_ms: candidates.provider_primary.duration_ms,
        },
    };

    let local_shadow_eligible = !candidates.target_filter_required
        && candidates.local_shadow_owner_end_aligned
        && matches!(
            authority,
            ProductFinalAuthority::ProviderPrimary
                | ProductFinalAuthority::RetainedAudioReplay
        );
    let mut local_shadow_recovered = false;
    if local_shadow_eligible {
        if let Some(local) = candidates.local_shadow.as_deref() {
            if let Some(recovered) = recover_local_shadow_omissions(&transcript.text, local) {
                transcript.text = recovered;
                local_shadow_recovered = true;
            }
        }
    }

    let mut content_supplement_recovered = false;
    if !candidates.target_filter_required && !candidates.primary_mixture_rejected
        && matches!(authority, ProductFinalAuthority::ProviderPrimary) {
        if let Some(supplement) = candidates.content_supplement.as_ref() {
            let decision = crate::asr::content_consensus::append_consensus(&transcript.text, supplement);
            content_supplement_recovered = !decision.appended_channels.is_empty();
            log::info!("[content-recovery] shared final decision reason={} appended_channels={} owner_identity=false endpoint_changed=false",
                decision.reason, decision.appended_channels.len());
            transcript.text = decision.text;
        }
    }

    if let Some(preview) = preview_for_hotwords.as_deref() {
        transcript.text = reconcile_final_transcript_with_preview_hotwords(
            &transcript.text,
            preview,
            hotwords,
        );
    }
    if remove_filler_words {
        transcript.text = remove_standalone_dictation_fillers(&transcript.text);
    }

    ProductFinalDecision {
        transcript,
        authority,
        local_shadow_recovered,
        content_supplement_recovered,
    }
}

fn current_embedded_audio_final_preview_candidate(
    inner: &Arc<Inner>,
    session_id: SessionId,
    duration_ms: u64,
) -> Option<RawTranscript> {
    // The visible high-water mark can retain a provisional or superseded tail
    // after speaker filtering corrects the authoritative ledger. It is useful
    // for display cadence, but is not evidence for insertion or recovery.
    let preview = inner.embedded_audio_preview.lock().authoritative(session_id);
    preview.map(|preview| RawTranscript {
        text: filter_automatic_wake_text(inner, session_id, &preview, false),
        duration_ms,
    })
}

fn invalidate_embedded_audio_authoritative_preview(
    inner: &Arc<Inner>,
    session_id: SessionId,
    reason: &'static str,
) -> bool {
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!("authoritative_preview_invalidated reason={reason}"),
        |_| {
            inner
                .embedded_audio_preview
                .lock()
                .invalidate_authoritative(session_id)
        },
    )
}

fn compact_spoken_preview(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace() && !is_embedded_audio_partial_preview_decorative(*ch)
        })
        .collect()
}

fn reconcile_final_transcript_with_preview_hotwords(
    final_text: &str,
    preview: &str,
    hotwords: &[DictionaryHotword],
) -> String {
    let preview_key = ascii_alphanumeric_key(preview);
    if preview_key.is_empty() {
        return final_text.to_string();
    }

    let mut corrected = final_text.to_string();
    for hotword in hotwords {
        if !hotword.enabled {
            continue;
        }
        let phrase = hotword.phrase.trim();
        if phrase.len() < 3 || !phrase.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            continue;
        }
        let phrase_key = phrase.to_ascii_lowercase();
        if !preview_key.contains(&phrase_key)
            || ascii_alphanumeric_key(&corrected).contains(&phrase_key)
        {
            continue;
        }

        let replacement =
            spaced_ascii_token_spans(&corrected)
                .into_iter()
                .find_map(|(start, end, candidate)| {
                    (ascii_edit_distance_at_most_one(&candidate, &phrase_key) == Some(1))
                        .then_some((start, end))
                });
        if let Some((start, end)) = replacement {
            corrected.replace_range(start..end, phrase);
            log::info!(
                "[coord] restored preview-confirmed ASR hotword in final transcript: {phrase}"
            );
        }
    }
    corrected
}

/// Recover only characters that a warm local decode can insert around an
/// otherwise unchanged cloud transcript. This is intentionally much stricter
/// than a general ASR merge: every cloud alphanumeric character must occur in
/// order in the local text, no cloud character can be replaced, one-character
/// disagreements are rejected (Paraformer can hallucinate those), and the
/// total addition is tightly bounded. Punctuation from the authoritative cloud
/// result is preserved verbatim.
fn recover_local_shadow_omissions(cloud: &str, local: &str) -> Option<String> {
    fn units(text: &str) -> Vec<(char, usize, usize)> {
        text.char_indices()
            .filter_map(|(start, ch)| {
                ch.is_alphanumeric().then_some((
                    if ch.is_ascii() {
                        ch.to_ascii_lowercase()
                    } else {
                        ch
                    },
                    start,
                    start + ch.len_utf8(),
                ))
            })
            .collect()
    }

    let cloud_units = units(cloud);
    let local_units = units(local);
    if cloud_units.len() < 10 || local_units.len() <= cloud_units.len() {
        return None;
    }

    let mut matched_local = Vec::with_capacity(cloud_units.len());
    let mut local_cursor = 0usize;
    for (cloud_ch, _, _) in &cloud_units {
        let relative = local_units[local_cursor..]
            .iter()
            .position(|(local_ch, _, _)| local_ch == cloud_ch)?;
        let matched = local_cursor + relative;
        matched_local.push(matched);
        local_cursor = matched + 1;
    }

    let mut additions = Vec::<(usize, String)>::new();
    let mut added_chars = 0usize;
    for cloud_index in 0..=cloud_units.len() {
        let local_start = if cloud_index == 0 {
            0
        } else {
            matched_local[cloud_index - 1] + 1
        };
        let local_end = if cloud_index == cloud_units.len() {
            local_units.len()
        } else {
            matched_local[cloud_index]
        };
        if local_start == local_end {
            continue;
        }
        let gap_len = local_end - local_start;
        // A single added character is more likely to be a local-model
        // insertion (the public overlap sample produced exactly one) than a
        // trustworthy cloud omission. Large/busy gaps can be another speaker.
        if gap_len < 2 || gap_len > 8 {
            return None;
        }
        if (cloud_index == 0 || cloud_index == cloud_units.len()) && gap_len > 6 {
            return None;
        }
        added_chars += gap_len;
        let text: String = local_units[local_start..local_end]
            .iter()
            .map(|(ch, _, _)| *ch)
            .collect();
        additions.push((cloud_index, text));
    }

    let allowed_added_chars = (cloud_units.len() / 5).clamp(2, 12);
    if additions.is_empty() || additions.len() > 2 || added_chars > allowed_added_chars {
        return None;
    }

    let mut additions = additions.into_iter().peekable();
    let mut recovered = String::with_capacity(cloud.len() + added_chars * 3);
    let mut copied_until = 0usize;
    for cloud_index in 0..=cloud_units.len() {
        let insertion_offset = if cloud_index < cloud_units.len() {
            cloud_units[cloud_index].1
        } else {
            cloud_units.last()?.2
        };
        if insertion_offset > copied_until {
            recovered.push_str(&cloud[copied_until..insertion_offset]);
            copied_until = insertion_offset;
        }
        while additions
            .peek()
            .is_some_and(|(before_index, _)| *before_index == cloud_index)
        {
            let (_, addition) = additions.next().expect("peeked local shadow addition");
            recovered.push_str(&addition);
        }
    }
    recovered.push_str(&cloud[copied_until..]);
    (recovered != cloud).then_some(recovered)
}

fn ascii_alphanumeric_key(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn spaced_ascii_token_spans(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (start, first) = chars[index];
        if !first.is_ascii_alphanumeric() {
            index += 1;
            continue;
        }

        let mut end = start + first.len_utf8();
        let mut token = String::from(first.to_ascii_lowercase());
        index += 1;
        loop {
            let separator_start = index;
            while index < chars.len() && chars[index].1.is_ascii_whitespace() {
                index += 1;
            }
            if index < chars.len() && chars[index].1.is_ascii_alphanumeric() {
                let (next_offset, next) = chars[index];
                token.push(next.to_ascii_lowercase());
                end = next_offset + next.len_utf8();
                index += 1;
                continue;
            }
            index = if separator_start == index {
                index
            } else {
                separator_start
            };
            break;
        }
        spans.push((start, end, token));
    }
    spans
}

fn ascii_edit_distance_at_most_one(left: &str, right: &str) -> Option<usize> {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len().abs_diff(right.len()) > 1 {
        return None;
    }

    let (mut left_index, mut right_index, mut edits) = (0, 0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            left_index += 1;
            right_index += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return None;
        }
        if left.len() > right.len() {
            left_index += 1;
        } else if right.len() > left.len() {
            right_index += 1;
        } else {
            left_index += 1;
            right_index += 1;
        }
    }
    edits += left.len().saturating_sub(left_index) + right.len().saturating_sub(right_index);
    (edits <= 1).then_some(edits)
}

fn update_embedded_audio_partial_preview(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: String,
) -> bool {
    reduce_embedded_audio_authoritative_preview(
        inner,
        session_id,
        text,
        false,
        crate::observability::PreviewSource::ProviderStream,
        "stream",
    )
}

fn reduce_embedded_audio_authoritative_preview(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: String,
    settle_visible: bool,
    source: crate::observability::PreviewSource,
    source_label: &'static str,
) -> bool {
    let preview = filter_dictation_preview_text(inner, session_id, &text);
    if preview.is_empty() {
        // A normal empty provider partial is not an ownership denial and must
        // not retract a live preview. Explicit ownership invalidation uses
        // invalidate_embedded_audio_authoritative_preview at the final
        // arbitration boundary below.
        return false;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!(
            "source={source_label} chars={} settle_visible={settle_visible}",
            preview.chars().count()
        ),
        |_| {
            let reduction = inner.embedded_audio_preview.lock().observe_authoritative(
                session_id,
                &preview,
                settle_visible,
            );
            let emitted = reduction.visible_update.is_some_and(|visible| {
                emit_embedded_audio_partial_preview_if_active(inner, session_id, visible)
            });
            if emitted {
                crate::observability::record_embedded_audio_preview_published(
                    session_id,
                    source,
                    embedded_audio_stop_feedback_latched(inner),
                );
            }
            reduction.authoritative_changed
        },
    )
}

/// Publish a diarization-pending provider tail to the capsule without
/// promoting it into the authoritative preview ledger. The ASR layer rejects
/// this callback on current NonTarget evidence, a debounced switch away from
/// the owner, or a suspected owner handoff; open/untracked wake policy remains
/// valid. It may therefore improve visual cadence, but cannot extend the
/// endpoint or enter final insertion/recovery.
fn update_embedded_audio_visual_preview(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: String,
) -> bool {
    let preview = filter_dictation_visual_preview_text(inner, session_id, &text);
    if preview.is_empty() {
        return false;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!("visual_provisional chars={}", preview.chars().count()),
        |_| {
            let Some(provider_preview) = inner
                .embedded_audio_preview
                .lock()
                .observe_provisional(session_id, &preview)
            else {
                return false;
            };
            emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview)
        },
    )
}

fn reconcile_embedded_audio_owner_visual(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: String,
) -> bool {
    let owner_view = filter_dictation_visual_preview_text(inner, session_id, &text);
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!("confirmed_owner_visual chars={}", owner_view.chars().count()),
        |_| {
            let Some(visible) = inner.embedded_audio_preview.lock()
                .reconcile_confirmed_owner_partition(session_id, &owner_view)
            else { return false; };
            emit_embedded_audio_partial_preview_if_active(inner, session_id, visible)
        },
    )
}

fn update_embedded_audio_partial_preview_from_final_supplement(
    inner: &Arc<Inner>,
    session_id: SessionId,
    update: crate::asr::volcengine::FinalIntermediateTranscript,
) -> bool {
    let authoritative_two_pass = update.authoritative_two_pass;
    reduce_embedded_audio_authoritative_preview(
        inner,
        session_id,
        update.text,
        true,
        crate::observability::PreviewSource::FinalSupplement,
        if authoritative_two_pass {
            "final_supplement_two_pass"
        } else {
            "final_supplement"
        },
    )
}


fn embedded_audio_partial_preview_stability_key(text: &str) -> String {
    crate::transcript_boundary::content_key(text)
}

fn is_embedded_audio_partial_preview_decorative(ch: char) -> bool {
    crate::transcript_boundary::is_decorative(ch)
}

fn preserve_recording_transcript(text: &str) -> String {
    text.trim().to_string()
}

fn wake_phrase_character_matches(actual: char, expected: char) -> bool {
    if actual == expected {
        return true;
    }
    use pinyin::ToPinyin;
    actual
        .to_pinyin()
        .zip(expected.to_pinyin())
        .is_some_and(|(actual, expected)| actual.plain() == expected.plain())
}

fn strip_activation_after_short_lead_in(text: &str, phrase: &[char]) -> Option<String> {
    const MAX_LEAD_IN_CONTENT_CHARS: usize = 2;
    if phrase.len() < 2 || text.is_empty() {
        return None;
    }
    let mut content_starts = Vec::new();
    for (index, ch) in text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch)
            || matches!(ch, '嗯' | '呃' | '额' | '唔')
        {
            continue;
        }
        content_starts.push(index);
        if content_starts.len() > MAX_LEAD_IN_CONTENT_CHARS + phrase.len() {
            break;
        }
    }
    let max_lead = MAX_LEAD_IN_CONTENT_CHARS.min(content_starts.len().saturating_sub(1));
    for lead in 1..=max_lead {
        let remainder = &text[content_starts[lead]..];
        let mut phrase_index = 0usize;
        let mut consumed_end = 0usize;
        for (index, ch) in remainder.char_indices() {
            let next = index + ch.len_utf8();
            if is_embedded_audio_partial_preview_decorative(ch) {
                if phrase_index > 0 {
                    consumed_end = next;
                }
                continue;
            }
            if phrase_index == phrase.len() {
                break;
            }
            if !wake_phrase_character_matches(ch, phrase[phrase_index]) {
                phrase_index = 0;
                break;
            }
            phrase_index += 1;
            consumed_end = next;
        }
        if phrase_index == phrase.len() {
            return Some(
                remainder[consumed_end..]
                    .trim_start_matches(is_embedded_audio_partial_preview_decorative)
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

fn strip_bounded_activation_suffix(text: &str, phrase: &[char]) -> Option<String> {
    if phrase.len() < 2 {
        return None;
    }
    for suffix_start in 1..phrase.len() {
        let mut text_chars = text.char_indices();
        let mut consumed_end = 0usize;
        let mut matched = true;
        for expected in &phrase[suffix_start..] {
            let Some((index, actual)) = text_chars.next() else {
                matched = false;
                break;
            };
            if is_embedded_audio_partial_preview_decorative(actual)
                || !wake_phrase_character_matches(actual, *expected)
            {
                matched = false;
                break;
            }
            consumed_end = index + actual.len_utf8();
        }
        let matched_len = phrase.len() - suffix_start;
        let followed_by_boundary = text[consumed_end..]
            .chars()
            .next()
            .is_some_and(is_embedded_audio_partial_preview_decorative);
        if matched && matched_len > 0 && (matched_len >= 2 || followed_by_boundary) {
            return Some(
                text[consumed_end..]
                    .trim_start_matches(is_embedded_audio_partial_preview_decorative)
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

/// The live provider may use a different first character for a wake phrase
/// that the local KWS and owner gate already accepted. Strip that bounded
/// cloud spelling only when the remaining phrase still matches and the next
/// character closes the phrase. The final two-pass result can then revise the
/// spelling without changing the body prefix already delivered to the app.
fn strip_bounded_near_activation_prefix(text: &str, phrase: &[char]) -> Option<String> {
    if phrase.len() < 4 {
        return None;
    }
    let mut chars = text.char_indices();
    let mut mismatches = 0;
    let mut consumed_end = 0;
    for (position, expected) in phrase.iter().enumerate() {
        let (index, actual) = chars.next()?;
        if is_embedded_audio_partial_preview_decorative(actual) {
            return None;
        }
        if !wake_phrase_character_matches(actual, *expected) {
            if position != 0 {
                return None;
            }
            mismatches += 1;
            if mismatches > 1 {
                return None;
            }
        }
        consumed_end = index + actual.len_utf8();
    }
    if mismatches != 1
        || !text[consumed_end..]
            .chars()
            .next()
            .is_some_and(is_embedded_audio_partial_preview_decorative)
    {
        return None;
    }
    Some(
        text[consumed_end..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .trim()
            .to_string(),
    )
}

fn strip_automatic_activation_prefix(text: &str, phrase: &str, partial: bool) -> String {
    let text = text.trim();
    let phrase = phrase
        .chars()
        .filter(|ch| !is_embedded_audio_partial_preview_decorative(*ch))
        .collect::<Vec<_>>();
    if text.is_empty() || phrase.is_empty() {
        return text.to_string();
    }

    // The authoritative pass can prepend a standalone hesitation to the wake
    // phrase (for example, "嗯，开始录音，正文").  Filler cleanup runs later
    // and must remain independently configurable, so consume only a bounded
    // leading run here and only commit that removal when the wake phrase (or
    // its already-established suffix) actually matches afterwards.
    let mut activation_candidate = text;
    loop {
        let trimmed = activation_candidate
            .trim_start_matches(is_embedded_audio_partial_preview_decorative);
        let filler_bytes = trimmed
            .char_indices()
            .take_while(|(_, ch)| matches!(ch, '嗯' | '呃' | '额' | '唔'))
            .map(|(index, ch)| index + ch.len_utf8())
            .last()
            .unwrap_or(0);
        if filler_bytes == 0 {
            activation_candidate = trimmed;
            break;
        }
        activation_candidate = &trimmed[filler_bytes..];
    }

    let mut phrase_index = 0usize;
    let mut consumed_end = 0usize;
    let mut matched_then_decorative = false;
    for (index, ch) in activation_candidate.char_indices() {
        let next = index + ch.len_utf8();
        if is_embedded_audio_partial_preview_decorative(ch) {
            if phrase_index > 0 {
                matched_then_decorative = true;
            }
            consumed_end = next;
            continue;
        }
        if phrase_index == phrase.len() {
            break;
        }
        if !wake_phrase_character_matches(ch, phrase[phrase_index]) {
            // Live 6599aae8: ASR wrote "开始。今天下午…" so the capsule and
            // insert kept 开始。 Full 开始录音 did not match. A 开始 + punct
            // prefix is still the wake remnant, not body text.
            if phrase_index >= 2 && matched_then_decorative {
                return keep_body_if_strip_emptied(
                    text,
                    phrase.len(),
                    activation_candidate[index..]
                        .trim_start_matches(is_embedded_audio_partial_preview_decorative)
                        .trim()
                        .to_string(),
                );
            }
            return keep_body_if_strip_emptied(
                text,
                phrase.len(),
                strip_bounded_near_activation_prefix(activation_candidate, &phrase)
                    .or_else(|| strip_activation_after_short_lead_in(activation_candidate, &phrase))
                    .or_else(|| strip_bounded_activation_suffix(activation_candidate, &phrase))
                    .unwrap_or_else(|| text.to_string()),
            );
        }
        phrase_index += 1;
        consumed_end = next;
        matched_then_decorative = false;
    }

    let stripped = if phrase_index == phrase.len() {
        activation_candidate[consumed_end..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .trim()
            .to_string()
    } else if partial && phrase_index > 0 {
        String::new()
    } else {
        text.to_string()
    };
    keep_body_if_strip_emptied(text, phrase.len(), stripped)
}

fn keep_body_if_strip_emptied(text: &str, phrase_len: usize, stripped: String) -> String {
    if stripped.trim().is_empty() {
        let core = compact_spoken_preview(text);
        if core.chars().count() > phrase_len + 2 {
            return text.to_string();
        }
    }
    stripped
}

fn clear_automatic_wake_text_guard(inner: &Arc<Inner>) {
    *inner.embedded_audio_automatic_wake_guard.lock() = None;
}

fn arm_automatic_wake_text_guard(
    inner: &Arc<Inner>,
    session_id: SessionId,
    phrase: String,
    capsule_audio_boundary_ms: u64,
) {
    let wait_for_visible_ack = inner.prefs.get().show_capsule
        && std::env::var("LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW")
            .ok()
            .as_deref()
            != Some("1");
    let body_wait_armed_immediately = !wait_for_visible_ack;
    *inner.embedded_audio_automatic_wake_guard.lock() = Some(AutomaticWakeGuard {
        session_id,
        phrase,
        activation_audio_end_ms: None,
        activation_confirmation_window_ms: None,
        activation_fragments: Vec::new(),
        activation_prefix_evidence: Vec::new(),
        latest_audio_ms: capsule_audio_boundary_ms,
        initial_body_wait_until_audio_ms: body_wait_armed_immediately.then_some(
            capsule_audio_boundary_ms.saturating_add(EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS),
        ),
        // Start a wall-clock escape even while waiting for the frontend ACK.
        // An early capsule can already be visible before the accepted session
        // installs this guard, so that ACK may legitimately never repeat.
        initial_body_wait_started_at: Some(Instant::now()),
        body_started: false,
        body_started_at: None,
        stop_requested: false,
    });
}

/// Install the accepted automatic-session guard without losing an early
/// candidate-capsule visibility edge. Candidate UI has an independent token
/// and never creates a product session; this boolean only transfers the known
/// visibility fact after the real owner session has been created.
fn arm_accepted_automatic_wake_text_guard(
    inner: &Arc<Inner>,
    session_id: SessionId,
    phrase: String,
    capsule_audio_boundary_ms: u64,
    early_capsule_was_visible: bool,
) {
    arm_automatic_wake_text_guard(
        inner,
        session_id,
        phrase,
        capsule_audio_boundary_ms,
    );
    if early_capsule_was_visible {
        acknowledge_automatic_wake_capsule_visible(inner, session_id);
        log::info!(
            "[wake-phrase] accepted automatic session inherited early capsule visibility session_id={session_id}"
        );
    }
}

/// Close the automatic wake text gate at the same logical boundary as the
/// transcribing feedback. Provider frames can still arrive after this point,
/// but they must not turn an empty wake-only capsule into a late body.
pub(super) fn mark_automatic_wake_stop_requested(
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
    if let Some(guard) = slot
        .as_mut()
        .filter(|guard| guard.session_id == session_id)
    {
        if !guard.stop_requested {
            guard.stop_requested = true;
            log::info!(
                "[wake-phrase] automatic wake text gate closed at stop boundary session_id={session_id} body_started={}",
                guard.body_started
            );
        }
    }
}

pub(super) fn acknowledge_automatic_wake_capsule_visible(
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
    let Some(guard) = slot
        .as_mut()
        .filter(|guard| guard.session_id == session_id)
    else {
        return;
    };
    if guard.initial_body_wait_until_audio_ms.is_some() {
        return;
    }
    guard.initial_body_wait_until_audio_ms = Some(
        guard
            .latest_audio_ms
            .saturating_add(EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS),
    );
    guard.initial_body_wait_started_at = Some(Instant::now());
    log::info!(
        "[wake-phrase] automatic body wait started from visible capsule session_id={session_id} audio_ms={}",
        guard.latest_audio_ms
    );
}

#[cfg(test)]
fn automatic_wake_initial_body_wait_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    audio_duration_ms: Option<u64>,
) -> bool {
    automatic_wake_initial_body_wait_snapshot_at(
        inner,
        session_id,
        audio_duration_ms,
        Instant::now(),
    )
    .0
}

#[cfg(test)]
fn automatic_wake_initial_body_wait_active_at(
    inner: &Arc<Inner>,
    session_id: SessionId,
    audio_duration_ms: Option<u64>,
    now: Instant,
) -> bool {
    automatic_wake_initial_body_wait_snapshot_at(
        inner,
        session_id,
        audio_duration_ms,
        now,
    )
    .0
}

/// Return the body-wait decision and its original monotonic origin from one
/// guard snapshot.  The endpoint reducer must use the same origin instead of
/// starting another three-second wait when the provider is silent.
fn automatic_wake_initial_body_wait_snapshot(
    inner: &Arc<Inner>,
    session_id: SessionId,
    audio_duration_ms: Option<u64>,
) -> (bool, Option<Instant>) {
    automatic_wake_initial_body_wait_snapshot_at(
        inner,
        session_id,
        audio_duration_ms,
        Instant::now(),
    )
}

fn automatic_wake_initial_body_wait_snapshot_at(
    inner: &Arc<Inner>,
    session_id: SessionId,
    audio_duration_ms: Option<u64>,
    now: Instant,
) -> (bool, Option<Instant>) {
    let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
    let Some(guard) = slot
        .as_mut()
        .filter(|guard| guard.session_id == session_id)
    else {
        return (false, None);
    };
    if let Some(audio_ms) = audio_duration_ms {
        guard.latest_audio_ms = guard.latest_audio_ms.max(audio_ms);
    }
    let audio_wait_active = guard
        .initial_body_wait_until_audio_ms
        .map(|deadline_ms| {
            audio_duration_ms
                .map(|audio_ms| audio_ms < deadline_ms)
                .unwrap_or(true)
        })
        .unwrap_or(true);
    let wall_wait_active = guard
        .initial_body_wait_started_at
        .map(|started_at| {
            now.saturating_duration_since(started_at)
                < Duration::from_millis(EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS)
        })
        .unwrap_or(true);
    // Real body releases the wake-only wait immediately. With no body, the
    // capsule acknowledgement arms the bounded audio/wall deadline.
    (
        !guard.body_started && audio_wait_active && wall_wait_active,
        guard.initial_body_wait_started_at,
    )
}

fn automatic_wake_session_active(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .is_some_and(|guard| guard.session_id == session_id)
}

fn automatic_wake_body_started(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .is_some_and(|guard| guard.session_id == session_id && guard.body_started)
}

fn bind_automatic_wake_audio_boundary(
    inner: &Arc<Inner>,
    session_id: SessionId,
    wake_end_seconds: f32,
    retained_audio_offset_bytes: usize,
) {
    if !wake_end_seconds.is_finite() || wake_end_seconds <= 0.0 { return; }
    let wake_end_ms = (wake_end_seconds as f64 * 1000.0).round() as u64;
    let offset_ms = retained_audio_offset_bytes as u64 / 32;
    let Some(relative_end_ms) = wake_end_ms.checked_sub(offset_ms) else { return; };
    if let Some(guard) = inner.embedded_audio_automatic_wake_guard.lock().as_mut()
        .filter(|guard| guard.session_id == session_id)
    {
        guard.activation_audio_end_ms = Some(relative_end_ms);
        log::info!("[wake-phrase] retained audio activation boundary bound session_id={session_id} activation_end_ms={relative_end_ms} retained_origin_ms={offset_ms}");
    }
}

fn bind_automatic_wake_confirmation_window(
    inner: &Arc<Inner>, session_id: SessionId,
    source_range: Option<crate::observability::CandidateRange>, retained_origin: Option<u64>,
) {
    let (Some(range), Some(origin)) = (source_range, retained_origin) else { return; };
    if range.end <= origin || range.start >= range.end { return; }
    let window = (range.start.saturating_sub(origin) / 32, (range.end - origin) / 32);
    if let Some(guard) = inner.embedded_audio_automatic_wake_guard.lock().as_mut()
        .filter(|guard| guard.session_id == session_id)
    {
        guard.activation_confirmation_window_ms = Some(window);
    }
}

fn note_automatic_wake_transcript_timing(
    inner: &Arc<Inner>,
    session_id: SessionId,
    timing: Option<crate::transcript_boundary::TimedTranscript>,
) {
    let Some(timing) = timing else { return; };
    let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
    let Some(guard) = slot.as_mut().filter(|guard| guard.session_id == session_id) else { return; };
    // A local recognizer can confirm the phrase without a KWS timestamp.
    // Recover that missing coordinate from exact, timed provider words inside
    // the very same confirmed PCM window, never from later body mentions.
    let confirmed = guard.activation_confirmation_window_ms
        .and_then(|window| timing.confirmed_activation_prefix(window, &guard.phrase, guard.activation_audio_end_ms));
    if confirmed.is_none() && guard.activation_audio_end_ms.is_none() {
        if let Some(window) = guard.activation_confirmation_window_ms {
            let phrase_key=crate::transcript_boundary::content_key(&guard.phrase);
            for fragment in timing.activation_fragments(window,&guard.phrase) {
                let completed=guard.activation_fragments.iter().any(|prior| {
                    prior.lead_key==fragment.lead_key && prior.end_ms<=fragment.start_ms
                        && format!("{}{}",prior.phrase_part,fragment.phrase_part)==phrase_key
                });
                if completed {
                    guard.activation_audio_end_ms=Some(fragment.end_ms);
                    log::info!("[wake-phrase] ordered provider fragments resolved confirmed activation boundary session_id={session_id} activation_end_ms={}",fragment.end_ms);
                    break;
                }
                if phrase_key.starts_with(&fragment.phrase_part)
                    && !guard.activation_fragments.iter().any(|prior| prior.lead_key==fragment.lead_key
                        && prior.phrase_part==fragment.phrase_part && prior.start_ms==fragment.start_ms && prior.end_ms==fragment.end_ms)
                {
                    if guard.activation_fragments.len()==16 {guard.activation_fragments.remove(0);}
                    guard.activation_fragments.push(fragment);
                }
            }
        }
    }
    let evidence = if let Some((end_ms, evidence)) = confirmed {
        if guard.activation_audio_end_ms.is_none() {
            log::info!("[wake-phrase] provider words resolved confirmed activation boundary session_id={session_id} activation_end_ms={end_ms}");
            guard.activation_audio_end_ms = Some(end_ms);
        }
        evidence
    } else {
        let Some(end_ms) = guard.activation_audio_end_ms else { return; };
        let Some(evidence) = timing.activation_prefix(end_ms, &guard.phrase) else { return; };
        evidence
    };
    guard.activation_prefix_evidence.retain(|prior| prior.source_key != evidence.source_key);
    // Keep bounded revision evidence: an early-sealed provider ledger may
    // select an older snapshot, while a later final can revise its preamble.
    if guard.activation_prefix_evidence.len() == 64 {
        guard.activation_prefix_evidence.remove(0);
    }
    guard.activation_prefix_evidence.push(evidence);
}

fn normalize_automatic_wake_final_candidate(
    inner: &Arc<Inner>,
    session_id: SessionId,
    candidate: Option<RawTranscript>,
    timing: Option<crate::transcript_boundary::TimedTranscript>,
) -> Option<RawTranscript> {
    note_automatic_wake_transcript_timing(inner, session_id, timing);
    candidate.map(|mut candidate| {
        candidate.text = filter_automatic_wake_text(inner, session_id, &candidate.text, false);
        candidate
    })
}

fn filter_automatic_wake_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
    partial: bool,
) -> String {
    resolve_automatic_wake_text(inner, session_id, text, partial).text
}

struct ResolvedAutomaticWakeText {
    text: String,
    /// Exact timed source evidence has located this session's activation.
    /// This is control-prefix provenance, not speaker or editor permission.
    activation_resolved: bool,
}

fn resolve_automatic_wake_text(
    inner: &Arc<Inner>, session_id: SessionId, text: &str, partial: bool,
) -> ResolvedAutomaticWakeText {
    let (phrase, timed_body, late_body_blocked, activation_pending) = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| guard.session_id == session_id)
        .map(|guard| {
            (
                Some(guard.phrase.clone()),
                guard.activation_prefix_evidence.iter().rev()
                    .find_map(|evidence| evidence.body(text).map(str::to_string)),
                guard.stop_requested && !guard.body_started,
                guard.activation_confirmation_window_ms.is_some() && guard.activation_audio_end_ms.is_none(),
            )
        })
        .unwrap_or((None, None, false, false));
    // Early provider callbacks may contain only the ambient preamble. They
    // cannot become immutable editor text while activation location is pending.
    if activation_pending && timed_body.is_none() {
        // A final can omit provider word coordinates. The independently
        // confirmed activation is still removable when it uniquely starts
        // that final; never authorize an untimed ambient preamble or a live
        // irreversible paste from this fallback.
        let final_starts_with_activation = !partial && phrase.as_deref().is_some_and(|phrase| {
            let phrase_key = crate::transcript_boundary::content_key(phrase);
            let source_key = crate::transcript_boundary::content_key(text);
            !phrase_key.is_empty() && source_key.starts_with(&phrase_key)
                && source_key.match_indices(&phrase_key).count() == 1
        });
        if !final_starts_with_activation {
            return ResolvedAutomaticWakeText { text: String::new(), activation_resolved: false };
        }
    }
    // Audio-aligned evidence is tied to this exact provider revision. Once it
    // identifies the control prefix, do not strip a later literal wake phrase
    // from the body a second time. Untimed revisions retain the legacy path.
    let body_phrase = if timed_body.is_some() { None } else { phrase.as_deref() };
    let activation_resolved = timed_body.is_some();
    let filtered = timed_body.unwrap_or_else(|| phrase.as_deref().map_or_else(
        || preserve_recording_transcript(text),
        |phrase| strip_automatic_activation_prefix(text, phrase, partial && !late_body_blocked),
    ));
    if late_body_blocked {
        // Live afd9e265: 28-char final arrived after stop with no visual
        // latch, then late_body returned empty and the spoken sentence was
        // swallowed. Keep a stripped body even after stop; only drop true
        // wake-only leftovers.
        if automatic_wake_filtered_text_is_body(&filtered, text, body_phrase) {
            latch_automatic_wake_body_if_filtered(
                inner,
                session_id,
                &filtered,
                text,
                body_phrase,
            );
            log::info!(
                "[wake-phrase] kept spoken body after stop before visual latch session_id={session_id} chars={}",
                filtered.chars().count()
            );
            return ResolvedAutomaticWakeText { text: filtered, activation_resolved };
        }
        log::debug!(
            "[wake-phrase] ignored late text after stop before body start session_id={session_id} partial={partial} chars={}",
            text.chars().count()
        );
        return ResolvedAutomaticWakeText { text: String::new(), activation_resolved };
    }
    latch_automatic_wake_body_if_filtered(inner, session_id, &filtered, text, body_phrase);
    ResolvedAutomaticWakeText { text: filtered, activation_resolved }
}

fn latch_automatic_wake_body_if_filtered(
    inner: &Arc<Inner>,
    session_id: SessionId,
    filtered: &str,
    original: &str,
    phrase: Option<&str>,
) {
    if !automatic_wake_filtered_text_is_body(filtered, original, phrase) {
        return;
    }
    let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
    if let Some(guard) = slot
        .as_mut()
        .filter(|guard| guard.session_id == session_id && !guard.body_started)
    {
        guard.body_started = true;
        guard.body_started_at = Some(Instant::now());
        log::info!(
            "[wake-phrase] automatic body started after capsule session_id={session_id}"
        );
    }
}

/// True when this automatic-wake session's body latch flipped within `within`.
/// The endpoint dispatch guard uses it to keep a just-started body from being
/// cut by an inactive deadline that was armed by the wake phrase itself
/// (r46f: stop proposed 40 ms before the first body preview became visible).
fn automatic_wake_body_started_recently(
    inner: &Arc<Inner>,
    session_id: SessionId,
    within: Duration,
) -> bool {
    inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .is_some_and(|guard| {
            guard.session_id == session_id
                && guard
                    .body_started_at
                    .is_some_and(|started_at| started_at.elapsed() < within)
        })
}

fn automatic_wake_text_counts_as_body(text: &str) -> bool {
    text.chars().any(|ch| {
        !ch.is_whitespace()
            && !is_embedded_audio_partial_preview_decorative(ch)
            && !matches!(ch, '嗯' | '呃' | '额' | '唔')
    })
}

fn compact_automatic_wake_comparable(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !is_embedded_audio_partial_preview_decorative(*ch)
                && !matches!(*ch, '嗯' | '呃' | '额' | '唔')
        })
        .collect()
}

fn automatic_wake_filtered_text_is_body(
    filtered: &str,
    original: &str,
    phrase: Option<&str>,
) -> bool {
    if !automatic_wake_text_counts_as_body(filtered) {
        return false;
    }
    let Some(phrase) = phrase else {
        return true;
    };
    let compact_filtered = compact_automatic_wake_comparable(filtered);
    let compact_phrase = compact_automatic_wake_comparable(phrase);
    if compact_filtered == compact_phrase {
        return false;
    }
    if filtered == original && compact_filtered.starts_with(&compact_phrase) {
        return compact_filtered.len() > compact_phrase.len();
    }
    true
}

fn is_dictation_filler_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, '嗯' | '呃' | '额' | '唔')
        && chars.all(|ch| matches!(ch, '嗯' | '呃' | '额' | '唔'))
}

fn collapsed_filler_gap(gap: &str, terminal: bool) -> String {
    let has_sentence_boundary = gap.chars().any(is_filler_sentence_boundary);
    let mut kept_pause = false;
    let mut result = String::new();
    for ch in gap.chars() {
        if is_filler_pause_separator(ch) {
            // Deleting a filler joins its two separator runs. A sentence end
            // wins over a comma; otherwise keep one pause. Do not collapse
            // quotes, brackets, line breaks, or combinations such as ?! / …….
            if !has_sentence_boundary && !kept_pause {
                result.push(ch);
                kept_pause = true;
            }
        } else if !ch.is_whitespace() || matches!(ch, '\n' | '\r') {
            result.push(ch);
        }
    }
    if result.is_empty() && !terminal && gap.chars().any(char::is_whitespace) {
        result.push(' ');
    }
    result
}

fn is_filler_pause_separator(ch: char) -> bool {
    matches!(ch, ',' | '，' | '、' | ';' | '；' | ':' | '：')
}

fn is_filler_sentence_boundary(ch: char) -> bool {
    matches!(ch, '.' | '。' | '?' | '？' | '!' | '！' | '…')
}

fn leading_filler_gap(gap: &str) -> String {
    gap.chars().filter(|ch| {
        !is_filler_pause_separator(*ch)
            && !is_filler_sentence_boundary(*ch)
            && (!ch.is_whitespace() || matches!(ch, '\n' | '\r'))
    }).collect()
}

fn remove_standalone_dictation_fillers(text: &str) -> String {
    let text = text.trim();
    if !text.chars().any(|ch| matches!(ch, '嗯' | '呃' | '额' | '唔')) {
        return text.to_string();
    }
    let mut output = String::new();
    let mut word = String::new();
    let mut gap = String::new();
    let mut have_retained_word = false;
    let mut filler_removed_in_gap = false;

    let flush_word = |word: &mut String,
                      gap: &mut String,
                      output: &mut String,
                      have_retained_word: &mut bool,
                      filler_removed_in_gap: &mut bool| {
        if word.is_empty() {
            return;
        }
        if is_dictation_filler_word(word) {
            *filler_removed_in_gap = true;
            word.clear();
            return;
        }
        if *have_retained_word {
            if *filler_removed_in_gap {
                output.push_str(&collapsed_filler_gap(gap, false));
            } else {
                output.push_str(gap);
            }
        } else if *filler_removed_in_gap {
            output.push_str(&leading_filler_gap(gap));
        } else {
            output.push_str(gap);
        }
        output.push_str(word);
        *have_retained_word = true;
        *filler_removed_in_gap = false;
        gap.clear();
        word.clear();
    };

    for ch in text.chars() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            flush_word(
                &mut word,
                &mut gap,
                &mut output,
                &mut have_retained_word,
                &mut filler_removed_in_gap,
            );
            gap.push(ch);
        } else {
            word.push(ch);
        }
    }
    flush_word(
        &mut word,
        &mut gap,
        &mut output,
        &mut have_retained_word,
        &mut filler_removed_in_gap,
    );
    if have_retained_word {
        if filler_removed_in_gap {
            output.push_str(&collapsed_filler_gap(&gap, true));
        } else {
            output.push_str(&gap);
        }
    }
    let trimmed = output.trim();
    strip_inlined_chinese_filler_runs(trimmed)
}

/// 从连续中文文本里剥离被汉字夹住的犹豫语气词串(嗯/呃/唔)。
///
/// [`remove_standalone_dictation_fillers`] 只删被标点/空白分隔的"独立"语气词;
/// 但中文 ASR 常输出无标点的连续文本(如"今天嗯去测试"),语气词粘连在正文中间,
/// standalone 检测不到——开关看上去就"没用"。这里把被汉字夹住的嗯/呃/唔串删掉:
/// 只要语气词串任一侧紧邻普通字(非标点/空白)即视为粘连,予以删除;两侧都是分隔
/// 符的留给 standalone,避免重复处理。
///
/// 只针对嗯/呃/唔:它们在普通话里几乎只作语气词,不会出现在实义词内部。`额`
/// 故意不在此剥离——它有"额外/金额/名额"等实义用法,粘连删除会误伤,只由
/// standalone 路径(被标点分隔的独立"额")处理。
const INLINE_CHINESE_FILLER_CHARS: &[char] = &['嗯', '呃', '唔'];

fn strip_inlined_chinese_filler_runs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let mut keep = vec![true; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if !INLINE_CHINESE_FILLER_CHARS.contains(&chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && INLINE_CHINESE_FILLER_CHARS.contains(&chars[i]) {
            i += 1;
        }
        let end = i; // 语气词串占据 [start, end)
        let left_is_word =
            start > 0 && !is_embedded_audio_partial_preview_decorative(chars[start - 1]);
        let right_is_word =
            end < chars.len() && !is_embedded_audio_partial_preview_decorative(chars[end]);
        // 任一侧紧邻普通字 → 粘连在正文里,删除;两侧都是分隔符 → 交给 standalone。
        if left_is_word || right_is_word {
            for j in start..end {
                keep[j] = false;
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    for (idx, ch) in chars.iter().enumerate() {
        if keep[idx] {
            out.push(*ch);
        }
    }
    out.trim().to_string()
}

fn filter_dictation_preview_text(inner: &Arc<Inner>, session_id: SessionId, text: &str) -> String {
    let text = filter_automatic_wake_text(inner, session_id, text, true);
    if inner.prefs.get().remove_filler_words {
        remove_standalone_dictation_fillers(&text)
    } else {
        text
    }
}

fn filter_dictation_visual_preview_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
) -> String {
    // Strip the wake prefix before latching visible body so a provisional body
    // cannot expire as a wake-only session. Final delivery separately requires
    // the authoritative preview; this visual path does not supply final text.
    let filtered = filter_automatic_wake_text(inner, session_id, text, true);
    let text = if filtered.is_empty() {
        reversible_automatic_wake_visual_body(inner, session_id, text).unwrap_or(filtered)
    } else {
        filtered
    };
    // Filler cleanup belongs on the sealed insert, not the live capsule.
    // Stripping 嗯/呃 from short partials delayed the first visible body
    // and made the preview look like it was swallowing text.
    text
}

/// Reveal an exact leading activation to the reversible capsule while its
/// provider word coordinates are still pending. The confirmed local PCM window
/// proves that this session contains the activation, while the exact textual
/// prefix keeps an ambient lead-in from being treated as body. This result is
/// used only by `update_embedded_audio_visual_preview`: it does not latch body,
/// advance the authoritative preview, or enter the delivery ledger.
fn reversible_automatic_wake_visual_body(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
) -> Option<String> {
    let phrase = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| {
            guard.session_id == session_id
                && guard.activation_confirmation_window_ms.is_some()
                && guard.activation_audio_end_ms.is_none()
                && !guard.stop_requested
        })
        .map(|guard| guard.phrase.clone())?;
    exact_leading_activation_body(text, &phrase)
}

fn exact_leading_activation_body(text: &str, phrase: &str) -> Option<String> {
    let text = text.trim();
    let phrase = phrase.trim();
    if text.is_empty() || phrase.is_empty() {
        return None;
    }

    let mut text_chars = text.char_indices();
    let mut consumed_end = 0usize;
    let mut matched_content = false;
    for expected in phrase
        .chars()
        .filter(|ch| !is_embedded_audio_partial_preview_decorative(*ch))
    {
        let (index, actual) = loop {
            let next = text_chars.next()?;
            if !is_embedded_audio_partial_preview_decorative(next.1) {
                break next;
            }
        };
        if actual != expected {
            return None;
        }
        matched_content = true;
        consumed_end = index + actual.len_utf8();
    }
    if !matched_content {
        return None;
    }

    let body = text[consumed_end..]
        .trim_start_matches(is_embedded_audio_partial_preview_decorative)
        .trim();
    (!body.is_empty()).then(|| body.to_string())
}


fn emit_embedded_audio_partial_preview_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    preview: String,
) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::AsrPartial {
            session_id,
            after_stop: embedded_audio_stop_feedback_latched(inner),
        },
        current_embedded_audio_capsule_level(inner),
        Some(preview),
        None,
    )
}

fn emit_embedded_audio_pcm_capsule_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    capsule_state: CapsuleState,
    level: f32,
) -> bool {
    let after_stop = match capsule_state {
        CapsuleState::Recording => false,
        CapsuleState::Transcribing => true,
        _ => return false,
    };
    let level = remember_embedded_audio_capsule_level(inner, level);
    // PCM is a high-frequency level tick. The frontend retains the last ASR
    // message when this field is absent, so do not clone and retransmit the
    // complete preview for every audio packet (long dictation UI stall).
    if embedded_ble_actor_context_active(inner) {
        let trace_timeline = should_trace_embedded_ble_pcm_capsule(inner, session_id, after_stop);
        apply_embedded_ble_session_actor_dictation_event_with_trace(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            session_id,
            format!("pcm_capsule after_stop={after_stop}"),
            trace_timeline,
            DictationEvent::BlePcm {
                session_id,
                after_stop,
            },
            level,
            None,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::BlePcm {
                session_id,
                after_stop,
            },
            level,
            None,
            None,
        )
    }
}

fn emit_embedded_audio_transcribing_if_active(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: Option<String>,
) -> bool {
    if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            session_id,
            "transcribing feedback after stop boundary",
            DictationEvent::BleStop { session_id },
            0.0,
            message,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::BleStop { session_id },
            0.0,
            message,
            None,
        )
    }
}

fn embedded_audio_streaming_session_accepts_pcm(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.state.lock();
    state.session_id == session_id && !state.cancelled && state.phase == SessionPhase::Listening
}

pub(super) fn request_embedded_audio_stop_feedback(
    inner: &Arc<Inner>,
    _reason: &'static str,
) -> bool {
    let session_id = {
        let state = inner.state.lock();
        if state.phase != SessionPhase::Listening {
            return false;
        }
        state.session_id
    };
    latch_embedded_audio_stop_feedback(inner, session_id);
    emit_embedded_audio_transcribing_if_active(
        inner,
        session_id,
        current_embedded_audio_visual_preview(inner),
    )
}

// ───────────────── pause-early-delivery (2026-09-22 跟手①) ─────────────────
// 设计卡：work/pause-early-delivery-design-20260922.md。主人账本前缀稳定
// ≥ 耐心窗 → 稳定前缀当场走插入通道上屏；会话不结束，端点契约原样保留；
// 终稿只插余量（stability-key 前缀比对；云端改写前缀 → 跳过第二次插入，
// 宁可少不可重——H 族教训）。回退开关 LISTENER_DISABLE_PAUSE_EARLY_DELIVERY=1。

const PAUSE_EARLY_DELIVERY_MIN_STABLE: Duration =
    Duration::from_millis(1_000);
// When ASR omits punctuation, wait until both the owner ledger and the
// capsule's live text stop growing. Local VAD frequently follows background
// noise even through a human pause, so its speech edge alone is not a safe
// clause boundary on this device.
const PAUSE_EARLY_UNPUNCTUATED_QUIET: Duration = Duration::from_millis(800);
// Legacy rolling-stable prefixes can still be provider placeholders. In
// installed 1.0.6 session 3e8ab1c5 it was pasted, then the provider rewrote
// the opening and final reconciliation could not safely retract it. Keep such
// fragments in the reversible capsule. Complete spoken clauses can be
// shorter than unpunctuated fragments: sessions a89b39ce and da86f71c held
// stable "然后呢？" and "我没有懂，" until much later sentences.
const PAUSE_EARLY_FIRST_CHUNK_MIN_CONTENT_CHARS: usize = 6;
const PAUSE_EARLY_FIRST_COMPLETE_CLAUSE_MIN_CONTENT_CHARS: usize = 4;
const PAUSE_EARLY_FIRST_COMPLETE_SENTENCE_MIN_CONTENT_CHARS: usize = 3;
const PAUSE_EARLY_NEXT_CHUNK_MIN_CONTENT_CHARS: usize = 3;

/// Paste only a stable, punctuated clause. The capsule continues showing the
/// reversible live preview while this segment is still being spoken. Taking
/// the last boundary batches provider revisions into one insertion instead
/// of pasting every three newly stable characters.
fn pause_early_complete_clause(delta: &str) -> Option<&str> {
    let mut boundary = None;
    let mut previous = None;
    for (offset, ch) in delta.char_indices() {
        let next = delta[offset + ch.len_utf8()..].chars().next();
        let numeric_separator = matches!(ch, '.' | ',')
            && previous.is_some_and(|prev: char| prev.is_ascii_digit())
            && next.is_some_and(|following| following.is_ascii_digit());
        if !numeric_separator
            && matches!(ch, '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '；' | ';')
        {
            boundary = Some(offset + ch.len_utf8());
        }
        previous = Some(ch);
    }
    let mut end = boundary?;
    for ch in delta[end..].chars() {
        if matches!(ch, '”' | '’' | '"' | '\'' | '）' | ')' | '】' | ']') {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    Some(&delta[..end])
}

fn pause_early_segment(delta: &str, quiet_boundary: bool) -> Option<&str> {
    if quiet_boundary {
        Some(delta)
    } else {
        pause_early_complete_clause(delta)
    }
}

/// Native clause/word confirmation already supplies the lexical boundary.
/// Keep the old batching heuristics only for legacy rolling-stable views;
/// otherwise short or unpunctuated confirmed speech waits for STOP despite
/// having satisfied the ASR delivery contract. This cannot grant ownership,
/// invent a source seam, or advance the editor receipt clock.
fn pause_early_ready_segment<'a>(
    delivered_display: &str,
    delivered_key: &str,
    remainder: &'a str,
    quiet_boundary: bool,
    stability: crate::asr::volcengine::LiveDeliveryStability,
) -> Option<&'a str> {
    if stability == crate::asr::volcengine::LiveDeliveryStability::ProviderConfirmed
        && !embedded_audio_partial_preview_stability_key(remainder).is_empty()
    {
        return Some(remainder);
    }
    let delta = pause_early_segment(remainder, quiet_boundary)?;
    (pause_early_chunk_ready(delivered_key, delta)
        || pause_early_short_terminal_completion_ready(delivered_display, delta, true))
        .then_some(delta)
}

/// An unpunctuated owner suffix is complete only when the owner-approved
/// snapshot has caught up to the text the user can currently see. The visible
/// preview may be provisional, so equality is only a completeness check: it
/// never supplies speaker authority to `snapshot_text`. A longer/different
/// visible preview (including a foreign tail) keeps the suffix reversible.
fn pause_early_snapshot_covers_visible_body(
    snapshot_text: &str,
    visible_text: Option<&str>,
) -> bool {
    let snapshot_key = embedded_audio_partial_preview_stability_key(snapshot_text);
    let visible_key = visible_text
        .map(embedded_audio_partial_preview_stability_key)
        .unwrap_or_default();
    !snapshot_key.is_empty() && snapshot_key == visible_key
}

/// A stable snapshot can trail a newer owner-approved provider revision while
/// the capsule trails by the same amount. Their equality does not make that
/// shorter prefix complete. Querying after the snapshot's source body key
/// keeps this check before filler, script and correction display transforms.
/// `None` grants no authority: the stable snapshot and visible-body checks
/// above still provide the required evidence.
fn pause_early_snapshot_has_no_newer_owner_body(
    pending_after_snapshot: Option<&str>,
) -> bool {
    pending_after_snapshot.is_none()
}

fn pause_early_chunk_ready(delivered_key: &str, delta: &str) -> bool {
    // Stable words already crossed the irreversible delivery boundary. Their
    // later terminal punctuation is decoration of that same body, not a new
    // short provider placeholder to hold until finalization.
    if !delivered_key.is_empty() && pause_early_terminal_punctuation_only(delta) {
        return true;
    }
    let count = embedded_audio_partial_preview_stability_key(delta)
        .chars()
        .count();
    count >= if delivered_key.is_empty() {
        if pause_early_complete_clause(delta) == Some(delta) {
            let sentence_end = delta
                .trim_end_matches(|ch| matches!(ch, '”' | '’' | '"' | '\'' | '）' | ')' | '】' | ']'))
                .chars()
                .last()
                .is_some_and(|ch| matches!(ch, '。' | '！' | '!' | '？' | '?'));
            if sentence_end {
                PAUSE_EARLY_FIRST_COMPLETE_SENTENCE_MIN_CONTENT_CHARS
            } else {
                PAUSE_EARLY_FIRST_COMPLETE_CLAUSE_MIN_CONTENT_CHARS
            }
        } else {
            PAUSE_EARLY_FIRST_CHUNK_MIN_CONTENT_CHARS
        }
    } else {
        PAUSE_EARLY_NEXT_CHUNK_MIN_CONTENT_CHARS
    }
}

fn pause_early_short_terminal_completion_ready(
    delivered_display: &str,
    delta: &str,
    source_boundary_available: bool,
) -> bool {
    if !source_boundary_available || delivered_display.trim().is_empty() {
        return false;
    }
    let ends_sentence = |text: &str| {
        text.trim_end_matches(|ch: char| {
            matches!(ch, '”' | '’' | '"' | '\'' | '）' | ')' | '】' | ']')
        })
        .chars()
        .last()
        .is_some_and(|ch| matches!(ch, '。' | '！' | '!' | '？' | '?'))
    };
    if pause_early_complete_clause(delivered_display) == Some(delivered_display)
        || !ends_sentence(delta)
    {
        return false;
    }
    let content_chars = embedded_audio_partial_preview_stability_key(delta)
        .chars()
        .count();
    (1..PAUSE_EARLY_NEXT_CHUNK_MIN_CONTENT_CHARS).contains(&content_chars)
        && pause_early_complete_clause(delta) == Some(delta)
}

fn pause_early_terminal_punctuation_only(delta: &str) -> bool {
    crate::transcript_boundary::terminal_punctuation_only(delta)
}

/// Session-scoped bookkeeping of text already inserted mid-session.
#[derive(Default)]
pub(super) struct PauseEarlyDeliveryLedger {
    pub(super) session_id: Option<SessionId>,
    /// Sample credentials once per recording, outside the watchdog's hot path.
    /// Outer None means unobserved; Some(None) preserves a lookup failure.
    pub(super) llm_auth_fingerprint: Option<Option<u64>>,
    /// Exact display text already on screen for this session.
    pub(super) delivered_display: String,
    /// stability key（去标点小写）of `delivered_display`.
    pub(super) delivered_key: String,
    /// Prefix in the rendered provider snapshot, used only for display
    /// reconciliation. It is not the raw ASR or audio consumption coordinate.
    pub(super) source_key: String,
    /// Raw provider consumption is committed with the same successful
    /// transaction. A reservation cannot advance it, and rollback retains it.
    pub(super) committed_body_consumption: Option<crate::asr::volcengine::LiveBodyConsumption>,
    pub(super) reserved_body_consumption: Option<crate::asr::volcengine::LiveBodyConsumption>,
    /// Only the last successful transport submission may establish a timed seam.
    /// Like source_key, this is not proof of an editor receipt; receipt ownership
    /// and the three-second clock remain in SettledTargetEndpointClock.
    /// A reserve leaves it intact; source/session checks prevent borrowing it
    /// until rollback restores that same source coordinate.
    pub(super) confirmed_timed_boundary: Option<(String, crate::transcript_boundary::TimedDeliveryBoundary, crate::asr::volcengine::LiveDeliverySource)>,
    /// Raw prefix belonging to that successful timed submission, including the
    /// original wake prefix. Never rebuilt from later provider revisions.
    pub(super) confirmed_source_text: Option<String>,
    pub(super) delivery_generation: u64,
    /// The state before the newest unconfirmed reserve. Rollback restores
    /// this instead of wiping the ledger: a failed mid-session paste must not
    /// make the final dispatch forget (and re-paste) text that is already on
    /// screen — 2026-09-23 用户实锤"一毛一样粘贴两次"。
    pub(super) unconfirmed_prior: Option<(String, String, String)>,
    /// Sticky proof this session delivered early text at least once. Never
    /// cleared by rollback; the final dispatch uses it as a hard floor — when
    /// set, a full-text re-paste is cancelled even if the reconcilable prefix
    /// was lost (宁可少不可重——H 族).
    pub(super) ever_delivered_session: Option<SessionId>,
    /// A submitted early chunk used the clipboard paste transport. The final
    /// remainder must continue on that route instead of waiting for a TSF
    /// client that did not deliver this session's preceding chunks.
    pub(super) paste_delivered_session: Option<SessionId>,
    /// 一次性门诊断：同一会话同一原因只记一行，防止看门狗 20/s 刷屏。
    pub(super) gate_blocked_logged: Option<(SessionId, &'static str)>,
    /// 2026-09-23 tkg 后诊断:tick 首达行(证明看门狗路径活着+当时门状态)。
    pub(super) tick_diag_logged: Option<SessionId>,
    /// 诊断:快照首次合格(此后若仍无交付,问题在显示门/焦点/粘贴下游)。
    pub(super) qualified_diag_logged: Option<SessionId>,
    /// 诊断:三条件看似全满足却拿不到快照=快照侧还有隐藏门,一锤定音。
    pub(super) contradictory_diag_logged: Option<SessionId>,
}

/// 停顿落屏被门挡下时的判读行（每会话每原因至多一次）。
fn pause_early_note_gate_blocked(
    inner: &Arc<Inner>,
    session_id: SessionId,
    reason: &'static str,
) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.gate_blocked_logged == Some((session_id, reason)) {
        return;
    }
    ledger.gate_blocked_logged = Some((session_id, reason));
    log::info!(
        "[coord] pause-early-delivery gate blocked session_id={session_id} reason={reason}"
    );
}

/// 终稿尚未上屏的余量。`Some("")` = 前缀已覆盖全部内容；`None` = 云端改写/
/// 收缩了已交付前缀（余量不可安全切分，调用方须跳过第二次插入）。
/// 与 `embedded_audio_partial_preview_stability_key` 同口径逐字推进。
fn pause_early_boundary_tail<'a>(tail: &'a str, delivered_display: &str) -> &'a str {
    let tail = tail.trim_start();
    // The content coordinate omits decoration, but the editor coordinate
    // owns its complete boundary run. Removing only one separator makes a
    // source such as "body.." offer the second dot on every watchdog tick.
    // Match the entire already-submitted suffix, including closing delimiters
    // and literal punctuation. Rollback restores this same editor coordinate.
    let consumed: Vec<char> = delivered_display
        .chars()
        .rev()
        .take_while(|ch| is_embedded_audio_partial_preview_decorative(*ch))
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let incoming: Vec<(usize, char)> = tail
        .char_indices()
        .take_while(|(_, ch)| is_embedded_audio_partial_preview_decorative(*ch))
        .filter(|(_, ch)| !ch.is_whitespace())
        .collect();
    let covered = (1..=consumed.len().min(incoming.len()))
        .rev()
        .find(|count| {
            consumed[..*count].iter().rev().zip(&incoming[..*count]).all(
                |(previous, (_, next))| previous == next
                    || (pause_early_is_boundary_separator(*previous)
                        && pause_early_is_boundary_separator(*next)),
            )
        })
        .unwrap_or(0);
    let end = covered.checked_sub(1)
        .map(|last| incoming[last].0 + incoming[last].1.len_utf8())
        .unwrap_or(0);
    tail[end..].trim_start()
}

fn pause_early_is_boundary_separator(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。' | '、' | '；' | '：' | '？' | '！' | ',' | '.' | ';' | ':' | '?' | '!'
    )
}

fn pause_early_final_remainder(
    final_text: &str,
    delivered_display: &str,
    delivered_key: &str,
) -> Option<String> {
    if delivered_key.is_empty() {
        return Some(final_text.trim().to_string());
    }
    let mut seen_key = String::new();
    for (offset, ch) in final_text.char_indices() {
        if seen_key == delivered_key {
            return Some(pause_early_boundary_tail(&final_text[offset..], delivered_display).to_string());
        }
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            seen_key.push(lower);
        }
        if !delivered_key.starts_with(seen_key.as_str()) {
            return None;
        }
    }
    if seen_key == delivered_key {
        Some(String::new())
    } else {
        None
    }
}

/// A final can revise earlier words without adding speech beyond the consumed
/// boundary. Distinguish that from an unknown tail. A unique terminal anchor
/// and a bounded full-text alignment certify the same boundary; only its
/// punctuation may remain. Committed editor words are never replaced.
fn pause_early_revised_terminal_remainder(
    final_text: &str,
    delivered_display: &str,
    source_key: &str,
) -> Option<String> {
    let tail = crate::transcript_boundary::revised_terminal_suffix(final_text, source_key)?;
    Some(pause_early_boundary_tail(tail, delivered_display).to_string())
}

/// A live provider revision may correct words inside an already pasted
/// segment. Continue delivering only when the end of that segment still has
/// one unambiguous position near its old boundary. This is stricter than the
/// final recovery path: an uncertain live boundary can wait for finalization.
fn pause_early_anchored_continuation(
    text: &str,
    delivered_display: &str,
    delivered_key: &str,
) -> Option<String> {
    let delivered: Vec<char> = delivered_key.chars().collect();
    if delivered.len() < 8 {
        return None;
    }
    let mut content = Vec::new();
    let mut offsets = Vec::new();
    for (offset, ch) in text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            content.push(lower);
            offsets.push(offset);
        }
    }
    if content.len() <= delivered.len() {
        return None;
    }
    let shared_prefix = delivered
        .iter()
        .zip(&content)
        .take_while(|(a, b)| a == b)
        .count();
    let anchor = &delivered[delivered.len() - 8..];
    // A two-pass provider revision can correct the opening from its first
    // character while retaining a unique seam beside the paste boundary.
    // Requiring an unchanged opening stopped live delivery for the rest of
    // session 67577453. With no shared opening, use a wider but unique seam.
    let tolerance = if shared_prefix < 2 { 4 } else { 2 };
    let boundary_low = delivered.len().saturating_sub(tolerance);
    let boundary_high = delivered.len() + tolerance;
    let mut matched_boundary = None;
    let mut anchor_occurrences = 0;
    for start in 0..=content.len() - anchor.len() {
        let after = start + anchor.len();
        if content[start..after] != *anchor {
            continue;
        }
        anchor_occurrences += 1;
        if after < boundary_low || after > boundary_high {
            continue;
        }
        if matched_boundary.replace(after).is_some() {
            // A repeated suffix close to the paste boundary cannot establish
            // which occurrence is already on screen.
            return None;
        }
    }
    if shared_prefix < 2 && anchor_occurrences != 1 {
        return None;
    }
    let after = matched_boundary?;
    if after >= content.len() {
        return None;
    }
    let boundary = offsets[after - 1]
        + text[offsets[after - 1]..]
            .chars()
            .next()
            .expect("content offset points at a character")
            .len_utf8();
    let tail = pause_early_boundary_tail(&text[boundary..], delivered_display).to_string();
    (!embedded_audio_partial_preview_stability_key(&tail).is_empty()).then_some(tail)
}

/// The stop result may revise an already committed clause. Count content
/// characters from the committed boundary and append only genuine growth
/// beyond it. Never append the rewritten portion of the committed prefix.
/// Deep revisions remain ambiguous and are left on screen as originally
/// committed, while the exact and anchored paths handle clearer tails first.
fn pause_early_mismatch_recovery_tail(final_text: &str, delivered_key: &str) -> Option<String> {
    if delivered_key.is_empty() {
        return None;
    }
    let delivered_content_chars = delivered_key.chars().count();
    let mut seen_key = String::new();
    let mut diverged = false;
    let mut lcp_chars = 0usize;
    let mut content_index = 0usize;
    // 终稿内容第 delivered_content_chars 个字符的字节偏移：增长尾从这里切齐。
    let mut growth_offset: Option<usize> = None;
    for (offset, ch) in final_text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        if growth_offset.is_none() && content_index == delivered_content_chars {
            growth_offset = Some(offset);
        }
        for lower in ch.to_lowercase() {
            seen_key.push(lower);
        }
        if !diverged && !delivered_key.starts_with(seen_key.as_str()) {
            diverged = true;
        } else if !diverged {
            lcp_chars += 1;
        }
        content_index += 1;
    }
    if !diverged || content_index <= delivered_content_chars {
        return None;
    }
    // A length increase can be an insertion inside the old sentence. If the
    // committed ending still closes the stop result, there is no evidence of
    // speech after the committed boundary (and appending would repeat it).
    let delivered_end: Vec<char> = delivered_key.chars().rev().take(3).collect();
    if delivered_end.len() == 3
        && seen_key.chars().rev().take(3).eq(delivered_end.into_iter())
    {
        return None;
    }
    // Small growth (≤3 content chars) can still carry a real tail after a
    // deep provider revision. The unchanged-ending guard above rejects an
    // insertion inside the already committed clause.
    let growth_chars = content_index.saturating_sub(delivered_content_chars);
    if growth_chars > 0 && growth_chars <= 3 {
        return growth_offset.map(|offset| seam_deduped_growth_tail(final_text, offset, delivered_key));
    }
    if lcp_chars * 2 < delivered_content_chars {
        return None;
    }
    let rewritten_tail_chars = delivered_content_chars - lcp_chars;
    // 2026-09-23 14:31/14:32 连续实锤:云端两遍精修润色了中段一个字,旧契约
    // 把 2-16 字的纯新增尾巴一起丢掉——违反优先级锁第 3 条(不吞你的字)。
    // Longer growth with a small revision near the old boundary can still
    // append the content beyond that boundary. A deep restatement stays
    // ambiguous and is not appended.
    if content_index > delivered_content_chars
        && rewritten_tail_chars <= usize::max(8, delivered_content_chars / 3)
    {
        return growth_offset.map(|offset| seam_deduped_growth_tail(final_text, offset, delivered_key));
    }
    None
}

/// 增长尾接缝去重(2026-09-23 15:4x 测试实锤):改写区变长或重排时,按索引
/// 切出的尾巴开头可能复述已交付的结尾(尾部"别的呀"叠在屏上"…别的"后面
/// 成"别的别的呀")。尾巴前 ≤2 字若与已交付 key 的结尾逐字相同则剥掉,
/// 剥空则放弃——宁少不重复。
fn seam_deduped_growth_tail(final_text: &str, offset: usize, delivered_key: &str) -> String {
    let tail: Vec<char> = final_text[offset..].chars().collect();
    let key: Vec<char> = delivered_key.chars().collect();
    for strip in (1..=2usize).rev() {
        if tail.len() <= strip || key.len() < strip {
            continue;
        }
        let overlaps = (0..strip).all(|i| {
            tail[i]
                .to_lowercase()
                .eq(key[key.len() - strip + i].to_lowercase())
        });
        if overlaps {
            let remainder: String = tail[strip..].iter().collect();
            if !remainder.trim_matches(|ch: char| is_embedded_audio_partial_preview_decorative(ch)).is_empty() {
                return remainder;
            }
        }
    }
    final_text[offset..].to_string()
}

/// 2026-09-23 17:31 d37e3562 实锤"吞尾部"：原始终稿(54字含唤醒词)完整
/// 含尾"然后我现在继续说话"，但已交付是流式中间态(58字，含临时膨胀)，
/// 润色终稿又压到 47 字——长度比较与 LCP 在两个失真域里全部判"改写，
/// 跳过"，唯一没上屏的真尾巴被丢。长度不可比时改用内容锚点：取已交付
/// 末尾锚串在原始终稿归一化内容里的最后一次出现，锚后还有内容即未
/// 交付尾巴。锚串从 8 字起，找不到降 4 字（太长经不起两遍精修，太短
/// 易假锚）。锚点必须落在已交付覆盖范围内（假锚会把已交付内容再贴
/// 一遍）；找不到锚或锚后为空 → None，维持跳过（宁少勿重）。
fn pause_early_tail_beyond_delivered_anchor(
    final_text: &str,
    delivered_key: &str,
) -> Option<String> {
    if delivered_key.is_empty() {
        return None;
    }
    let delivered_chars: Vec<char> = delivered_key.chars().collect();
    // 原始终稿的归一化内容串 + 每个内容字符在原文的字节偏移（切尾巴用）。
    let mut norm: Vec<char> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    for (offset, ch) in final_text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            norm.push(lower);
            offsets.push(offset);
        }
    }
    // A two-pass correction can rewrite the opening from its first character
    // while leaving an eight-character seam intact near the delivered end.
    // Session 67577453 lost 56 owner characters because the old opening-only
    // guard rejected that unique seam. A four-character seam still requires
    // an unchanged opening; an eight-character seam must be unique and near
    // the already delivered boundary.
    let shared_prefix = delivered_chars
        .iter()
        .zip(norm.iter())
        .take_while(|(delivered, sealed)| delivered == sealed)
        .count();
    for &anchor_len in &[8usize, 4] {
        if shared_prefix < 2 && anchor_len < 8 {
            continue;
        }
        if delivered_chars.len() < anchor_len || norm.len() < anchor_len {
            continue;
        }
        let anchor: &[char] = &delivered_chars[delivered_chars.len() - anchor_len..];
        // 最后一次出现：取最晚锚点让尾巴最小，重复上屏风险最低。
        let mut head: isize = norm.len() as isize - anchor_len as isize;
        let mut anchor_at: Option<usize> = None;
        let mut matches = 0usize;
        while head >= 0 {
            let start = head as usize;
            if norm[start..start + anchor_len] == *anchor {
                matches += 1;
                if anchor_at.is_none() {
                    anchor_at = Some(start);
                }
            }
            head -= 1;
        }
        let Some(start) = anchor_at else {
            continue;
        };
        let after = start + anchor_len;
        if shared_prefix < 2
            && (matches != 1 || after.abs_diff(delivered_chars.len()) > 4)
        {
            continue;
        }
        // 锚后无新内容 = 已交付覆盖到终稿末尾，无尾可补（也排除末尾重复）。
        if after >= norm.len() {
            continue;
        }
        // 假锚卫兵：锚点末尾必须仍在已交付覆盖范围内（允许 2 字归一化
        // 损耗）。落在覆盖范围外的早锚意味着前文重复短语，补了必重贴。
        if after > delivered_chars.len() + 2 {
            continue;
        }
        let tail = final_text[offsets[after]..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .to_string();
        if !tail
            .trim_matches(|ch: char| is_embedded_audio_partial_preview_decorative(ch))
            .is_empty()
        {
            log::info!(
                "[coord] pause-early anchor tail recovered delivered_chars={} final_chars={} tail_chars={}",
                delivered_chars.len(),
                norm.len(),
                tail.chars().count()
            );
            return Some(tail);
        }
    }
    None
}

/// Recover a genuinely new tail when cloud ASR changed both the middle and the
/// last few characters of an early paste. Exact suffix anchors cannot locate
/// the boundary in that case. Align the delivered content with a prefix of the
/// final content, allowing a small number of recognition edits, then append
/// only content after that boundary. A large rewrite remains ambiguous and is
/// left to the existing no-duplicate fallback.
fn pause_early_aligned_growth_tail(final_text: &str, delivered_key: &str) -> Option<String> {
    if let Some(end) = crate::transcript_boundary::revised_prefix_end(final_text, delivered_key) {
        return Some(final_text[end..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .to_string());
    }
    let delivered: Vec<char> = delivered_key.chars().collect();
    let len = delivered.len();
    if len < 8 || len > 512 {
        return None;
    }
    let mut final_content = Vec::new();
    let mut offsets = Vec::new();
    for (offset, ch) in final_text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            final_content.push(lower);
            offsets.push(offset);
        }
    }
    // The existing micro-growth path handles 1-3 new characters. This path
    // needs substantial growth to distinguish a continuation from a rewrite.
    if final_content.len() < len + 4
        || delivered
            .iter()
            .zip(&final_content)
            .take_while(|(left, right)| left == right)
            .count()
            < 2
    {
        return None;
    }
    // If the final still ends with the delivered ending, the extra content
    // may be an insertion inside the already pasted text. There is no proof of
    // a continuation after the paste boundary.
    if delivered
        .iter()
        .rev()
        .zip(final_content.iter().rev())
        .take_while(|(left, right)| left == right)
        .take(4)
        .count()
        == 4
    {
        return None;
    }
    let max_edits = (len / 5).clamp(2, 12);
    let max_prefix = final_content.len().min(len + max_edits);
    let mut previous: Vec<usize> = (0..=max_prefix).collect();
    let mut current = vec![0; max_prefix + 1];
    for (index, expected) in delivered.iter().enumerate() {
        current[0] = index + 1;
        for prefix in 1..=max_prefix {
            current[prefix] = (previous[prefix] + 1)
                .min(current[prefix - 1] + 1)
                .min(previous[prefix - 1] + usize::from(*expected != final_content[prefix - 1]));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let mut best: Option<(usize, usize)> = None;
    for prefix in len.saturating_sub(max_edits)..=max_prefix {
        if final_content.len() - prefix < 4 || previous[prefix] > max_edits {
            continue;
        }
        if best.is_none_or(|(cost, boundary)| {
            previous[prefix] < cost || (previous[prefix] == cost && prefix > boundary)
        }) {
            best = Some((previous[prefix], prefix));
        }
    }
    let (_, boundary) = best?;
    let tail = final_text[offsets[boundary]..]
        .trim_start_matches(is_embedded_audio_partial_preview_decorative)
        .to_string();
    (!tail
        .trim_matches(|ch: char| is_embedded_audio_partial_preview_decorative(ch))
        .is_empty())
    .then_some(tail)
}

fn pause_early_delivery_session_state(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> (String, String) {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id.as_ref() == Some(&session_id) {
        (
            ledger.delivered_display.clone(),
            ledger.delivered_key.clone(),
        )
    } else {
        (String::new(), String::new())
    }
}

fn pause_early_delivery_source_key(inner: &Arc<Inner>, session_id: SessionId) -> String {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id == Some(session_id) {
        if ledger.source_key.is_empty() && !ledger.delivered_key.is_empty() {
            // A missing source coordinate cannot authorize a full replay of
            // already submitted text. Keep the previous conservative floor.
            ledger.delivered_key.clone()
        } else {
            ledger.source_key.clone()
        }
    } else {
        String::new()
    }
}

fn pause_early_timed_continuation(inner: &Arc<Inner>, session_id: SessionId,
    text: &str, source_key: &str, timing: Option<&crate::asr::volcengine::LiveDeliveryTiming>) -> Option<String> {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id != Some(session_id) || ledger.unconfirmed_prior.is_some()
        || ledger.ever_delivered_session != Some(session_id) || ledger.source_key != source_key {
        return None;
    }
    let (key, boundary, source) = ledger.confirmed_timed_boundary.as_ref()?;
    if key != source_key { return None; }
    let timing = timing?;
    if !source.same_capture(timing.source) { return None; }
    let tail = timing.timing.body_suffix_after_delivery(text, boundary)?;
    let raw_end = text.len().checked_sub(tail.len())?;
    let body = filter_automatic_wake_text(inner, session_id, text, false);
    let body_start = text.trim_end().strip_suffix(&body)?.len();
    let prefs = inner.prefs.get();
    let rules = inner.correction_rules.list().unwrap_or_default();
    let rendered = render_raw_delivery_body(&body, &prefs, &rules);
    let rendered_prefix = render_raw_delivery_body(text.get(body_start..raw_end)?, &prefs, &rules);
    let rendered_tail = rendered.strip_prefix(&rendered_prefix)?;
    Some(pause_early_boundary_tail(rendered_tail, &ledger.delivered_display).to_string())
}

fn pause_early_remember_timed_boundary(inner: &Arc<Inner>, session_id: SessionId,
    source_key: &str, timing: Option<&crate::asr::volcengine::LiveDeliveryTiming>) {
    let boundary = timing.and_then(|timing| timing.timing.delivery_boundary(source_key)
        .map(|boundary| (source_key.into(), boundary, timing.source)));
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id == Some(session_id) && ledger.unconfirmed_prior.is_none()
        && ledger.ever_delivered_session == Some(session_id) && ledger.source_key == source_key {
        if let Some(consumed) = ledger.committed_body_consumption.as_ref()
            .filter(|consumed| consumed.rendered_key == source_key).cloned() {
            ledger.confirmed_source_text = Some(consumed.raw_prefix);
            ledger.confirmed_timed_boundary = consumed.boundary
                .map(|boundary| (source_key.into(), boundary, consumed.source));
            return;
        }
        ledger.confirmed_source_text = timing.and_then(|timing| {
            let raw_key = crate::transcript_boundary::content_key(&timing.timing.text);
            if source_key.is_empty() { return None; }
            let mut matches = raw_key.match_indices(source_key);
            let (start, _) = matches.next()?;
            if matches.next().is_some() { return None; }
            let end = crate::transcript_boundary::consumed_prefix_end(&timing.timing.text,
                &raw_key[..start + source_key.len()])?;
            Some(timing.timing.text[..end].to_string())
        });
        ledger.confirmed_timed_boundary = boundary;
    }
}

fn pause_early_ledger_anchor(inner: &Arc<Inner>, session_id: SessionId) -> Option<crate::asr::volcengine::LiveLedgerAnchor> {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id != Some(session_id) || ledger.unconfirmed_prior.is_some()
        || ledger.ever_delivered_session != Some(session_id) { return None; }
    let (key, boundary, source) = ledger.confirmed_timed_boundary.as_ref()?;
    if key != &ledger.source_key { return None; }
    Some(crate::asr::volcengine::LiveLedgerAnchor {
        source: *source, generation: ledger.delivery_generation,
        source_key: key.clone(), source_text: ledger.confirmed_source_text.clone()?,
        boundary: boundary.clone(),
    })
}

fn pause_early_advance_continuation(inner: &Arc<Inner>, session_id: SessionId,
    continuation: &crate::asr::volcengine::LiveLedgerContinuation,
    expected_display_prefix: &str, new_source_key: &str) -> Option<crate::asr::volcengine::LiveLedgerAnchor> {
    let mut suffix_key = String::new();
    for word in &continuation.suffix.timing.words {
        suffix_key.push_str(&crate::transcript_boundary::content_key(&word.text));
        let Some(next) = continuation.advance(&suffix_key, new_source_key) else { continue; };
        // Apply wake/filler/script/corrections to the complete raw coordinate.
        // A display boundary inside a child token cannot advance the audio seam.
        if pause_early_display_text(inner, session_id, &next.source_text).as_deref() == Some(expected_display_prefix) {
            return Some(next);
        }
    }
    None
}

fn pause_early_remember_continuation(inner: &Arc<Inner>, session_id: SessionId,
    anchor: &crate::asr::volcengine::LiveLedgerAnchor) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    let continues_same_capture = ledger.confirmed_timed_boundary.as_ref()
        .is_some_and(|(_, _, source)| source.same_capture(anchor.source));
    let extends_committed_raw = ledger.confirmed_source_text.as_deref()
        .and_then(|prior| anchor.source_text.strip_prefix(prior))
        .is_some_and(|tail| !crate::transcript_boundary::content_key(tail).is_empty());
    if ledger.session_id == Some(session_id) && ledger.unconfirmed_prior.is_none()
        && ledger.ever_delivered_session == Some(session_id)
        && ledger.delivery_generation == anchor.generation && ledger.source_key == anchor.source_key
        && continues_same_capture && extends_committed_raw {
        ledger.confirmed_source_text = Some(anchor.source_text.clone());
        ledger.confirmed_timed_boundary = Some((anchor.source_key.clone(), anchor.boundary.clone(), anchor.source));
        log::info!("[coord] cropped source bookmark advanced session_id={session_id} generation={} capture_stream={} child_decoder={} source_word_ms={:?}",
            anchor.generation, anchor.source.capture_stream_id, anchor.source.decoder_stream_id, anchor.boundary.range_ms());
    }
}

fn pause_early_admitted_source_key(inner: &Arc<Inner>, session_id: SessionId) -> String {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id != Some(session_id)
        || ledger.ever_delivered_session != Some(session_id)
    {
        return String::new();
    }
    // A reserve is still in flight and may roll back. Only the source of a
    // previously successful submission establishes where the body began.
    let (source, display_key) = ledger.unconfirmed_prior.as_ref()
        .map(|(_, key, source)| (source, key))
        .unwrap_or((&ledger.source_key, &ledger.delivered_key));
    if source.is_empty() { display_key.clone() } else { source.clone() }
}

/// All endpoint readers use the same successful raw consumption transaction.
/// The rendered key remains available to display reconciliation independently.
fn pause_early_committed_body_consumption(
    inner: &Arc<Inner>, session_id: SessionId,
) -> Option<crate::asr::volcengine::LiveBodyConsumption> {
        let ledger = inner.embedded_audio_pause_early_delivery.lock();
        let key = ledger.unconfirmed_prior.as_ref().map(|(_, _, key)| key)
            .unwrap_or(&ledger.source_key);
        (ledger.session_id == Some(session_id) && ledger.ever_delivered_session == Some(session_id))
            .then(|| ledger.committed_body_consumption.as_ref()
                .filter(|value| &value.rendered_key == key).cloned()).flatten()
}

fn pause_early_pending_body_delivery(
    inner: &Arc<Inner>, session_id: SessionId,
    asr: &crate::asr::volcengine::VolcengineStreamingASR,
) -> Option<crate::asr::volcengine::PendingBodyDelivery> {
    match pause_early_committed_body_consumption(inner, session_id) {
        Some(consumed) => asr.pending_body_delivery_from_consumption(&consumed),
        None => asr.pending_body_delivery_after(&pause_early_admitted_source_key(inner, session_id)),
    }
}

fn pause_early_provider_consumed_key(inner: &Arc<Inner>, session_id: SessionId) -> String {
    pause_early_committed_body_consumption(inner, session_id)
        .map(|consumed| crate::transcript_boundary::content_key(&consumed.raw_prefix))
        .unwrap_or_else(|| pause_early_admitted_source_key(inner, session_id))
}

fn pause_early_source_consumption(
    inner: &Arc<Inner>, session_id: SessionId, snapshot: &str, rendered_key: &str,
    source: crate::asr::volcengine::LiveDeliverySource,
    timing: Option<&crate::asr::volcengine::LiveDeliveryTiming>,
) -> Option<crate::asr::volcengine::LiveBodyConsumption> {
    // Observe activation once on the actual full snapshot. Prefix probing is
    // pure; artificial partial strings must not mutate the wake/body latch.
    let body = filter_automatic_wake_text(inner, session_id, snapshot, false);
    if body.is_empty() { return None; }
    let body_offset = snapshot.trim_end().strip_suffix(&body)?.len();
    let prefs = inner.prefs.get();
    let rules = inner.correction_rules.list().unwrap_or_default();
    let raw_prefix = crate::transcript_boundary::source_prefix_for_rendered_key(
        snapshot, rendered_key, |prefix| prefix.get(body_offset..)
            .map(|body| render_raw_delivery_body(body, &prefs, &rules)))?;
    let boundary = timing.filter(|timing| timing.source == source)
        .and_then(|timing| timing.timing.delivery_boundary(&crate::transcript_boundary::content_key(raw_prefix)));
    Some(crate::asr::volcengine::LiveBodyConsumption {
        raw_prefix: raw_prefix.into(), body_start: body_offset, rendered_key: rendered_key.into(), source, boundary,
    })
}

fn render_raw_delivery_body(
    body: &str, prefs: &crate::types::UserPreferences, rules: &[crate::types::CorrectionRule],
) -> String {
    let body = if prefs.remove_filler_words { remove_standalone_dictation_fillers(body) }
        else { body.to_string() };
    let body = apply_chinese_script_preference(&body, prefs.chinese_script_preference);
    apply_correction_rules(&body, rules).trim().to_string()
}

/// Advance in the provider snapshot, not in the concatenated editor text.
/// Both the remainder and the selected clause must be exact slices of that
/// snapshot. A failed or partial submission must not consume unsent words.
fn pause_early_source_key_after_append(
    snapshot: &str,
    remainder: &str,
    appended: &str,
) -> Option<String> {
    let prefix = snapshot.strip_suffix(remainder)?;
    remainder.strip_prefix(appended)?;
    Some(embedded_audio_partial_preview_stability_key(
        &snapshot[..prefix.len() + appended.len()],
    ))
}

fn pause_early_cumulative_submitted_text(
    prior_display: &str,
    submitted_tail: Option<&str>,
) -> Option<String> {
    match (prior_display.is_empty(), submitted_tail) {
        (true, None) => None,
        (true, Some(tail)) => Some(tail.to_string()),
        (false, None) => Some(prior_display.to_string()),
        (false, Some(tail)) => Some(format!("{prior_display}{tail}")),
    }
}

fn pause_early_delivery_reserve(
    inner: &Arc<Inner>,
    session_id: SessionId,
    delivered_display: String,
    delivered_key: String,
    source_key: String,
) {
    pause_early_delivery_reserve_source(inner, session_id, delivered_display, delivered_key, source_key, None);
}

fn pause_early_delivery_reserve_source(
    inner: &Arc<Inner>, session_id: SessionId, delivered_display: String,
    delivered_key: String, source_key: String,
    consumed: Option<crate::asr::volcengine::LiveBodyConsumption>,
) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id != Some(session_id) {
        // A cancelled recording can leave a committed prefix behind. Its
        // rollback state belongs to that recording, never the next one.
        *ledger = PauseEarlyDeliveryLedger::default();
    }
    // 单一漏斗日志：粘贴路径的成败行会在毫秒级跟进来；若出现 reserve 行却
    // 没有后继 success/fail 行 = 粘贴调用中断（2026-09-23 47452640 幻影
    // +40 字取证缺口——账本 72 字但全天只有 32 字粘贴成功行）。
    log::info!(
        "[coord] pause-early reserve session_id={session_id} total_display_chars={} key_chars={} source_key_chars={}",
        delivered_display.chars().count(),
        delivered_key.chars().count(),
        source_key.chars().count()
    );
    ledger.unconfirmed_prior = Some((
        std::mem::take(&mut ledger.delivered_display),
        std::mem::take(&mut ledger.delivered_key),
        std::mem::take(&mut ledger.source_key),
    ));
    ledger.session_id = Some(session_id);
    ledger.delivered_display = delivered_display;
    ledger.delivered_key = delivered_key;
    ledger.source_key = source_key;
    ledger.reserved_body_consumption = consumed;
}

/// The newest reserved paste succeeded on screen: the ledger prefix is now
/// confirmed and the sticky floor latches.
fn pause_early_delivery_confirm(inner: &Arc<Inner>, session_id: SessionId) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id.as_ref() == Some(&session_id) && ledger.unconfirmed_prior.is_some() {
        ledger.committed_body_consumption = ledger.reserved_body_consumption.take();
        ledger.unconfirmed_prior = None;
        ledger.ever_delivered_session = Some(session_id);
        ledger.delivery_generation = ledger.delivery_generation.wrapping_add(1);
    }
}

fn pause_early_delivery_rollback(inner: &Arc<Inner>, session_id: SessionId) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id.as_ref() != Some(&session_id) {
        return;
    }
    // 只回滚这一次未确认的交付，恢复到上一个已确认前缀；已上屏的早期文本
    // 绝不从账本里消失（整段抹掉会让终稿全文重贴 = 双粘贴）。首贴失败时
    // prior 为空串 → 前缀归零，终稿全文交付，行为与旧版一致。
    if let Some((prior_display, prior_key, prior_source_key)) = ledger.unconfirmed_prior.take() {
        ledger.delivered_display = prior_display;
        ledger.delivered_key = prior_key;
        ledger.source_key = prior_source_key;
    }
    ledger.reserved_body_consumption = None;
}

/// 本会话是否确实上屏过停顿落屏文本（跨回滚粘滞）。终稿交付的硬底线。
pub(super) fn pause_early_ever_delivered(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let ledger = inner.embedded_audio_pause_early_delivery.lock();
    ledger.ever_delivered_session.as_ref() == Some(&session_id)
}

pub(super) fn pause_early_paste_delivered(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    inner.embedded_audio_pause_early_delivery.lock().paste_delivered_session == Some(session_id)
}

/// 终稿路径取走本会话的已交付前缀（display, key），取走即清零。粘滞底线
/// （曾上屏）是历史事实，跨 take 保留——终稿侧即使先 take 后查询也不翻转。
pub(super) fn take_pause_early_delivery(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<(String, String)> {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id.as_ref() == Some(&session_id) && !ledger.delivered_key.is_empty() {
        let taken = Some((
            ledger.delivered_display.clone(),
            ledger.delivered_key.clone(),
        ));
        let ever = ledger.ever_delivered_session;
        *ledger = PauseEarlyDeliveryLedger::default();
        ledger.ever_delivered_session = ever;
        return taken;
    }
    None
}

/// pause-early 与组字流式共用的显示变换链(2026-09-22 切片3 设计卡铁律:
/// 两条路径必须逐字同款,stability-key 记账才连续)。门失败返回 None。
/// 胶囊预览继续跟随未落定文本；目标应用的中途粘贴只在稳定账本出现标点
/// 边界时提交整段。终稿仍交付不带标点的余量，避免云端只在终稿补标点时吞尾。
fn automatic_wake_has_unstripped_lead_in(raw_text: &str, phrase: &str, admitted_source_key: &str) -> bool {
    raw_text
        .find(phrase)
        .is_some_and(|offset| {
            let lead_key = embedded_audio_partial_preview_stability_key(&raw_text[..offset]);
            // A literal phrase is body both before and after a successful
            // append crosses it. Before crossing, the lead extends the
            // committed source; after crossing, the complete current body
            // extends that source. Comparing only the lead with the advancing
            // consumption cursor revoked admission as soon as the phrase had
            // been pasted (9919084d, ticket 77), freezing all later clauses.
            // This requires the same complete prefix, not merely a shorter
            // matching lead: prepended ambient text and incompatible revisions
            // remain uncovered. The caller supplies only successful source
            // consumption, never an in-flight reservation or display transform.
            if !admitted_source_key.is_empty()
                && (lead_key.starts_with(admitted_source_key)
                    || embedded_audio_partial_preview_stability_key(raw_text)
                        .starts_with(admitted_source_key))
            {
                return false;
            }
            raw_text[..offset].chars().filter(|ch| ch.is_alphanumeric()).count() > 2
                // The same wake stripping path already knows how to consume
                // bounded hesitation + short lead-ins. The old raw character
                // count vetoed those valid results for the entire session,
                // leaving a live preview but no early text until finalization.
                && strip_automatic_activation_prefix(raw_text, phrase, true)
                    == raw_text.trim()
        })
}

fn pause_early_display_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    raw_text: &str,
) -> Option<String> {
    live_delivery_display_text(inner, session_id, raw_text, false)
}

fn session_llm_circuit_state(inner: &Arc<Inner>, session_id: SessionId) -> (bool, bool) {
    let cached = {
        let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
        if ledger.session_id != Some(session_id) {
            *ledger = PauseEarlyDeliveryLedger { session_id: Some(session_id), ..Default::default() };
        }
        ledger.llm_auth_fingerprint
    };
    let fingerprint = cached.unwrap_or_else(|| {
        let fingerprint = current_llm_auth_fingerprint().ok();
        let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
        if ledger.session_id == Some(session_id) {
            ledger.llm_auth_fingerprint = Some(fingerprint);
        }
        fingerprint
    });
    (fingerprint.is_some_and(current_llm_auth_is_rejected), llm_stall_circuit_open())
}

fn live_raw_delivery_supported(
    translation_active: bool, needs_llm_polish: bool, auth_blocked: bool, stall_blocked: bool,
) -> bool {
    // Use the same actual disposition as final dispatch. A rejected/unavailable
    // polish service cannot disable raw clauses that the final will pass through.
    !translation_active && (!needs_llm_polish || auth_blocked || stall_blocked)
}

fn live_delivery_display_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    raw_text: &str,
    partial: bool,
) -> Option<String> {
    // The phrase detector can open a session even when the provider's early
    // text begins with another speaker and places the real wake phrase later.
    // Normalize the same proven control prefix used by capsule and final
    // before checking for an ambiguous, still-unstripped lead-in.
    let resolved = resolve_automatic_wake_text(inner, session_id, raw_text, partial);
    let text = resolved.text;
    let wake_phrase = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| guard.session_id == session_id)
        .map(|guard| guard.phrase.clone());
    let admitted_source_key = pause_early_committed_body_consumption(inner, session_id)
        .and_then(|consumed| consumed.raw_prefix.get(consumed.body_start..)
            .map(crate::transcript_boundary::content_key))
        .unwrap_or_else(|| pause_early_admitted_source_key(inner, session_id));
    if !resolved.activation_resolved && wake_phrase
        .as_deref()
        .is_some_and(|phrase| automatic_wake_has_unstripped_lead_in(&text, phrase, &admitted_source_key))
    {
        pause_early_note_gate_blocked(inner, session_id, "unstripped_pre_wake_lead_in");
        return None;
    }
    let prefs = inner.prefs.get();
    let translation_active = inner.translation_modifier_seen.load(Ordering::SeqCst)
        && !prefs.translation_target_language.trim().is_empty()
        && !pause_early_ever_delivered(inner, session_id);
    let force_raw_output = std::env::var("LISTENER_TYPE_FORCE_RAW_OUTPUT")
        .map(|value| value == "1")
        .unwrap_or(false) || pause_early_ever_delivered(inner, session_id);
    let pack = if force_raw_output {
        crate::types::builtin_style_pack_for_mode(PolishMode::Raw)
    } else {
        match inner.style_packs.get_or_default_active(&prefs.active_style_pack_id) {
            Ok(pack) => pack,
            Err(_) => {
                pause_early_note_gate_blocked(inner, session_id, "style_pack_unavailable");
                return None;
            }
        }
    };
    let raw_uses_llm = !force_raw_output
        && pack.base_mode == PolishMode::Raw
        && super::raw_style_pack_uses_llm(&pack);
    let needs_llm_polish = pack.base_mode != PolishMode::Raw || raw_uses_llm;
    let (auth_blocked, stall_blocked) = if needs_llm_polish {
        session_llm_circuit_state(inner, session_id)
    } else { (false, false) };
    if !live_raw_delivery_supported(translation_active, needs_llm_polish, auth_blocked, stall_blocked) {
        pause_early_note_gate_blocked(inner, session_id, "polish_or_translation_active");
        return None;
    }
    let correction_rules = inner.correction_rules.list().unwrap_or_default();
    let text = render_raw_delivery_body(&text, &prefs, &correction_rules);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Watchdog 每拍评估（无 STOP 待决时）。条件全满足才动作：
/// 干净会话（ASR 账本卫兵：无冻结）+ 同源原生确认或旧协议稳定前缀 +
/// Raw 一次性路径（翻译/LLM 会整体改写，前缀会被孤立）+ 有新增内容 +
/// 当前光标有输入目标。组字流式活着时改走 stream_commit 落定(不碰焦点,
/// 组字锚在目标进程);任何失败静默回退到现行为(终稿一次性交付)。
async fn pause_early_delivery_tick(
    inner: &Arc<Inner>,
    session_id: SessionId,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    if std::env::var("LISTENER_DISABLE_PAUSE_EARLY_DELIVERY").as_deref() == Ok("1") {
        return;
    }
    {
        let state = inner.state.lock();
        if state.session_id != session_id
            || state.cancelled
            || state.phase != SessionPhase::Listening
        {
            return;
        }
    }
    // 2026-09-23 tkg 后诊断:第一拍全量门状态。这行不出现=看门狗路径没跑到
    // (endpoint_policy.body_started 假/循环没进);出现但 frozen=true=归属冻结;
    // ledger_chars 一直 0=账本没进字;stable_ms 一直小于耐心窗=partial 抖动。
    {
        let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
        if ledger.tick_diag_logged.as_ref() != Some(&session_id) {
            ledger.tick_diag_logged = Some(session_id);
            let (frozen, stable_ms, ledger_chars) = asr.pause_early_delivery_gate_diag();
            log::info!(
                "[coord] pause-early tick first reached session_id={session_id} frozen={frozen} stable_ms={stable_ms:?} ledger_chars={ledger_chars}"
            );
        }
    }
    let source_key = pause_early_delivery_source_key(inner, session_id);
    let provider_consumed_key = pause_early_provider_consumed_key(inner, session_id);
    let continuation = pause_early_ledger_anchor(inner, session_id).and_then(|anchor|
        asr.pause_early_ledger_continuation_after(PAUSE_EARLY_DELIVERY_MIN_STABLE, &anchor));
    let view = continuation.as_ref().map(|value| (value.source_text(), None,
            crate::asr::volcengine::LiveDeliveryStability::ProviderConfirmed, value.suffix.source))
        .or_else(|| asr.pause_early_delivery_evidence_after(PAUSE_EARLY_DELIVERY_MIN_STABLE, &provider_consumed_key)
            .map(|snapshot| (snapshot.text, snapshot.timing, snapshot.stability, snapshot.source)));
    let Some((snapshot_text, snapshot_timing, snapshot_stability, snapshot_source)) = view
    else {
        if let Some(reason) = asr.pause_early_delivery_persistent_block_reason() {
            pause_early_note_gate_blocked(inner, session_id, reason);
        }
        // 诊断(一次性):三条件看似全满足却拿不到快照 → 快照侧有隐藏门。
        let (frozen, stable_ms, ledger_chars) = asr.pause_early_delivery_gate_diag();
        if !frozen
            && ledger_chars >= 10
            && stable_ms.is_some_and(|ms| ms >= PAUSE_EARLY_DELIVERY_MIN_STABLE.as_millis())
        {
            let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
            if ledger.contradictory_diag_logged.as_ref() != Some(&session_id) {
                ledger.contradictory_diag_logged = Some(session_id);
                log::warn!(
                    "[coord] pause-early contradictory gate session_id={session_id} frozen={frozen} stable_ms={stable_ms:?} ledger_chars={ledger_chars} — snapshot refused despite qualifying"
                );
            }
        }
        return;
    };
    let snapshot_chars = snapshot_text.chars().count();
    let full_ledger_chars = asr.pause_early_delivery_gate_diag().2;
    {
        let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
        if ledger.qualified_diag_logged.as_ref() != Some(&session_id) {
            ledger.qualified_diag_logged = Some(session_id);
            log::info!(
                "[coord] pause-early snapshot qualified session_id={session_id} chars={}",
                snapshot_text.chars().count()
            );
        }
    }
    // Preserve the exact ASR source coordinate before wake/filler/script/
    // correction display transforms. The current owner candidate lives in
    // this same coordinate, including a bounded hesitation before the wake.
    // Asking with the displayed body key would fail to locate such a prefix
    // and could mistake an older stable snapshot for the complete candidate.
    let snapshot_owner_source_key =
        crate::transcript_boundary::content_key(&snapshot_text);
    // 显示侧变换必须镜像终稿 Raw 路径，前缀才能与终稿逐字可比（标点差由
    // stability key 容忍）。组字流式(切片3)与这里共用同一条链。
    let Some(text) = pause_early_display_text(inner, session_id, &snapshot_text) else {
        return;
    };
    let prefs = inner.prefs.get();
    let (delivered_display, delivered_key) =
        pause_early_delivery_session_state(inner, session_id);
    if continuation.is_some() && !text.starts_with(&delivered_display) {
        pause_early_note_gate_blocked(inner, session_id, "cropped_display_prefix_changed");
        return;
    }
    let exact_delta = pause_early_final_remainder(&text, &delivered_display, &source_key);
    let anchored = exact_delta.is_none();
    let Some(delta) = exact_delta
        .or_else(|| pause_early_revised_terminal_remainder(&text, &delivered_display, &source_key)
            .filter(|tail| pause_early_terminal_punctuation_only(tail)))
        .or_else(|| pause_early_anchored_continuation(&text, &delivered_display, &source_key))
        // Provider revisions can change the last words of an already pasted
        // clause, so an exact eight-character seam disappears even though the
        // new owner-ledger prefix has kept growing. Reuse the bounded edit
        // alignment used at finalization, on the stable owner snapshot only.
        // Its internal-growth and large-rewrite guards still prevent a replay.
        .or_else(|| pause_early_aligned_growth_tail(&text, &source_key))
        .or_else(|| pause_early_timed_continuation(inner, session_id, &snapshot_text, &source_key, snapshot_timing.as_ref()))
    else {
        let current_key = embedded_audio_partial_preview_stability_key(&text);
        if source_key.starts_with(&current_key) {
            // A stable snapshot can stop inside an already submitted prefix.
            // That means no new stable body, not a failed source alignment.
            return;
        }
        let shared = source_key
            .chars()
            .zip(current_key.chars())
            .take_while(|(left, right)| left == right)
            .count();
        let first_block = inner.embedded_audio_pause_early_delivery.lock().gate_blocked_logged
            != Some((session_id, "delivered_prefix_revised_unaligned"));
        if first_block {
            log::info!(
                "[coord] pause-early alignment unavailable session_id={session_id} source_key_chars={} snapshot_key_chars={} shared_prefix_chars={shared}",
                source_key.chars().count(),
                current_key.chars().count(),
            );
        }
        pause_early_note_gate_blocked(inner, session_id, "delivered_prefix_revised_unaligned");
        return;
    };
    if delta.is_empty() {
        return;
    }
    let ledger_quiet = asr.pause_early_delivery_gate_diag().1
        .is_some_and(|ms| ms >= PAUSE_EARLY_DELIVERY_MIN_STABLE.as_millis());
    let preview_quiet = inner.embedded_audio_preview.lock()
        .last_visible_growth_at(session_id)
        .is_some_and(|at| at.elapsed() >= PAUSE_EARLY_UNPUNCTUATED_QUIET);
    // `full_ledger_chars` is an ASR-internal source coordinate. A late
    // original-source Owner proof can extend the approved snapshot after the
    // last provider frame, while that diagnostic still describes the shorter
    // filtered ledger (and automatic wake stripping changes its length again).
    // Compare the normalized body with the actual visible body instead. This
    // remains conservative when the visual path contains any unowned tail.
    let visible_delivery_text = current_embedded_audio_visual_preview(inner)
        .and_then(|visible| pause_early_display_text(inner, session_id, &visible));
    let visible_body_covered = pause_early_snapshot_covers_visible_body(
        &text,
        visible_delivery_text.as_deref(),
    );
    let remainder = delta;
    let pending_after_snapshot = (!snapshot_owner_source_key.is_empty())
        .then(|| asr.pending_body_delivery_after(&snapshot_owner_source_key))
        .flatten()
        .map(|pending| pending.key);
    let owner_candidate_caught_up = continuation.as_ref().map_or_else(
        || pause_early_snapshot_has_no_newer_owner_body(pending_after_snapshot.as_deref()),
        |value| value.owner_caught_up,
    );
    if snapshot_stability == crate::asr::volcengine::LiveDeliveryStability::RollingStable
        && visible_body_covered
        && ledger_quiet
        && preview_quiet
        && !owner_candidate_caught_up
        && pause_early_complete_clause(&remainder).is_none()
    {
        pause_early_note_gate_blocked(inner, session_id, "newer_owner_candidate_settling");
    }
    let quiet_boundary = visible_body_covered
        && owner_candidate_caught_up
        && ledger_quiet
        && preview_quiet;
    let Some(delta) = pause_early_ready_segment(&delivered_display, &delivered_key,
        &remainder, quiet_boundary, snapshot_stability) else {
        return;
    };
    let Some(new_source_key) = pause_early_source_key_after_append(&text, &remainder, delta) else {
        pause_early_note_gate_blocked(inner, session_id, "source_boundary_unavailable");
        return;
    };
    let next_anchor = if let Some(value) = &continuation {
        let Some(prefix_len) = text.len().checked_sub(remainder.len()) else { return; };
        let Some(next) = pause_early_advance_continuation(inner, session_id, value,
            &text[..prefix_len + delta.len()], &new_source_key) else {
            pause_early_note_gate_blocked(inner, session_id, "cropped_child_boundary_unavailable");
            return;
        };
        if pause_early_ledger_anchor(inner, session_id).as_ref() != Some(&value.anchor)
            || !asr.ledger_continuation_is_current(value) { return; }
        Some(next)
    } else { None };
    let consumed = if let Some(anchor) = &next_anchor {
        let Some(prior) = pause_early_committed_body_consumption(inner, session_id)
            .filter(|prior| anchor.source_text.starts_with(&prior.raw_prefix)) else {
                pause_early_note_gate_blocked(inner, session_id, "cropped_raw_body_boundary_unavailable");
                return;
            };
        Some(crate::asr::volcengine::LiveBodyConsumption {
            raw_prefix: anchor.source_text.clone(), body_start: prior.body_start, rendered_key: new_source_key.clone(),
            source: anchor.source, boundary: Some(anchor.boundary.clone()),
        })
    } else {
        pause_early_source_consumption(inner, session_id, &snapshot_text, &new_source_key,
            snapshot_source, snapshot_timing.as_ref())
    };
    let Some(consumed) = consumed else {
        pause_early_note_gate_blocked(inner, session_id, "raw_source_boundary_unavailable");
        return;
    };
    let new_display = format!("{delivered_display}{delta}");
    // The key must describe what was actually pasted. The provider's revised
    // full prefix may differ from that text, even when its suffix aligns.
    let new_key = embedded_audio_partial_preview_stability_key(&new_display);
    if anchored {
        log::info!(
            "[coord] pause-early anchored continuation session_id={session_id} prefix_chars={} total_delivered_chars={}",
            delta.chars().count(),
            new_display.chars().count()
        );
    }
    if native_composition_written(inner, session_id) {
        if native_composition_commit_stable(inner, session_id, delta, &delivered_display,
            &new_key, &new_source_key, consumed, endpoint_clock).await {
            if let Some(next) = &next_anchor {
                pause_early_remember_continuation(inner, session_id, next);
            } else {
                pause_early_remember_timed_boundary(inner, session_id, &new_source_key, snapshot_timing.as_ref());
            }
        }
        return;
    }
    // 组字流式活着 → 落定走 stream_commit(组字锚在目标进程,不需要焦点
    // 恢复;账本先记后 commit,失败回滚——次序与粘贴路径同款)。失败时驱动
    // 已降级清组字,下一拍以空账本走粘贴分支补上。
    if streaming_composition_active(inner, session_id) {
        if pause_early_terminal_punctuation_only(delta)
            && !matches!(inner.inserter.terminal_append_decision(&delivered_display, delta),
                crate::insertion::TerminalAppendDecision::Append | crate::insertion::TerminalAppendDecision::AlreadyPresent)
        {
            pause_early_note_gate_blocked(inner, session_id, "terminal_suffix_current_editor_unverified");
            return;
        }
        if streaming_composition_commit_stable(inner, session_id, delta, &delivered_display, &new_key, &new_source_key, consumed, endpoint_clock).await {
            if let Some(next) = &next_anchor {
                pause_early_remember_continuation(inner, session_id, next);
            } else {
                pause_early_remember_timed_boundary(inner, session_id, &new_source_key, snapshot_timing.as_ref());
            }
        }
        return;
    }
    // 组字降级且清场也失败:文档里可能有残留组字,粘贴会重复,宁停手。
    if streaming_composition_contaminated(inner, session_id) {
        log::warn!(
            "[coord] pause-early-delivery skipped: streaming composition contaminated session_id={session_id}"
        );
        return;
    }
    let delivery_target = current_delivery_target();
    if !delivery_target_is_current(delivery_target) {
        log::info!(
            "[coord] pause-early-delivery skipped: current cursor target unavailable session_id={session_id}"
        );
        return;
    }
    // 先记账再粘贴：并发触发的终稿交付会按在途前缀计算余量；粘贴失败回滚。
    pause_early_delivery_reserve_source(inner, session_id, new_display, new_key, new_source_key.clone(), Some(consumed));
    // Close the endpoint race before the synchronous platform insertion. The
    // sequence is settled only by this dispatch, so a late failure cannot
    // release a newer delivery's hold.
    let body_submission = endpoint_clock
        .lock()
        .begin_text_submission(Instant::now(), delta);
    // 2026-09-23 tki:中途粘贴不恢复剪贴板。750ms 恢复窗口追不上忙碌目标
    // (VS Code/Chromium 渲染器)的粘贴派发——08:39 实锤:第 1 段 22 字正确,
    // 第 2/3 段粘出的是用户 08:22 复制的旧报告×2(账本记 14+14 字已交付,
    // 落屏的却是恢复后的旧剪贴板)。会话内剪贴板暂存听写增量无碍;终稿
    // 交付仍按用户偏好恢复,那才是剪贴板所有权归还的正当时机。
    let restore_clipboard = false;
    let terminal_status = if pause_early_terminal_punctuation_only(delta) {
        match inner.inserter.insert_session_terminal_suffix(session_id, &delivered_display, delta, prefs.paste_shortcut) {
            Ok(status) => Some(status),
            Err(crate::insertion::TerminalAppendDecision::AlreadyPresent) => Some(InsertStatus::Inserted),
            Err(_) => {
                pause_early_delivery_rollback(inner, session_id);
                endpoint_clock
                    .lock()
                    .settle_text_submission(body_submission, false, None);
                pause_early_note_gate_blocked(inner, session_id, "terminal_suffix_current_editor_unverified");
                return;
            }
        }
    } else {
        None
    };
    #[cfg(target_os = "windows")]
    let (status, route, receipt) = {
        if let Some(status) = terminal_status {
            (status, DeliveryRoute::Paste, None)
        } else {
            let result = insert_via_non_tsf_fallback_scoped(inner, delta, restore_clipboard, prefs.paste_shortcut,
                Some(crate::insertion::SessionPasteScope { session_id, previous_body: delivered_display.clone(), replace: false }));
            (result.status, result.route, result.receipt)
        }
    };
    #[cfg(not(target_os = "windows"))]
    let (status, route, receipt) = {
        if let Some(status) = terminal_status {
            (status, DeliveryRoute::Paste, None)
        } else {
            let submission = inner.inserter.insert_session_paste(delta, prefs.paste_shortcut,
                crate::insertion::SessionPasteScope { session_id, previous_body: delivered_display.clone(), replace: false });
            (submission.status, DeliveryRoute::Paste, submission.receipt)
        }
    };
    let inserted = matches!(
        status,
        InsertStatus::PasteSent | InsertStatus::Inserted | InsertStatus::SubmittedUnconfirmed
    );
    if !inserted {
        pause_early_delivery_rollback(inner, session_id);
        endpoint_clock
            .lock()
            .settle_text_submission(body_submission, false, None);
        log::warn!(
            "[coord] pause-early-delivery paste failed status={:?} chars={} — final will deliver the remainder",
            status,
            delta.chars().count()
        );
        return;
    }
    pause_early_delivery_confirm(inner, session_id);
    if let Some(next) = &next_anchor {
                pause_early_remember_continuation(inner, session_id, next);
            } else {
                pause_early_remember_timed_boundary(inner, session_id, &new_source_key, snapshot_timing.as_ref());
            }
    endpoint_clock
        .lock()
        .settle_text_submission(body_submission, true, receipt);
    if route == DeliveryRoute::Paste {
        inner.embedded_audio_pause_early_delivery.lock().paste_delivered_session = Some(session_id);
    }
    log::info!(
        "[coord] pause-early-delivery prefix_chars={} total_delivered_chars={} snapshot_chars={snapshot_chars} ledger_chars={full_ledger_chars} min_stable_ms={} route={:?} status={:?}",
        delta.chars().count(),
        delivered_display.chars().count() + delta.chars().count(),
        PAUSE_EARLY_DELIVERY_MIN_STABLE.as_millis(),
        route,
        status
    );
}

#[cfg(test)]
mod reversible_visual_activation_tests {
    use super::exact_leading_activation_body;

    #[test]
    fn exact_activation_prefix_reveals_only_the_reversible_body() {
        assert_eq!(
            exact_leading_activation_body("开始录音，今天的安排已经确认", "开始录音"),
            Some("今天的安排已经确认".to_string())
        );
    }

    #[test]
    fn ambient_or_incomplete_prefix_cannot_become_reversible_body() {
        assert_eq!(
            exact_leading_activation_body("你欺负她。开始录音，这是我的正文", "开始录音"),
            None
        );
        assert_eq!(
            exact_leading_activation_body("开始安排今天的资料", "开始录音"),
            None
        );
        assert_eq!(exact_leading_activation_body("开始录音。", "开始录音"), None);
    }

    #[test]
    fn repeated_wake_phrase_in_body_is_preserved() {
        assert_eq!(
            exact_leading_activation_body(
                "开始录音，正文里再说开始录音也要保留",
                "开始录音"
            ),
            Some("正文里再说开始录音也要保留".to_string())
        );
    }
}
