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
    /// r36：主轨终稿是否为 speaker_filtered 认证封存（说话人过滤已切尾的
    /// 干净本人全文）。认证稿覆盖分离稿时分离稿必为截断/劣化（r36 吞字
    /// 形态：主轨 51 字完好 vs 分离稿截断 20 字被误当"排除"放行）。
    primary_speaker_filtered_certified: bool,
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

    // r24（2026-09-18，session 999a5b67）：干扰轮里 separated 解码受提取伪影拖累
    // 严重劣化（24 字/"第二步"，与主终稿内容分叉），却无条件压过第一仲裁已封存
    // 的正确 52 字 speaker_filtered 终稿——用户判"吞字"。分离轨的存在价值是归属
    // 排除：当主终稿只是比分离稿**多出一段尾巴**（分离稿是主终稿的紧凑子序列）
    // 时，分离轨仍在做排除工作，维持分离优先；两轨内容分叉且主终稿说出内容
    // 覆盖 ≥ 分离稿时，主终稿（标点/数字写法更好，r23）不应被劣化稿覆盖。
    // r36（2026-09-18 晚）补丁：分离稿劣化还有第二种形态——**截断成主轨前缀**
    // （主轨 51 字完好 vs 分离稿截断 20 字）。文本层面"前缀"与"排除尾巴后剩
    // 正文"无法区分，需要主轨的封存认证位：speaker_filtered 认证稿（说话人
    // 过滤已切尾的干净本人全文）只要覆盖分离稿，分离稿必为丢正文的劣化稿，
    // 直接降权；无认证（可能是未过滤的带尾原始稿）才退回子序列判别。
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
            && primary > separated
            && {
                let separated_text = candidates
                    .separated_owner
                    .as_ref()
                    .map(|candidate| candidate.text.as_str())
                    .unwrap_or_default();
                compact_subsequence_of(separated_text, &candidates.provider_primary.text)
            };
        if primary >= separated && !separated_still_excluding {
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
        target_filter_required: candidates.target_filter_required,
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
                strip_activation_after_short_lead_in(activation_candidate, &phrase)
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
        // Live afd9e265: 28-char final arrived after stop with no visual
        // latch, then late_body returned empty and the spoken sentence was
        // swallowed. Keep a stripped body even after stop; only drop true
        // wake-only leftovers.
        let filtered = phrase.as_deref().map_or_else(
            || preserve_recording_transcript(text),
            |phrase| strip_automatic_activation_prefix(text, phrase, false),
        );
        if automatic_wake_filtered_text_is_body(&filtered, text, phrase.as_deref()) {
            latch_automatic_wake_body_if_filtered(
                inner,
                session_id,
                &filtered,
                text,
                phrase.as_deref(),
            );
            log::info!(
                "[wake-phrase] kept spoken body after stop before visual latch session_id={session_id} chars={}",
                filtered.chars().count()
            );
            return filtered;
        }
        log::debug!(
            "[wake-phrase] ignored late text after stop before body start session_id={session_id} partial={partial} chars={}",
            text.chars().count()
        );
        return String::new();
    }
    let filtered = phrase.as_deref().map_or_else(
        || preserve_recording_transcript(text),
        |phrase| strip_automatic_activation_prefix(text, phrase, partial),
    );
    latch_automatic_wake_body_if_filtered(inner, session_id, &filtered, text, phrase.as_deref());
    filtered
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
    let phrase = inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .filter(|guard| guard.session_id == session_id)
        .map(|guard| guard.phrase.clone());
    let original = text;
    let phrase_for_latch = phrase.clone();
    let text = phrase.map_or_else(
        || preserve_recording_transcript(text),
        |phrase| strip_automatic_activation_prefix(text, &phrase, true),
    );
    latch_automatic_wake_body_if_filtered(
        inner,
        session_id,
        &text,
        original,
        phrase_for_latch.as_deref(),
    );
    // Filler cleanup belongs on the sealed insert, not the live capsule.
    // Stripping 嗯/呃 from short partials delayed the first visible body
    // and made the preview look like it was swallowing text.
    text
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
// 设计卡：work/pause-early-delivery-design-20260922.md。句末标点 + 账本稳定
// ≥ 耐心窗 → 稳定前缀当场走插入通道上屏；会话不结束，端点契约原样保留；
// 终稿只插余量（stability-key 前缀比对；云端改写前缀 → 跳过第二次插入，
// 宁可少不可重——H 族教训）。回退开关 LISTENER_DISABLE_PAUSE_EARLY_DELIVERY=1。

