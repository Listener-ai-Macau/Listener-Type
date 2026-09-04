// Embedded audio partial preview / wake-phrase filtering helpers.
// Included into `coordinator::dictation` via `include!`.

#[derive(Debug)]
struct ProductFinalCandidates {
    provider_primary: RawTranscript,
    separated_owner: Option<RawTranscript>,
    retained_audio_replay: Option<RawTranscript>,
    debug_override: Option<RawTranscript>,
    partial_preview: Option<RawTranscript>,
    local_shadow: Option<String>,
    local_shadow_owner_end_aligned: bool,
    target_filter_required: bool,
}

#[derive(Debug)]
struct ProductFinalDecision {
    transcript: RawTranscript,
    authority: crate::speech_decision_kernel::ProductFinalAuthority,
    local_shadow_recovered: bool,
}

fn nonempty_transcript(candidate: &Option<RawTranscript>) -> bool {
    candidate
        .as_ref()
        .is_some_and(|candidate| !candidate.text.trim().is_empty())
}

/// The only product-boundary text selector. Every upstream result is supplied
/// as an immutable candidate, one authority is selected once, and optional
/// omission/hotword/filler normalization happens before the returned content
/// is sealed by the caller. No downstream recovery path may grow this result.
fn arbitrate_product_final_transcript(
    candidates: ProductFinalCandidates,
    hotwords: &[DictionaryHotword],
    remove_filler_words: bool,
) -> ProductFinalDecision {
    use crate::speech_decision_kernel::{
        arbitrate_product_final, ProductFinalAuthority, ProductFinalEvidence,
    };

    let authority = arbitrate_product_final(ProductFinalEvidence {
        target_filter_required: candidates.target_filter_required,
        separated_owner_available: nonempty_transcript(&candidates.separated_owner),
        provider_primary_available: !candidates.provider_primary.text.trim().is_empty(),
        retained_audio_replay_available: nonempty_transcript(&candidates.retained_audio_replay),
        debug_override_available: nonempty_transcript(&candidates.debug_override),
        partial_preview_available: nonempty_transcript(&candidates.partial_preview),
    });
    let preview_for_hotwords = candidates
        .partial_preview
        .as_ref()
        .map(|preview| preview.text.clone());

    let mut transcript = match authority {
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
    }
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
    let preview = filter_dictation_preview_text(inner, session_id, &text);
    if preview.is_empty() {
        return false;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!("chars={}", preview.chars().count()),
        |_| {
            let mut slot = inner.embedded_audio_partial_preview.lock();
            let Some(provider_preview) = provider_preview_change(slot.as_deref(), &preview) else {
                return false;
            };
            *slot = Some(provider_preview.clone());
            *inner.embedded_audio_visual_preview.lock() = Some(provider_preview.clone());
            let emitted =
                emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview);
            if emitted {
                crate::observability::record_embedded_audio_preview_published(
                    session_id,
                    crate::observability::PreviewSource::ProviderStream,
                    embedded_audio_stop_feedback_latched(inner),
                );
            }
            emitted
        },
    )
}

/// Publish a diarization-pending provider tail to the capsule without
/// promoting it into the authoritative preview ledger. The ASR layer permits
/// this only while the verified wake identity remains debounced as the owner
/// and no NonTarget evidence is present. It may therefore improve visual
/// cadence, but cannot extend the endpoint or enter final insertion/recovery.
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
            // Visual-only text is derived from the same provider stream as the
            // authoritative preview, but it may have the automatic wake prefix
            // stripped (and therefore be shorter).  Sending that shorter value
            // through the shared capsule channel makes the frontend animate a
            // backtrack on every update (for example 8 -> 3 -> 8 chars).  Keep
            // the visible preview monotonic until the authoritative stream has
            // caught up; visual-only text remains a latency hint, never a
            // reason to erase text the user already saw.
            let authoritative = inner.embedded_audio_partial_preview.lock();
            if authoritative.as_deref().is_some_and(|current| {
                spoken_preview_len(&preview) < spoken_preview_len(current)
            }) {
                return false;
            }
            drop(authoritative);
            let mut slot = inner.embedded_audio_visual_preview.lock();
            let Some(provider_preview) = provider_preview_change(slot.as_deref(), &preview) else {
                return false;
            };
            *slot = Some(provider_preview.clone());
            emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview)
        },
    )
}

