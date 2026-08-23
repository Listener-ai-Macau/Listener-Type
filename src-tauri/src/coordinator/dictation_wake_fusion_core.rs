#[cfg(target_os = "windows")]
fn local_confirmation_can_activate(
    has_keyword_model_hit: bool,
    relation: crate::wake_phrase::LocalPhraseRelation,
) -> bool {
    denzic_voice_activation_v1_core::local_confirmation_can_activate(
        has_keyword_model_hit,
        relation,
    )
}

#[cfg(target_os = "windows")]
fn secondary_fallback_can_accept_keyword(
    keyword_model_hit: bool,
    explicit_absent_count: u8,
) -> bool {
    matches!(
        denzic_voice_activation_v1_core::decide_secondary_fallback(
            denzic_voice_activation_v1_core::SecondaryFallbackInput {
                keyword_model_hit,
                explicit_absent_count,
                secondary_unavailable_or_timed_out: true,
            },
        ),
        denzic_voice_activation_v1_core::SecondaryFallbackDecision::AcceptKeywordModel
    )
}