const PAUSE_EARLY_DELIVERY_MIN_STABLE: Duration =
    Duration::from_millis(1_000);

/// Session-scoped bookkeeping of text already inserted mid-session.
#[derive(Default)]
pub(super) struct PauseEarlyDeliveryLedger {
    pub(super) session_id: Option<SessionId>,
    /// Exact display text already on screen for this session.
    pub(super) delivered_display: String,
    /// stability key（去标点小写）of `delivered_display`.
    pub(super) delivered_key: String,
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
fn pause_early_final_remainder(final_text: &str, delivered_key: &str) -> Option<String> {
    if delivered_key.is_empty() {
        return Some(final_text.trim().to_string());
    }
    let mut seen_key = String::new();
    for (offset, ch) in final_text.char_indices() {
        if seen_key == delivered_key {
            // The delivered display already printed its terminal punctuation;
            // skip decorative separators at the split so they cannot print twice.
            return Some(
                final_text[offset..]
                    .trim_start_matches(is_embedded_audio_partial_preview_decorative)
                    .to_string(),
            );
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

/// 干净会话里云端改写了已交付前缀时的内容保全（2026-09-22 15:1x，0e9b79fc
/// 实锤 30 字落屏后终稿改写、16 字尾巴被丢）：按最长公共前缀定位分界，
/// 返回终稿在分界之后的尾巴（含被改写的字）。LCP 不足已交付一半时放弃
/// （改写太剧烈，接缝读不通，宁少勿乱）。终稿比已交付短时同样放弃。
fn pause_early_mismatch_recovery_tail(final_text: &str, delivered_key: &str) -> Option<String> {
    if delivered_key.is_empty() {
        return None;
    }
    let delivered_content_chars = delivered_key.chars().count();
    let mut seen_key = String::new();
    let mut divergence_offset: Option<usize> = None;
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
        if divergence_offset.is_none() && !delivered_key.starts_with(seen_key.as_str()) {
            divergence_offset = Some(offset);
        } else if divergence_offset.is_none() {
            lcp_chars += 1;
        }
        content_index += 1;
    }
    let Some(divergence_offset) = divergence_offset else {
        return None;
    };
    // 2026-09-23 13:33 panic 实锤（"…必须要报用内置浏览器看蓝湖"）：
    // 字节级 LCP 会在共享 UTF-8 前缀的同音字内部切分（报 E6 8A A5 /
    // 抱 E6 8A B1 共享前两字节），delivered_key[lcp..] 直接 panic 成
    // "内部错误"。按整字符推进，切点必落字符边界。
    let lcp_bytes: usize = delivered_key
        .chars()
        .take(lcp_chars)
        .map(char::len_utf8)
        .sum();
    // 微增长(≤3 字)无条件补,且先于半程卫兵:2026-09-23 15:29 实锤
    // delivered=23/final=24,深改写把 1 字新增吞掉——1-3 字的尾巴不可能
    // 是整段重述,且严格只追加交付长度之后的内容,物理上不可能重复。
    let growth_chars = content_index.saturating_sub(delivered_content_chars);
    if growth_chars > 0 && growth_chars <= 3 {
        return growth_offset.map(|offset| seam_deduped_growth_tail(final_text, offset, delivered_key));
    }
    if lcp_bytes * 2 < delivered_key.len() {
        return None;
    }
    let rewritten_tail_chars = delivered_content_chars - lcp_chars;
    if rewritten_tail_chars <= 2 {
        // 2026-09-22 21:5x 用户实锤"出来两次"后改版契约:改写只允许在交付
        // 末尾 ≤2 字(标点/同音边界级)时从分界补尾。
        return Some(final_text[divergence_offset..].to_string());
    }
    // 2026-09-23 14:31/14:32 连续实锤:云端两遍精修润色了中段一个字,旧契约
    // 把 2-16 字的纯新增尾巴一起丢掉——违反优先级锁第 3 条(不吞你的字)。
    // 增长尾分支:终稿内容更长且改写区(已交付-LCP)在 max(8 字, 已交付/3)
    // 以内时,从已交付长度处切齐补尾。严格只追加交付长度之后的内容,物理
    // 上不可能重复上屏;绝对差值口径让长句(105 字会话改写 22 字)也能补,
    // 而"出来两次"型整段重述(58 字改写 28 字)仍被挡在门外。
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

fn pause_early_delivery_reserve(
    inner: &Arc<Inner>,
    session_id: SessionId,
    delivered_display: String,
    delivered_key: String,
) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    ledger.session_id = Some(session_id);
    ledger.delivered_display = delivered_display;
    ledger.delivered_key = delivered_key;
}

fn pause_early_delivery_rollback(inner: &Arc<Inner>, session_id: SessionId) {
    let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
    if ledger.session_id.as_ref() == Some(&session_id) {
        *ledger = PauseEarlyDeliveryLedger::default();
    }
}

/// 终稿路径取走本会话的已交付前缀（display, key），取走即清零。
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
        *ledger = PauseEarlyDeliveryLedger::default();
        return taken;
    }
    None
}

/// pause-early 与组字流式共用的显示变换链(2026-09-22 切片3 设计卡铁律:
/// 两条路径必须逐字同款,stability-key 记账才连续)。门失败返回 None。
/// 2026-09-22 14:2x 实测修正沿袭:流式尾句不带句末标点(云端终稿才补),
/// 不设句末标点门——账本稳定窗本身即"这句说完了"的充分证据,stability
/// key 对标点免疫,终稿补的句号不会双插。
fn pause_early_display_text(
    inner: &Arc<Inner>,
    session_id: SessionId,
    raw_text: &str,
) -> Option<String> {
    let text = filter_automatic_wake_text(inner, session_id, raw_text, false);
    let prefs = inner.prefs.get();
    let text = if prefs.remove_filler_words {
        remove_standalone_dictation_fillers(&text)
    } else {
        text
    };
    let translation_active = inner.translation_modifier_seen.load(Ordering::SeqCst)
        && !prefs.translation_target_language.trim().is_empty();
    let force_raw_output = std::env::var("LISTENER_TYPE_FORCE_RAW_OUTPUT")
        .map(|value| value == "1")
        .unwrap_or(false);
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
    if translation_active || pack.base_mode != PolishMode::Raw || raw_uses_llm {
        pause_early_note_gate_blocked(inner, session_id, "polish_or_translation_active");
        return None;
    }
    let text = apply_chinese_script_preference(&text, prefs.chinese_script_preference);
    let correction_rules = inner.correction_rules.list().unwrap_or_default();
    let text = apply_correction_rules(&text, &correction_rules);
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Watchdog 每拍评估（无 STOP 待决时）。条件全满足才动作：
/// 干净会话（ASR 账本卫兵：无冻结/无非主人窗）+ 账本稳定 ≥1s +
/// Raw 一次性路径（翻译/LLM 会整体改写，前缀会被孤立）+ 有新增内容 +
/// 原焦点目标可恢复。组字流式活着时改走 stream_commit 落定(不碰焦点,
/// 组字锚在目标进程);任何失败静默回退到现行为(终稿一次性交付)。
async fn pause_early_delivery_tick(
    inner: &Arc<Inner>,
    session_id: SessionId,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
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
    let Some(snapshot) = asr.pause_early_delivery_ledger_snapshot(PAUSE_EARLY_DELIVERY_MIN_STABLE)
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
    {
        let mut ledger = inner.embedded_audio_pause_early_delivery.lock();
        if ledger.qualified_diag_logged.as_ref() != Some(&session_id) {
            ledger.qualified_diag_logged = Some(session_id);
            log::info!(
                "[coord] pause-early snapshot qualified session_id={session_id} chars={}",
                snapshot.text.chars().count()
            );
        }
    }
    // 显示侧变换必须镜像终稿 Raw 路径，前缀才能与终稿逐字可比（标点差由
    // stability key 容忍）。组字流式(切片3)与这里共用同一条链。
    let Some(text) = pause_early_display_text(inner, session_id, &snapshot.text) else {
        return;
    };
    let prefs = inner.prefs.get();
    let key = embedded_audio_partial_preview_stability_key(&text);
    let (delivered_display, delivered_key) =
        pause_early_delivery_session_state(inner, session_id);
    if !key.starts_with(&delivered_key) || key.len() == delivered_key.len() {
        return;
    }
    let Some(delta) = pause_early_final_remainder(&text, &delivered_key) else {
        return;
    };
    if delta.is_empty() {
        return;
    }
    // 组字流式活着 → 落定走 stream_commit(组字锚在目标进程,不需要焦点
    // 恢复;账本先记后 commit,失败回滚——次序与粘贴路径同款)。失败时驱动
    // 已降级清组字,下一拍以空账本走粘贴分支补上。
    if streaming_composition_active(inner, session_id) {
        streaming_composition_commit_stable(inner, session_id, &delta, &delivered_display, &key)
            .await;
        return;
    }
    // 组字降级且清场也失败:文档里可能有残留组字,粘贴会重复,宁停手。
    if streaming_composition_contaminated(inner, session_id) {
        log::warn!(
            "[coord] pause-early-delivery skipped: streaming composition contaminated session_id={session_id}"
        );
        return;
    }
    let (focus_target, focus_target_title) = {
        let state = inner.state.lock();
        (state.focus_target, state.focus_target_title.clone())
    };
    if !restore_focus_target_if_possible(focus_target, focus_target_title.as_deref()) {
        log::info!(
            "[coord] pause-early-delivery skipped: focus target unavailable session_id={session_id}"
        );
        return;
    }
    // 先记账再粘贴：并发触发的终稿交付会按在途前缀计算余量；粘贴失败回滚。
    let new_display = format!("{delivered_display}{delta}");
    pause_early_delivery_reserve(inner, session_id, new_display, key.clone());
    // 2026-09-23 tki:中途粘贴不恢复剪贴板。750ms 恢复窗口追不上忙碌目标
    // (VS Code/Chromium 渲染器)的粘贴派发——08:39 实锤:第 1 段 22 字正确,
    // 第 2/3 段粘出的是用户 08:22 复制的旧报告×2(账本记 14+14 字已交付,
    // 落屏的却是恢复后的旧剪贴板)。会话内剪贴板暂存听写增量无碍;终稿
    // 交付仍按用户偏好恢复,那才是剪贴板所有权归还的正当时机。
    let restore_clipboard = false;
    let result = insert_via_non_tsf_fallback(
        inner,
        &delta,
        restore_clipboard,
        prefs.paste_shortcut,
    );
    let inserted = matches!(
        result.status,
        InsertStatus::PasteSent | InsertStatus::Inserted | InsertStatus::SubmittedUnconfirmed
    );
    if !inserted {
        pause_early_delivery_rollback(inner, session_id);
        log::warn!(
            "[coord] pause-early-delivery paste failed status={:?} chars={} — final will deliver in full",
            result.status,
            delta.chars().count()
        );
        return;
    }
    log::info!(
        "[coord] pause-early-delivery prefix_chars={} total_delivered_chars={} min_stable_ms={} route={:?} status={:?}",
        delta.chars().count(),
        delivered_display.chars().count() + delta.chars().count(),
        PAUSE_EARLY_DELIVERY_MIN_STABLE.as_millis(),
        result.route,
        result.status
    );
}