fn spoken_preview_len(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

fn update_embedded_audio_partial_preview_from_final_supplement(
    inner: &Arc<Inner>,
    session_id: SessionId,
    update: crate::asr::volcengine::FinalIntermediateTranscript,
) -> bool {
    let authoritative_two_pass = update.authoritative_two_pass;
    let preview = filter_dictation_preview_text(inner, session_id, &update.text);
    if preview.is_empty() {
        return false;
    }
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        format!(
            "final_supplement chars={} authoritative_two_pass={}",
            preview.chars().count(),
            authoritative_two_pass
        ),
        |_| {
            let mut slot = inner.embedded_audio_partial_preview.lock();
            let Some(provider_preview) = provider_preview_change(slot.as_deref(), &preview) else {
                return false;
            };
            *slot = Some(provider_preview.clone());
            *inner.embedded_audio_visual_preview.lock() = Some(provider_preview.clone());
            let emitted =
                emit_embedded_audio_partial_preview_if_active(inner, session_id, provider_preview);
            if emitted {
                crate::observability::record_embedded_audio_preview_published(
                    session_id,
                    crate::observability::PreviewSource::FinalSupplement,
                    embedded_audio_stop_feedback_latched(inner),
                );
            }
            emitted
        },
    )
}

// `stream` and `two_pass` originate from the same authoritative ASR session.
// A newer non-identical candidate must replace the capsule text, including an
// early rewrite; only an exact duplicate is safe to suppress.
fn provider_preview_change(current: Option<&str>, candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() || current.is_some_and(|value| value.trim() == candidate) {
        return None;
    }
    Some(candidate.to_string())
}

fn stabilize_embedded_audio_partial_preview(
    current: Option<&str>,
    candidate: &str,
) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    let Some(current) = current.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(candidate.to_string());
    };
    let current_key = embedded_audio_partial_preview_stability_key(current);
    let candidate_key = embedded_audio_partial_preview_stability_key(candidate);
    if current_key == candidate_key || candidate_key.is_empty() {
        return None;
    }
    if current_key.is_empty() {
        return Some(candidate.to_string());
    }
    if candidate_key.starts_with(&current_key) {
        let current_key_chars = current_key.chars().count();
        if embedded_audio_partial_preview_repeats_recent_short_tail(
            &current_key,
            &candidate_key,
            current_key_chars,
        ) {
            return None;
        }
        return stitch_embedded_audio_partial_preview(current, candidate, current_key_chars);
    }
    if current_key.starts_with(&candidate_key) {
        return None;
    }

    let current_chars = current_key.chars().count();
    let candidate_chars = candidate_key.chars().count();
    if current_chars <= 4 && candidate_chars >= current_chars.saturating_add(3) {
        return Some(candidate.to_string());
    }

    None
}

fn stabilize_embedded_audio_final_supplemental_preview(
    current: Option<&str>,
    candidate: &str,
) -> Option<String> {
    stabilize_embedded_audio_final_supplemental_preview_with_provider_authority(
        current, candidate, false,
    )
}

fn stabilize_embedded_audio_final_supplemental_preview_with_provider_authority(
    current: Option<&str>,
    candidate: &str,
    authoritative_two_pass: bool,
) -> Option<String> {
    const FINAL_SUPPLEMENT_SEED_CHARS: usize = 2;
    const SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS: usize = 12;
    const SHORT_PREFIX_REPAIR_MAX_REWRITE_CHARS: usize = 2;
    const SHORT_PREFIX_REPAIR_MIN_EXTENSION_CHARS: usize = 3;
    const SHORT_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS: usize = 2;
    const LONG_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS: usize = 12;
    const LONG_PREFIX_REPAIR_MIN_EXTENSION_CHARS: usize = 6;
    const LONG_SHIFTED_REPAIR_MIN_TOTAL_GROWTH_CHARS: usize = 2;
    const AUTHORITATIVE_EARLY_REWRITE_MIN_SHARED_PREFIX_CHARS: usize = 2;
    const AUTHORITATIVE_EARLY_REWRITE_MIN_EXTENSION_CHARS: usize = 4;
    const AUTHORITATIVE_EARLY_REWRITE_MAX_EDIT_CHARS: usize = 4;
    const AUTHORITATIVE_LONG_REVISION_MIN_SHARED_PREFIX_CHARS: usize = 12;
    const AUTHORITATIVE_LONG_REVISION_MAX_LENGTH_DELTA_CHARS: usize = 4;
    const AUTHORITATIVE_LONG_REVISION_MAX_EDIT_CHARS: usize = 4;
    const AUTHORITATIVE_REWRITE_MIN_SHARED_PREFIX_CHARS: usize = 6;
    const AUTHORITATIVE_REWRITE_MIN_EXTENSION_CHARS: usize = 8;

    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    let candidate_key = embedded_audio_partial_preview_stability_key(candidate);
    if candidate_key.is_empty() {
        return None;
    }
    let candidate_key_chars = candidate_key.chars().count();
    let Some(current) = current.map(str::trim).filter(|value| !value.is_empty()) else {
        return (candidate_key_chars >= FINAL_SUPPLEMENT_SEED_CHARS).then(|| candidate.to_string());
    };
    let current_key = embedded_audio_partial_preview_stability_key(current);
    if current_key.is_empty() {
        return None;
    }
    if authoritative_two_pass && current_key != candidate_key {
        log::info!(
            "[coord] applied provider-authoritative two-pass preview correction current_chars={} candidate_chars={}",
            current_key.chars().count(),
            candidate_key_chars
        );
        return Some(candidate.to_string());
    }
    if current_key == candidate_key {
        return embedded_audio_final_supplement_adds_decorative_progress(current, candidate)
            .then(|| candidate.to_string());
    }
    if authoritative_two_pass
        && embedded_audio_final_supplement_is_brief_bounded_revision(&current_key, &candidate_key)
    {
        return Some(candidate.to_string());
    }
    let current_key_chars = current_key.chars().count();
    if candidate_key.starts_with(&current_key) {
        if embedded_audio_partial_preview_repeats_recent_short_tail(
            &current_key,
            &candidate_key,
            current_key_chars,
        ) {
            return None;
        }
        return stitch_embedded_audio_partial_preview(current, candidate, current_key_chars);
    }
    if current_key.starts_with(&candidate_key) {
        return None;
    }
    let shared_prefix =
        embedded_audio_partial_preview_common_prefix_chars(&current_key, &candidate_key);
    let bounded_long_revision = current_key_chars > SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS
        && embedded_audio_final_supplement_has_bounded_long_rewrite_alignment(
            &current_key,
            &candidate_key,
            shared_prefix,
            AUTHORITATIVE_LONG_REVISION_MIN_SHARED_PREFIX_CHARS,
            AUTHORITATIVE_LONG_REVISION_MAX_LENGTH_DELTA_CHARS,
            AUTHORITATIVE_LONG_REVISION_MAX_EDIT_CHARS,
        );
    if candidate_key_chars
        < current_key_chars.saturating_add(SHORT_PREFIX_REPAIR_MIN_EXTENSION_CHARS)
        && !bounded_long_revision
    {
        return None;
    }
    if current_key_chars > SHORT_PREFIX_REPAIR_MAX_CURRENT_CHARS {
        let stable_long_prefix = shared_prefix >= LONG_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS
            && candidate_key_chars
                >= current_key_chars.saturating_add(LONG_PREFIX_REPAIR_MIN_EXTENSION_CHARS);
        let bounded_prefix_insertion =
            embedded_audio_final_supplement_has_bounded_prefix_insertion_alignment(
                &current_key,
                &candidate_key,
                shared_prefix,
                LONG_SHIFTED_REPAIR_MIN_TOTAL_GROWTH_CHARS,
            );
        let bounded_early_rewrite =
            embedded_audio_final_supplement_has_bounded_early_rewrite_alignment(
                &current_key,
                &candidate_key,
                shared_prefix,
                AUTHORITATIVE_EARLY_REWRITE_MIN_SHARED_PREFIX_CHARS,
                AUTHORITATIVE_EARLY_REWRITE_MIN_EXTENSION_CHARS,
                AUTHORITATIVE_EARLY_REWRITE_MAX_EDIT_CHARS,
            );
        let authoritative_midstream_rewrite = shared_prefix
            >= AUTHORITATIVE_REWRITE_MIN_SHARED_PREFIX_CHARS
            && candidate_key_chars
                >= current_key_chars.saturating_add(AUTHORITATIVE_REWRITE_MIN_EXTENSION_CHARS);
        if bounded_early_rewrite {
            log::info!(
                "[coord] accepted bounded authoritative early preview rewrite current_chars={} candidate_chars={} shared_prefix_chars={}",
                current_key_chars,
                candidate_key_chars,
                shared_prefix
            );
            return Some(candidate.to_string());
        }
        if bounded_long_revision {
            log::info!(
                "[coord] accepted bounded authoritative long preview revision current_chars={} candidate_chars={} shared_prefix_chars={}",
                current_key_chars,
                candidate_key_chars,
                shared_prefix
            );
            return Some(candidate.to_string());
        }
        return (stable_long_prefix || bounded_prefix_insertion || authoritative_midstream_rewrite)
            .then(|| candidate.to_string());
    }
    if current_key_chars <= 4 || shared_prefix >= SHORT_PREFIX_REPAIR_MIN_SHARED_PREFIX_CHARS {
        return Some(candidate.to_string());
    }
    if current_key_chars.saturating_sub(shared_prefix) > SHORT_PREFIX_REPAIR_MAX_REWRITE_CHARS {
        return None;
    }
    Some(candidate.to_string())
}

fn embedded_audio_final_supplement_adds_decorative_progress(
    current: &str,
    candidate: &str,
) -> bool {
    candidate
        .strip_prefix(current)
        .filter(|suffix| !suffix.is_empty())
        .is_some_and(|suffix| {
            suffix
                .chars()
                .all(is_embedded_audio_partial_preview_decorative)
        })
}

// Final-session two-pass corrections can replace a short early branch without
// adding characters. Both ends must still agree before the visible preview is
// allowed to change, so unrelated short phrases cannot overwrite it.
fn embedded_audio_final_supplement_is_brief_bounded_revision(
    current_key: &str,
    candidate_key: &str,
) -> bool {
    const MIN_CHARS: usize = 5;
    const MAX_CHARS: usize = 12;
    const MAX_LENGTH_DELTA_CHARS: usize = 2;
    const MIN_SHARED_PREFIX_CHARS: usize = 2;
    const MIN_SHARED_SUFFIX_CHARS: usize = 2;

    let current_chars = current_key.chars().count();
    let candidate_chars = candidate_key.chars().count();
    current_chars >= MIN_CHARS
        && candidate_chars >= MIN_CHARS
        && current_chars <= MAX_CHARS
        && candidate_chars <= MAX_CHARS
        && current_chars.abs_diff(candidate_chars) <= MAX_LENGTH_DELTA_CHARS
        && embedded_audio_partial_preview_common_prefix_chars(current_key, candidate_key)
            >= MIN_SHARED_PREFIX_CHARS
        && embedded_audio_partial_preview_common_suffix_chars(current_key, candidate_key)
            >= MIN_SHARED_SUFFIX_CHARS
}

fn embedded_audio_final_supplement_has_bounded_prefix_insertion_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_total_growth_chars: usize,
) -> bool {
    const MIN_SHARED_PREFIX_CHARS: usize = 2;
    const MAX_INSERTED_PREFIX_CHARS: usize = 2;

    if shared_prefix_chars < MIN_SHARED_PREFIX_CHARS {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    if candidate.len() < current.len().saturating_add(min_total_growth_chars) {
        return false;
    }

    let mut current_index = shared_prefix_chars;
    let mut candidate_index = shared_prefix_chars;
    let mut inserted_chars = 0usize;
    while current_index < current.len() && candidate_index < candidate.len() {
        if current[current_index] == candidate[candidate_index] {
            current_index += 1;
            candidate_index += 1;
        } else if inserted_chars < MAX_INSERTED_PREFIX_CHARS {
            inserted_chars += 1;
            candidate_index += 1;
        } else {
            return false;
        }
    }

    current_index == current.len() && inserted_chars > 0
}

fn embedded_audio_final_supplement_has_bounded_early_rewrite_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_shared_prefix_chars: usize,
    min_extension_chars: usize,
    max_edit_chars: usize,
) -> bool {
    if shared_prefix_chars < min_shared_prefix_chars {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    if candidate.len() < current.len().saturating_add(min_extension_chars) {
        return false;
    }

    let min_prefix_len = current.len().saturating_sub(max_edit_chars);
    let max_prefix_len = current
        .len()
        .saturating_add(max_edit_chars)
        .min(candidate.len());
    (min_prefix_len..=max_prefix_len).any(|candidate_prefix_len| {
        embedded_audio_partial_preview_edit_distance_at_most(
            &current,
            &candidate[..candidate_prefix_len],
            max_edit_chars,
        )
    })
}

fn embedded_audio_final_supplement_has_bounded_long_rewrite_alignment(
    current_key: &str,
    candidate_key: &str,
    shared_prefix_chars: usize,
    min_shared_prefix_chars: usize,
    max_length_delta_chars: usize,
    max_edit_chars: usize,
) -> bool {
    if shared_prefix_chars < min_shared_prefix_chars {
        return false;
    }

    let current: Vec<char> = current_key.chars().collect();
    let candidate: Vec<char> = candidate_key.chars().collect();
    current.len().abs_diff(candidate.len()) <= max_length_delta_chars
        && embedded_audio_partial_preview_edit_distance_at_most(
            &current,
            &candidate,
            max_edit_chars,
        )
}

fn embedded_audio_partial_preview_edit_distance_at_most(
    left: &[char],
    right: &[char],
    max_distance: usize,
) -> bool {
    if left.len().abs_diff(right.len()) > max_distance {
        return false;
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (left_index, left_char) in left.iter().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(left_index + 1);
        for (right_index, right_char) in right.iter().enumerate() {
            let replace_cost = previous[right_index] + usize::from(left_char != right_char);
            let insert_cost = current[right_index] + 1;
            let delete_cost = previous[right_index + 1] + 1;
            current.push(replace_cost.min(insert_cost).min(delete_cost));
        }
        if current.iter().copied().min().unwrap_or_default() > max_distance {
            return false;
        }
        previous = current;
    }

    previous[right.len()] <= max_distance
}

fn embedded_audio_partial_preview_common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn embedded_audio_partial_preview_common_suffix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

fn embedded_audio_partial_preview_stability_key(text: &str) -> String {
    let mut key = String::new();
    for ch in text.chars() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            key.push(lower);
        }
    }
    key
}

fn is_embedded_audio_partial_preview_decorative(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。'
                | '、'
                | '；'
                | '：'
                | '？'
                | '！'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '…'
                | '—'
        )
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
    for (index, ch) in activation_candidate.char_indices() {
        let next = index + ch.len_utf8();
        if is_embedded_audio_partial_preview_decorative(ch) {
            consumed_end = next;
            continue;
        }
        if phrase_index == phrase.len() {
            break;
        }
        if !wake_phrase_character_matches(ch, phrase[phrase_index]) {
            return strip_bounded_activation_suffix(activation_candidate, &phrase)
                .unwrap_or_else(|| text.to_string());
        }
        phrase_index += 1;
        consumed_end = next;
    }

    if phrase_index == phrase.len() {
        activation_candidate[consumed_end..]
            .trim_start_matches(is_embedded_audio_partial_preview_decorative)
            .trim()
            .to_string()
    } else if partial && phrase_index > 0 {
        String::new()
    } else {
        text.to_string()
    }
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
        latest_audio_ms: capsule_audio_boundary_ms,
        initial_body_wait_until_audio_ms: body_wait_armed_immediately.then_some(
            capsule_audio_boundary_ms.saturating_add(EMBEDDED_AUTOMATIC_BODY_INITIAL_WAIT_MS),
        ),
        // Start a wall-clock escape even while waiting for the frontend ACK.
        // An early capsule can already be visible before the accepted session
        // installs this guard, so that ACK may legitimately never repeat.
        initial_body_wait_started_at: Some(Instant::now()),
        body_started: false,
        stop_requested: false,
    });
}

/// Install the accepted automatic-session guard without losing an early
/// capsule visibility edge. The phrase detector may show Recording before the
/// owner gate accepts; product activation must bind that already-visible UI to
/// the same session instead of waiting for a second frontend transition.
fn arm_accepted_automatic_wake_text_guard(
    inner: &Arc<Inner>,
    session_id: SessionId,
    phrase: String,
    capsule_audio_boundary_ms: u64,
    early_capsule_session_id: Option<SessionId>,
) {
    arm_automatic_wake_text_guard(
        inner,
        session_id,
        phrase,
        capsule_audio_boundary_ms,
    );
    if early_capsule_session_id == Some(session_id) {
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
    // Before the capsule-visible acknowledgement no deadline is armed, so the
    // wait remains active even if an eager provider preview already found
    // body text. After acknowledgement, either positive body text or expiry
    // of the bounded audio/wall deadline releases the endpoint reducer.
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

fn filter_automatic_wake_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    text: &str,
    partial: bool,
) -> String {
    let (phrase, late_body_blocked) = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| guard.session_id == session_id)
        .map(|guard| {
            (
                Some(guard.phrase.clone()),
                guard.stop_requested && !guard.body_started,
            )
        })
        .unwrap_or((None, false));
    if late_body_blocked {
        log::debug!(
            "[wake-phrase] ignored late text after stop before body start session_id={session_id} partial={partial} chars={}",
            text.chars().count()
        );
        return String::new();
    }
    let filtered = phrase.map_or_else(
        || preserve_recording_transcript(text),
        |phrase| strip_automatic_activation_prefix(text, &phrase, partial),
    );
    if !filtered.trim().is_empty() {
        let mut slot = inner.embedded_audio_automatic_wake_guard.lock();
        if let Some(guard) = slot
            .as_mut()
            .filter(|guard| guard.session_id == session_id && !guard.body_started)
        {
            guard.body_started = true;
            log::info!(
                "[wake-phrase] automatic body started after capsule session_id={session_id}"
            );
        }
    }
    filtered
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
    let punctuation = gap
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>();
    if punctuation.is_empty() {
        return (!terminal).then_some(" ").unwrap_or_default().to_string();
    }
    let chosen = punctuation
        .iter()
        .rev()
        .find(|ch| matches!(ch, '。' | '！' | '？' | '.' | '!' | '?'))
        .or_else(|| punctuation.last())
        .copied();
    chosen.map(|ch| ch.to_string()).unwrap_or_default()
}

fn remove_standalone_dictation_fillers(text: &str) -> String {
    let text = text.trim();
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
    // Do not call `filter_automatic_wake_text`: a display-only tail must not
    // latch body_started or otherwise influence the endpoint state machine.
    let phrase = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| guard.session_id == session_id)
        .map(|guard| guard.phrase.clone());
    let text = phrase.map_or_else(
        || preserve_recording_transcript(text),
        |phrase| strip_automatic_activation_prefix(text, &phrase, true),
    );
    if inner.prefs.get().remove_filler_words {
        remove_standalone_dictation_fillers(&text)
    } else {
        text
    }
}

fn embedded_audio_partial_preview_repeats_recent_short_tail(
    current_key: &str,
    candidate_key: &str,
    current_key_chars: usize,
) -> bool {
    const MIN_CURRENT_CHARS: usize = 6;
    const MIN_SUFFIX_CHARS: usize = 2;
    const MAX_SUFFIX_CHARS: usize = 6;

    if current_key_chars < MIN_CURRENT_CHARS {
        return false;
    }

    let suffix_key: String = candidate_key.chars().skip(current_key_chars).collect();
    let suffix_chars = suffix_key.chars().count();
    if !(MIN_SUFFIX_CHARS..=MAX_SUFFIX_CHARS).contains(&suffix_chars) {
        return false;
    }
    if !suffix_key
        .chars()
        .all(is_embedded_audio_partial_preview_cjk)
    {
        return false;
    }

    current_key.ends_with(&suffix_key)
}

fn is_embedded_audio_partial_preview_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn stitch_embedded_audio_partial_preview(
    current: &str,
    candidate: &str,
    current_key_chars: usize,
) -> Option<String> {
    let suffix_start = byte_index_after_stability_chars(candidate, current_key_chars);
    let mut suffix = &candidate[suffix_start..];
    if suffix.is_empty() {
        return None;
    }
    if current
        .chars()
        .last()
        .is_some_and(is_embedded_audio_partial_preview_decorative)
    {
        suffix = suffix.trim_start_matches(is_embedded_audio_partial_preview_decorative);
    }
    if suffix.is_empty() {
        return None;
    }
    let mut stitched = current.to_string();
    stitched.push_str(suffix);
    Some(stitched)
}

fn byte_index_after_stability_chars(text: &str, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let mut seen = 0usize;
    for (idx, ch) in text.char_indices() {
        if is_embedded_audio_partial_preview_decorative(ch) {
            continue;
        }
        seen = seen.saturating_add(1);
        if seen == count {
            return idx + ch.len_utf8();
        }
    }
    text.len()
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
