//! Pure, replayable product decisions for the voice-input path.
//!
//! Adapters may collect evidence in parallel, but they must ask this module for
//! the terminal product decision. In particular, an open (unenrolled) owner
//! policy is allowed to wake without being mislabeled as a verified owner.

use crate::coordinator_state::SessionId;
use denzic_voice_activation_v1_core::{decide_gate, GateDecision, GateInput, PhraseSignal};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerAccessEvidence {
    /// There is no persisted profile. Phrase evidence may open the product, but
    /// downstream owner-only filtering must stay disabled.
    OpenUnenrolled,
    /// A stored profile exists but is deliberately inactive for the configured
    /// phrase. This keeps legacy open-gate behavior explicit during re-enrolment.
    OpenInactiveProfile,
    EnrolledMatch,
    EnrolledNonMatch,
    /// Credential/model/schema failures fail closed and are never treated as
    /// absence of enrollment.
    Unavailable,
}

impl OwnerAccessEvidence {
    const fn gate_pass(self) -> Option<bool> {
        match self {
            Self::OpenUnenrolled | Self::OpenInactiveProfile | Self::EnrolledMatch => Some(true),
            Self::EnrolledNonMatch => Some(false),
            Self::Unavailable => None,
        }
    }

    pub(crate) const fn enrolled_owner_verified(self) -> bool {
        matches!(self, Self::EnrolledMatch)
    }

    pub(crate) const fn policy_label(self) -> &'static str {
        match self {
            Self::OpenUnenrolled => "open_unenrolled",
            Self::OpenInactiveProfile => "open_inactive_profile",
            Self::EnrolledMatch => "enrolled_match",
            Self::EnrolledNonMatch => "enrolled_non_match",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WakeArbitration {
    pub(crate) decision: GateDecision,
    pub(crate) owner_access: OwnerAccessEvidence,
}

pub(crate) fn arbitrate_wake(
    phrase_signal: PhraseSignal,
    owner_access: OwnerAccessEvidence,
    terminal: bool,
) -> WakeArbitration {
    let mut decision = decide_gate(GateInput {
        phrase_signal,
        owner_match: owner_access.gate_pass(),
        terminal,
    });
    // Product bias: keep wake over false-wake. A first voiceprint miss
    // must not throw away ExactStart/PresentLater/KeywordModel.
    if matches!(decision, GateDecision::Reject | GateDecision::Pending)
        && !matches!(phrase_signal, PhraseSignal::None)
        && matches!(owner_access, OwnerAccessEvidence::EnrolledNonMatch)
    {
        decision = GateDecision::Accept;
    }
    // VoiceActivation only means the device detected speech. Owner identity
    // cannot replace activation intent, even when that transport segment ends.
    // Phrase/phonetic recovery runs before this boundary and supplies a real
    // phrase signal; without one, retain Pending/Reject from the core gate.
    WakeArbitration {
        decision,
        owner_access,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderTextRevision {
    pub(crate) revision: u64,
    pub(crate) text: String,
    pub(crate) audio_coverage_ms: Option<u64>,
    pub(crate) diarization_present: bool,
    pub(crate) final_frame: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitSource {
    OwnerCandidate,
    ProviderRawFallback,
    Empty,
}

impl CommitSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::OwnerCandidate => "owner_candidate",
            Self::ProviderRawFallback => "provider_raw_fallback",
            Self::Empty => "empty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptCommit {
    pub(crate) text: String,
    pub(crate) source: CommitSource,
}

/// Sole authority for selecting the text representation at a provider's
/// protocol-final boundary. Provider adapters may calculate candidate-safety
/// facts, but they must not independently choose between raw, filtered and
/// optimistic text through overlapping boolean branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalTranscriptAuthority {
    SpeakerFiltered,
    ProviderRawRecovery,
    ProviderOwnerRecovery,
    SessionLedgerRecovery,
    OptimisticOwnerRecovery,
}

impl FinalTranscriptAuthority {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::SpeakerFiltered => "speaker_filtered",
            Self::ProviderRawRecovery => "provider_raw_recovery",
            Self::ProviderOwnerRecovery => "provider_owner_recovery",
            Self::SessionLedgerRecovery => "session_ledger_recovery",
            Self::OptimisticOwnerRecovery => "optimistic_owner_recovery",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FinalTranscriptEvidence {
    pub(crate) protocol_final: bool,
    /// Provider diarization and a time-aligned local owner mismatch agree that
    /// the candidate tail belongs to somebody else. This is a veto, not a
    /// request to delete already accepted owner text.
    pub(crate) explicit_non_owner_tail: bool,
    pub(crate) provider_raw_recovery_safe: bool,
    pub(crate) provider_owner_recovery_safe: bool,
    pub(crate) session_ledger_recovery_safe: bool,
    pub(crate) optimistic_owner_recovery_safe: bool,
}

pub(crate) fn arbitrate_final_transcript(
    evidence: FinalTranscriptEvidence,
) -> FinalTranscriptAuthority {
    if !evidence.protocol_final || evidence.explicit_non_owner_tail {
        return FinalTranscriptAuthority::SpeakerFiltered;
    }
    if evidence.provider_raw_recovery_safe {
        return FinalTranscriptAuthority::ProviderRawRecovery;
    }
    if evidence.provider_owner_recovery_safe {
        return FinalTranscriptAuthority::ProviderOwnerRecovery;
    }
    if evidence.session_ledger_recovery_safe {
        return FinalTranscriptAuthority::SessionLedgerRecovery;
    }
    if evidence.optimistic_owner_recovery_safe {
        return FinalTranscriptAuthority::OptimisticOwnerRecovery;
    }
    FinalTranscriptAuthority::SpeakerFiltered
}

/// Sole product-boundary authority for choosing which already-produced text
/// candidate may become the dictation result. Provider diarization, owner-only
/// separation, retained-audio replay, the capsule preview and local shadow ASR
/// are evidence producers; none of them may write the final text directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductFinalAuthority {
    SeparatedOwner,
    ProviderPrimary,
    RetainedAudioReplay,
    DebugOverride,
    PartialPreviewRecovery,
    Empty,
}

impl ProductFinalAuthority {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::SeparatedOwner => "separated_owner",
            Self::ProviderPrimary => "provider_primary",
            Self::RetainedAudioReplay => "retained_audio_replay",
            Self::DebugOverride => "debug_override",
            Self::PartialPreviewRecovery => "partial_preview_recovery",
            Self::Empty => "empty",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProductFinalEvidence {
    pub(crate) target_filter_required: bool,
    pub(crate) separated_owner_available: bool,
    pub(crate) provider_primary_available: bool,
    pub(crate) retained_audio_replay_available: bool,
    pub(crate) debug_override_available: bool,
    pub(crate) partial_preview_available: bool,
}

pub(crate) fn arbitrate_product_final(evidence: ProductFinalEvidence) -> ProductFinalAuthority {
    if evidence.separated_owner_available {
        return ProductFinalAuthority::SeparatedOwner;
    }
    if evidence.provider_primary_available {
        return ProductFinalAuthority::ProviderPrimary;
    }
    // Once upstream ownership filtering is required, no unsegmented replay,
    // preview, or local shadow candidate is safe enough to become final text.
    if evidence.target_filter_required {
        return ProductFinalAuthority::Empty;
    }
    if evidence.retained_audio_replay_available && !evidence.target_filter_required {
        return ProductFinalAuthority::RetainedAudioReplay;
    }
    if evidence.partial_preview_available {
        return ProductFinalAuthority::PartialPreviewRecovery;
    }
    if evidence.debug_override_available {
        return ProductFinalAuthority::DebugOverride;
    }
    ProductFinalAuthority::Empty
}

pub(crate) const OPEN_SESSION_WAKE_OWNED_MIN_BODY_CHARS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OpenSessionWakeOwnedBodyEvidence {
    pub(crate) tracking_enabled: bool,
    /// The wake was accepted without bank verification (open acceptance or a
    /// drifted enrolled non-match), so the session admitted it could not
    /// verify the waker.
    pub(crate) wake_owner_verified: bool,
    pub(crate) owner_isolation_frozen: bool,
    /// A local hard NonTarget window latched during the body. Media and a
    /// genuinely foreign speaker both produce this; the veto keeps standing.
    pub(crate) hard_non_target_latched: bool,
    /// Speaker filtering collapsed to exactly the wake phrase.
    pub(crate) filtered_is_wake_only: bool,
    /// The provider transcript starts with the wake phrase and carries at
    /// least OPEN_SESSION_WAKE_OWNED_MIN_BODY_CHARS of body after it.
    pub(crate) provider_has_wake_anchored_body: bool,
}

/// 2026-09-20 sessions 0dbc59da / 86768c0e: the wake itself failed bank
/// verification (open acceptance after an enrolled non-match), the provider
/// then transcribed 44/47 chars of continuous wake-anchored dictation, and
/// cloud diarization split the user's own body into a second stable cluster.
/// Every bank-relative judgment inherited the wake verifier's failure on the
/// true owner, so the final arbiter collapsed to the wake phrase, the
/// wake-phrase strip emptied the delivery, and the product reported
/// "没有识别到语音" for speech it had already previewed. A session that
/// admitted it could not verify the waker may not use the same verifier to
/// prove the body is somebody else: the wake-anchored continuous body is
/// session-owned, the same preference the wake-only schema gap already makes
/// for adaptive profiles. A bank-verified wake keeps the full strict
/// isolation; a latched hard NonTarget window (media, a real second speaker)
/// still vetoes.
pub(crate) const fn open_session_wake_owned_body_can_recover(
    evidence: OpenSessionWakeOwnedBodyEvidence,
) -> bool {
    evidence.tracking_enabled
        && !evidence.wake_owner_verified
        && !evidence.owner_isolation_frozen
        && !evidence.hard_non_target_latched
        && evidence.filtered_is_wake_only
        && evidence.provider_has_wake_anchored_body
}

/// Transcript ownership evidence is deliberately separate from the wake and
/// endpoint classification. Short, high-energy windows are too small to erase
/// text on their own, but two of them may corroborate a provider-final speaker
/// change. This prevents changing the global endpoint sensitivity to solve a
/// transcript-only failure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TranscriptSpeakerEvidence {
    #[default]
    Inconclusive,
    ForeignTailHint,
    HardNonTarget,
}

impl TranscriptSpeakerEvidence {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Inconclusive => "inconclusive",
            Self::ForeignTailHint => "foreign_tail_hint",
            Self::HardNonTarget => "hard_non_target",
        }
    }
}

const TRANSCRIPT_SPEAKER_MIN_SIGNAL_PEAK_RMS: f32 = 512.0;
const TRANSCRIPT_SPEAKER_FOREIGN_HINT_MIN_MS: usize = 300;
const TRANSCRIPT_SPEAKER_HARD_NON_TARGET_MIN_MS: usize = 600;
const TRANSCRIPT_SPEAKER_FOREIGN_MAX_SCORE: f32 = 0.10;

pub(crate) fn classify_transcript_speaker_evidence(
    score: f32,
    real_speech_ms: usize,
    peak_rms: f32,
) -> TranscriptSpeakerEvidence {
    if score > TRANSCRIPT_SPEAKER_FOREIGN_MAX_SCORE
        || real_speech_ms < TRANSCRIPT_SPEAKER_FOREIGN_HINT_MIN_MS
        || peak_rms < TRANSCRIPT_SPEAKER_MIN_SIGNAL_PEAK_RMS
    {
        return TranscriptSpeakerEvidence::Inconclusive;
    }
    if real_speech_ms >= TRANSCRIPT_SPEAKER_HARD_NON_TARGET_MIN_MS {
        TranscriptSpeakerEvidence::HardNonTarget
    } else {
        TranscriptSpeakerEvidence::ForeignTailHint
    }
}

/// Session-local, append-only provider evidence. Text is never persisted by
/// this type; it exists only for the lifetime of the ASR session.
#[derive(Debug, Default)]
pub(crate) struct TranscriptEvidenceLedger {
    revisions: Vec<ProviderTextRevision>,
    next_revision: u64,
    committed: bool,
}

fn meaningful_char_count(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

impl TranscriptEvidenceLedger {
    pub(crate) fn reset(&mut self) {
        self.revisions.clear();
        self.next_revision = 0;
        self.committed = false;
    }

    pub(crate) fn note_provider_revision(
        &mut self,
        text: &str,
        audio_coverage_ms: Option<u64>,
        diarization_present: bool,
        final_frame: bool,
    ) {
        let unchanged = self.revisions.last().is_some_and(|last| {
            last.text == text
                && last.audio_coverage_ms == audio_coverage_ms
                && last.diarization_present == diarization_present
                && last.final_frame == final_frame
        });
        if unchanged {
            return;
        }
        self.next_revision = self.next_revision.saturating_add(1);
        self.revisions.push(ProviderTextRevision {
            revision: self.next_revision,
            text: text.to_string(),
            audio_coverage_ms,
            diarization_present,
            final_frame,
        });
    }

    pub(crate) fn revision_count(&self) -> usize {
        self.revisions.len()
    }

    pub(crate) fn best_raw_text(&self) -> Option<&str> {
        self.revisions
            .iter()
            .filter(|revision| !revision.text.trim().is_empty())
            .max_by_key(|revision| meaningful_char_count(&revision.text))
            .map(|revision| revision.text.as_str())
    }

    /// Claims the sole terminal transcript delivery for this provider session.
    /// A caller may use raw fallback only after its ownership policy has proved
    /// there is no explicit non-target exclusion for the session.
    pub(crate) fn commit_once(
        &mut self,
        owner_candidate: &str,
        allow_provider_raw_fallback: bool,
    ) -> Option<TranscriptCommit> {
        if self.committed {
            return None;
        }
        self.committed = true;
        if !owner_candidate.trim().is_empty() {
            return Some(TranscriptCommit {
                text: owner_candidate.to_string(),
                source: CommitSource::OwnerCandidate,
            });
        }
        if allow_provider_raw_fallback {
            if let Some(raw) = self.best_raw_text() {
                return Some(TranscriptCommit {
                    text: raw.to_string(),
                    source: CommitSource::ProviderRawFallback,
                });
            }
        }
        Some(TranscriptCommit {
            text: String::new(),
            source: CommitSource::Empty,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EndpointEvidence {
    pub(crate) owner_watermark_ms: Option<u64>,
    pub(crate) provider_coverage_ms: Option<u64>,
    pub(crate) pending_provider_text: bool,
    pub(crate) latest_speech_confirmed_non_target: bool,
    pub(crate) unresolved_owner_tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointDecision {
    Hold,
    AwaitingOwnerAnalysis,
    CatchingUp,
    Stop,
}

/// Evidence for keeping the firmware's independent silence endpoint aligned
/// with the host endpoint arbiter.  The firmware cannot see provider lag or
/// speaker identity; it only sees a `VREC:SPEECH` lease renewal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FirmwareEndpointLeaseEvidence {
    pub(crate) visible_body: bool,
    pub(crate) owner_established: bool,
    /// Positive enrolled-owner activity watermark. Generic room-energy/VAD
    /// must never be passed to the firmware lease decision.
    pub(crate) owner_speech_watermark_ms: Option<u64>,
    pub(crate) provider_coverage_ms: Option<u64>,
    pub(crate) unresolved_owner_tail: bool,
    pub(crate) latest_speech_confirmed_non_target: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FirmwareEndpointLeaseDecision {
    Idle,
    Renew,
}

/// Renew only for a bounded, owner-compatible speech tail which the provider
/// has not covered yet.  This is deliberately asymmetric: uncertain owner
/// speech gets a short chance to survive provider lag, while explicit other
/// speaker evidence and owner-less room noise can never extend recording.
pub(crate) fn decide_firmware_endpoint_lease(
    evidence: FirmwareEndpointLeaseEvidence,
) -> FirmwareEndpointLeaseDecision {
    if !evidence.visible_body
        || !evidence.owner_established
        || !evidence.unresolved_owner_tail
        || evidence.latest_speech_confirmed_non_target
    {
        return FirmwareEndpointLeaseDecision::Idle;
    }
    let Some(owner_speech_ms) = evidence.owner_speech_watermark_ms else {
        return FirmwareEndpointLeaseDecision::Idle;
    };
    if evidence
        .provider_coverage_ms
        .is_some_and(|provider_ms| provider_ms >= owner_speech_ms)
    {
        FirmwareEndpointLeaseDecision::Idle
    } else {
        FirmwareEndpointLeaseDecision::Renew
    }
}

#[derive(Debug)]
enum EndpointPhase {
    Listening,
    CandidateEnd {
        owner_watermark_ms: Option<u64>,
        text_revision: u64,
    },
    CatchingUp {
        owner_watermark_ms: Option<u64>,
        text_revision: u64,
        deadline: Instant,
    },
}

/// The only lifecycle owned by the visible recording endpoint.
///
/// Wake-candidate creation and the outer coordinator's `SessionPhase` remain
/// separate concerns. Once a visible body exists, however, every endpoint
/// decision must be observable as one of these states; provider callbacks,
/// preview revisions, and firmware leases are inputs, never additional
/// lifecycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerEndpointState {
    OwnerActive,
    OwnerEvidencePending,
    QuietPending,
}

impl Default for EndpointPhase {
    fn default() -> Self {
        Self::Listening
    }
}

/// A bounded catch-up barrier between silence detection and irreversible stop.
/// It does not replace the product's normal inactivity timeout; it only closes
/// the callback-order race where local speech/provider text is still ahead of
/// the owner boundary at the instant that timeout expires.
#[derive(Debug, Default)]
pub(crate) struct OwnerEndpointController {
    phase: EndpointPhase,
    text_revision: u64,
    owner_analysis_deadline: Option<Instant>,
}

/// Session-scoped preview reducer.
///
/// Provider partials, two-pass supplements and provisional diarization text
/// arrive on independent callbacks. They must never own independent preview
/// slots: a late callback from session N could otherwise overwrite session
/// N+1, while an authoritative value equal to the previous ledger could
/// re-emit text already shown by the provisional path. This controller makes
/// identity admission and the visible/authoritative relationship atomic.
#[derive(Debug, Default)]
pub(crate) struct RecordingPreviewController {
    session_id: Option<SessionId>,
    authoritative: Option<String>,
    visible: Option<String>,
    last_visible_growth_at: Option<Instant>,
    /// Once ownership arbitration invalidates the authoritative preview, a
    /// late callback from the same provider session must not resurrect it.
    /// A new session (or clear_session) reopens admission.
    authoritative_invalidated: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RecordingPreviewReduction {
    pub(crate) authoritative_changed: bool,
    pub(crate) visible_update: Option<String>,
}

impl RecordingPreviewController {
    pub(crate) fn begin_session(&mut self, session_id: SessionId) {
        if self.session_id == Some(session_id) {
            return;
        }
        self.session_id = Some(session_id);
        self.authoritative = None;
        self.visible = None;
        self.last_visible_growth_at = None;
        self.authoritative_invalidated = false;
    }

    pub(crate) fn clear_session(&mut self, session_id: SessionId) -> bool {
        if self.session_id != Some(session_id) {
            return false;
        }
        self.session_id = None;
        self.authoritative = None;
        self.visible = None;
        self.last_visible_growth_at = None;
        self.authoritative_invalidated = false;
        true
    }

    pub(crate) fn authoritative(&self, session_id: SessionId) -> Option<String> {
        (self.session_id == Some(session_id))
            .then(|| self.authoritative.clone())
            .flatten()
    }

    pub(crate) fn invalidate_authoritative(&mut self, session_id: SessionId) -> bool {
        if self.session_id != Some(session_id) {
            return false;
        }
        self.authoritative_invalidated = true;
        self.clear_authoritative(session_id)
    }

    pub(crate) fn last_visible_growth_at(&self, session_id: SessionId) -> Option<Instant> {
        (self.session_id == Some(session_id))
            .then_some(self.last_visible_growth_at)
            .flatten()
    }

    pub(crate) fn visible(&self, session_id: SessionId) -> Option<String> {
        (self.session_id == Some(session_id))
            .then(|| self.visible.clone().or_else(|| self.authoritative.clone()))
            .flatten()
    }

    pub(crate) fn observe_authoritative(
        &mut self,
        session_id: SessionId,
        candidate: &str,
        settle_visible: bool,
    ) -> RecordingPreviewReduction {
        if !self.admit(session_id) {
            return RecordingPreviewReduction::default();
        }
        if self.authoritative_invalidated {
            return RecordingPreviewReduction::default();
        }
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return RecordingPreviewReduction {
                // An ordinary empty provider terminal clears the current
                // candidate but is not an ownership revocation. A later
                // accepted revision in the same live session must still be
                // admissible; explicit invalidation is the only fenced path.
                authoritative_changed: self.clear_authoritative(session_id),
                visible_update: None,
            };
        }

        let authoritative_changed = self.authoritative.as_deref() != Some(candidate);
        if authoritative_changed {
            self.authoritative = Some(candidate.to_string());
        }

        // Dictation F vs G: never retract already-shown spoken content.
        // Isolation may refuse to ADD later other-speaker text; it must not
        // replace a 69-char owner preview with a 19-char "filtered" string.
        // Tail drop is allowed only when the new text is a spoken prefix of
        // what is already visible AND shorter — that is still a retract, so
        // keep the high-water mark for dictation. Confirmed-other energy is
        // handled by the endpoint clock, not by shrinking the capsule.
        let would_shrink = self
            .visible
            .as_ref()
            .is_some_and(|current| spoken_preview_len(candidate) < spoken_preview_len(current));
        let visible_changed = self.visible.as_deref() != Some(candidate);
        let visible_update = if would_shrink {
            None
        } else if visible_changed && (authoritative_changed || settle_visible) {
            let candidate = candidate.to_string();
            self.visible = Some(candidate.clone());
            // Punctuation-only revisions must rearm the hang clock. Session
            // 41b3de62 added 。 without new alphanumeric chars, so the 1s
            // body clock kept counting from the previous CJK char and cut
            // the owner mid-utterance.
            self.last_visible_growth_at = Some(Instant::now());
            Some(candidate)
        } else {
            None
        };

        RecordingPreviewReduction {
            authoritative_changed,
            visible_update,
        }
    }

    pub(crate) fn observe_provisional(
        &mut self,
        session_id: SessionId,
        candidate: &str,
    ) -> Option<String> {
        if !self.admit(session_id) {
            return None;
        }
        if self.authoritative_invalidated {
            return None;
        }
        let candidate = candidate.trim();
        if candidate.is_empty() || self.visible.as_deref() == Some(candidate) {
            return None;
        }
        if self
            .visible
            .as_deref()
            .is_some_and(|current| spoken_preview_len(candidate) < spoken_preview_len(current))
        {
            return None;
        }
        let candidate = candidate.to_string();
        self.visible = Some(candidate.clone());
        self.last_visible_growth_at = Some(Instant::now());
        Some(candidate)
    }

    fn admit(&mut self, session_id: SessionId) -> bool {
        match self.session_id {
            Some(current) => current == session_id,
            // Session admission is owned by the coordinator's explicit
            // begin_session transition. A callback after clear_session must
            // not create a new session from an old session id.
            None => false,
        }
    }

    fn clear_authoritative(&mut self, session_id: SessionId) -> bool {
        if self.session_id != Some(session_id) {
            return false;
        }
        self.authoritative.take().is_some()
    }
}

fn spoken_preview_len(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

/// Product-level recording lifecycle.  Evidence producers (firmware VAD,
/// wake KWS/voiceprint, provider diarization and the endpoint clock) may run
/// concurrently, but they are not allowed to create their own lifecycle.
/// This reducer is the single owner of the capture boundary and makes stop
/// idempotent across callback, watchdog and cancellation races.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingLifecycleState {
    Idle,
    WakeCandidate,
    Active,
    Stopping,
    Closed,
}

#[derive(Debug, Default)]
pub(crate) struct RecordingLifecycleController {
    state: RecordingLifecycleState,
    embedded_session_id: Option<u32>,
    coordinator_session_id: Option<SessionId>,
    /// Non-None only for the automatic endpoint transaction that owns the
    /// current Stopping transition. Manual/device stops keep this None so a
    /// late automatic task cannot reopen their lifecycle.
    stop_owner_attempt_id: Option<u64>,
    /// A physical key press may arrive before the BLE actor has published its
    /// hidden wake candidate.  Keep that intent beside the candidate identity
    /// so it cannot be consumed by a later, unrelated segment.
    device_key_takeover_pending: bool,
    candidate_promotion_requested: bool,
}

impl Default for RecordingLifecycleState {
    fn default() -> Self {
        Self::Idle
    }
}

impl RecordingLifecycleController {
    #[cfg(test)]
    fn state(&self) -> RecordingLifecycleState {
        self.state
    }

    pub(crate) fn begin_candidate(&mut self, embedded_session_id: u32) -> bool {
        if embedded_session_id == 0 {
            return false;
        }
        match self.state {
            RecordingLifecycleState::Idle | RecordingLifecycleState::Closed => {
                self.state = RecordingLifecycleState::WakeCandidate;
                self.embedded_session_id = Some(embedded_session_id);
                self.coordinator_session_id = None;
                self.stop_owner_attempt_id = None;
                self.candidate_promotion_requested = self.device_key_takeover_pending;
                self.device_key_takeover_pending = false;
                true
            }
            RecordingLifecycleState::WakeCandidate
                if self.embedded_session_id == Some(embedded_session_id) =>
            {
                true
            }
            _ => false,
        }
    }

    /// Start a user-initiated recording. A hidden wake candidate must use
    /// `promote_candidate_to_owner`; keeping the transitions separate makes
    /// it impossible for evidence producers to bypass the wake reducer.
    pub(crate) fn begin_manual_owner(
        &mut self,
        embedded_session_id: u32,
        coordinator_session_id: SessionId,
    ) -> bool {
        if embedded_session_id == 0 {
            return false;
        }
        match self.state {
            RecordingLifecycleState::Idle | RecordingLifecycleState::Closed => {
                self.embedded_session_id = Some(embedded_session_id);
                self.coordinator_session_id = Some(coordinator_session_id);
                self.state = RecordingLifecycleState::Active;
                self.stop_owner_attempt_id = None;
                self.device_key_takeover_pending = false;
                self.candidate_promotion_requested = false;
                true
            }
            RecordingLifecycleState::Active
                if self.coordinator_session_id == Some(coordinator_session_id) =>
            {
                true
            }
            _ => false,
        }
    }

    pub(crate) fn promote_candidate_to_owner(
        &mut self,
        embedded_session_id: u32,
        coordinator_session_id: SessionId,
    ) -> bool {
        if self.state != RecordingLifecycleState::WakeCandidate
            || self.embedded_session_id != Some(embedded_session_id)
        {
            return false;
        }
        self.coordinator_session_id = Some(coordinator_session_id);
        self.state = RecordingLifecycleState::Active;
        self.stop_owner_attempt_id = None;
        self.device_key_takeover_pending = false;
        self.candidate_promotion_requested = false;
        true
    }

    pub(crate) fn hidden_candidate_active(&self) -> bool {
        self.state == RecordingLifecycleState::WakeCandidate && !self.candidate_promotion_requested
    }

    pub(crate) fn current_candidate_session_id(&self) -> Option<u32> {
        (self.state == RecordingLifecycleState::WakeCandidate)
            .then_some(self.embedded_session_id)
            .flatten()
    }

    /// Release a leftover WakeCandidate/Active/Stopping lock when the BLE
    /// actor has no session and no speaker candidate. Live 2462580845 stayed
    /// Active after a dropped end-packet, so every later 开始录音 was rejected.
    pub(crate) fn force_close_stale_lock(&mut self) {
        if matches!(
            self.state,
            RecordingLifecycleState::Idle | RecordingLifecycleState::Closed
        ) {
            return;
        }
        self.state = RecordingLifecycleState::Closed;
        self.embedded_session_id = None;
        self.coordinator_session_id = None;
        self.stop_owner_attempt_id = None;
        self.candidate_promotion_requested = false;
        self.device_key_takeover_pending = false;
    }

    /// A delayed physical STOP for a rejected candidate is safe only while the
    /// exact candidate tombstone still owns the lifecycle.  Merely observing
    /// that there is no *new candidate* is insufficient: the product may
    /// already have promoted or started an owner session during the delay.
    pub(crate) fn rejected_candidate_still_owns_transport_stop(
        &self,
        embedded_session_id: u32,
    ) -> bool {
        self.state == RecordingLifecycleState::Closed
            && self.embedded_session_id == Some(embedded_session_id)
            && self.coordinator_session_id.is_none()
    }

    /// Record a device-key start without manufacturing a second candidate
    /// state machine. Returns true when the caller should send ACTIVATE for an
    /// already-live candidate; otherwise the intent remains bound to the next
    /// candidate admitted by `begin_candidate`.
    pub(crate) fn note_device_key_takeover_intent(&mut self) -> bool {
        match self.state {
            RecordingLifecycleState::WakeCandidate => {
                self.device_key_takeover_pending = true;
                true
            }
            RecordingLifecycleState::Idle | RecordingLifecycleState::Closed => {
                self.device_key_takeover_pending = true;
                false
            }
            // A Start interpretation cannot be inherited from a session which
            // is already the active/stopping owner.  Retaining it here would
            // silently promote the next unrelated wake candidate.
            RecordingLifecycleState::Active | RecordingLifecycleState::Stopping => false,
        }
    }

    /// Commit the device control acknowledgement to this exact candidate.
    pub(crate) fn request_candidate_promotion(&mut self) -> bool {
        if self.state != RecordingLifecycleState::WakeCandidate {
            return false;
        }
        self.candidate_promotion_requested = true;
        self.device_key_takeover_pending = false;
        true
    }

    pub(crate) fn take_candidate_promotion(&mut self, embedded_session_id: u32) -> bool {
        if self.state != RecordingLifecycleState::WakeCandidate
            || self.embedded_session_id != Some(embedded_session_id)
            || !self.candidate_promotion_requested
        {
            return false;
        }
        self.candidate_promotion_requested = false;
        true
    }

    pub(crate) fn clear_device_key_takeover_intent(&mut self) {
        self.device_key_takeover_pending = false;
    }

    /// Close only the exact hidden candidate owned by the caller.  Retaining
    /// its identity in Closed makes delayed results harmless tombstones.
    pub(crate) fn close_candidate(&mut self, embedded_session_id: u32) -> bool {
        if self.state != RecordingLifecycleState::WakeCandidate
            || self.embedded_session_id != Some(embedded_session_id)
        {
            return false;
        }
        self.state = RecordingLifecycleState::Closed;
        self.coordinator_session_id = None;
        self.device_key_takeover_pending = false;
        self.candidate_promotion_requested = false;
        true
    }

    /// Returns true only for the first active/pending -> Stopping
    /// transition. Repeated stop requests are harmless and return false.
    pub(crate) fn commit_stop(&mut self, coordinator_session_id: SessionId) -> bool {
        self.commit_stop_owned(coordinator_session_id, None)
    }

    pub(crate) fn commit_stop_owned(
        &mut self,
        coordinator_session_id: SessionId,
        attempt_id: Option<u64>,
    ) -> bool {
        if self.coordinator_session_id != Some(coordinator_session_id) {
            return false;
        }
        match self.state {
            RecordingLifecycleState::Active => {
                self.state = RecordingLifecycleState::Stopping;
                self.stop_owner_attempt_id = attempt_id;
                true
            }
            RecordingLifecycleState::Stopping | RecordingLifecycleState::Closed => false,
            _ => false,
        }
    }

    pub(crate) fn reopen_after_failed_stop(&mut self, coordinator_session_id: SessionId) -> bool {
        self.reopen_after_failed_stop_owned(coordinator_session_id, None)
    }

    pub(crate) fn reopen_after_failed_stop_owned(
        &mut self,
        coordinator_session_id: SessionId,
        attempt_id: Option<u64>,
    ) -> bool {
        if self.coordinator_session_id != Some(coordinator_session_id)
            || self.state != RecordingLifecycleState::Stopping
            || self.stop_owner_attempt_id != attempt_id
        {
            return false;
        }
        self.state = RecordingLifecycleState::Active;
        self.stop_owner_attempt_id = None;
        true
    }

    pub(crate) fn close_owner(&mut self, coordinator_session_id: SessionId) -> bool {
        if self.coordinator_session_id != Some(coordinator_session_id) {
            return false;
        }
        if matches!(
            self.state,
            RecordingLifecycleState::Idle | RecordingLifecycleState::Closed
        ) {
            return false;
        }
        self.state = RecordingLifecycleState::Closed;
        self.stop_owner_attempt_id = None;
        self.device_key_takeover_pending = false;
        self.candidate_promotion_requested = false;
        true
    }
}

impl OwnerEndpointController {
    pub(crate) fn state(&self) -> OwnerEndpointState {
        if self.owner_analysis_deadline.is_some() {
            return OwnerEndpointState::OwnerEvidencePending;
        }
        match self.phase {
            EndpointPhase::Listening => OwnerEndpointState::OwnerActive,
            EndpointPhase::CandidateEnd { .. } | EndpointPhase::CatchingUp { .. } => {
                OwnerEndpointState::QuietPending
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.phase = EndpointPhase::Listening;
        self.text_revision = 0;
        self.owner_analysis_deadline = None;
    }

    /// Register the lifecycle of the single-flight owner classifier.  This is
    /// a stop barrier, never owner activity: it cannot renew the owner
    /// watermark or the normal inactivity timer.  Repeated pending
    /// observations retain the original deadline so a wedged/noisy analysis
    /// chain cannot hold recording forever.
    pub(crate) fn note_owner_analysis_pending(
        &mut self,
        pending: bool,
        now: Instant,
        maximum_wait: Duration,
    ) {
        if pending {
            self.owner_analysis_deadline
                .get_or_insert(now + maximum_wait);
        } else {
            self.owner_analysis_deadline = None;
        }
    }

    pub(crate) fn note_text_revision(&mut self) {
        self.text_revision = self.text_revision.saturating_add(1);
        // Keep the current endpoint phase alive.  The caller compares the
        // captured revision when the timer expires and will re-arm the
        // candidate with the latest evidence.  Resetting to Listening here
        // loses an already armed endpoint without scheduling a replacement;
        // the watchdog then observes a permanent Hold and auto-stop never
        // happens after an otherwise harmless preview revision.
    }

    pub(crate) fn arm(&mut self, evidence: EndpointEvidence) {
        self.phase = EndpointPhase::CandidateEnd {
            owner_watermark_ms: evidence.owner_watermark_ms,
            text_revision: self.text_revision,
        };
    }

    pub(crate) fn reopen_after_failed_stop(&mut self, evidence: EndpointEvidence) {
        self.phase = EndpointPhase::CandidateEnd {
            owner_watermark_ms: evidence.owner_watermark_ms,
            text_revision: self.text_revision,
        };
    }

    fn needs_provider_catch_up(evidence: EndpointEvidence) -> bool {
        // Explicit local identity wins over a provider row that may have
        // collapsed two people into one speaker. Neither provisional text nor
        // provider lag from that other person may extend the owner's session.
        if evidence.latest_speech_confirmed_non_target {
            return false;
        }
        // Provider coverage can already exceed the previous owner boundary
        // while a newer local tail still awaits attribution. Comparing only
        // those two old clocks stopped before the late text callback arrived.
        evidence.unresolved_owner_tail
    }

    pub(crate) fn decide_stop(
        &mut self,
        evidence: EndpointEvidence,
        now: Instant,
        catch_up_grace: Duration,
    ) -> EndpointDecision {
        if self
            .owner_analysis_deadline
            .is_some_and(|deadline| now < deadline)
        {
            return EndpointDecision::AwaitingOwnerAnalysis;
        }
        if self.owner_analysis_deadline.is_some() {
            self.owner_analysis_deadline = None;
        }
        // Provisional provider text is only a stop barrier while the local
        // capture still shows a recent owner-compatible tail.  Under strong
        // interference the provider can leave this bit set forever even after
        // the owner has gone quiet; treating it as an unconditional hold made
        // automatic endpointing hang indefinitely.  The local tail evidence
        // is the bounded safety condition that still protects mid-sentence
        // owner speech.
        if evidence.pending_provider_text
            && !evidence.latest_speech_confirmed_non_target
            && evidence.unresolved_owner_tail
        {
            return EndpointDecision::Hold;
        }
        let needs_catch_up = Self::needs_provider_catch_up(evidence);
        match self.phase {
            EndpointPhase::Listening => EndpointDecision::Hold,
            EndpointPhase::CandidateEnd {
                owner_watermark_ms,
                text_revision,
            } => {
                let text_revision_changed = text_revision != self.text_revision;
                // A preview revision is only an endpoint barrier while the
                // provider/local clocks still show an owner tail. A provider
                // can keep `pending_provider_text` set while room noise or a
                // second speaker produces late revisions; that flag alone is
                // not owner activity and must not re-arm the endpoint.
                let revision_needs_owner_tail = text_revision_changed
                    && !evidence.latest_speech_confirmed_non_target
                    && evidence.unresolved_owner_tail;
                if owner_watermark_ms != evidence.owner_watermark_ms || revision_needs_owner_tail {
                    self.arm(evidence);
                    return EndpointDecision::Hold;
                }
                // A confirmed non-target update is unrelated to the owner's
                // endpoint. Accept its revision without restarting the
                // candidate timer; otherwise every interference transcript
                // update can starve automatic stop indefinitely.
                let effective_text_revision = self.text_revision;
                if needs_catch_up {
                    self.phase = EndpointPhase::CatchingUp {
                        owner_watermark_ms,
                        text_revision: effective_text_revision,
                        deadline: now + catch_up_grace,
                    };
                    EndpointDecision::CatchingUp
                } else {
                    EndpointDecision::Stop
                }
            }
            EndpointPhase::CatchingUp {
                owner_watermark_ms,
                text_revision,
                deadline,
            } => {
                let text_revision_changed = text_revision != self.text_revision;
                let revision_needs_owner_tail = text_revision_changed
                    && !evidence.latest_speech_confirmed_non_target
                    && evidence.unresolved_owner_tail;
                if owner_watermark_ms != evidence.owner_watermark_ms || revision_needs_owner_tail {
                    self.arm(evidence);
                    return EndpointDecision::Hold;
                }
                if text_revision_changed {
                    self.phase = EndpointPhase::CatchingUp {
                        owner_watermark_ms,
                        text_revision: self.text_revision,
                        deadline,
                    };
                }
                if now < deadline {
                    EndpointDecision::CatchingUp
                } else {
                    EndpointDecision::Stop
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InterferenceEvidence {
    pub(crate) physical_overlap: bool,
    pub(crate) sustained_non_target: bool,
    pub(crate) degraded_owner_tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterferenceDecision {
    PreservePrimary,
    AwaitSeparatedOwner,
}

pub(crate) const fn decide_interference(evidence: InterferenceEvidence) -> InterferenceDecision {
    // Separator residual energy is deliberately sensitive and also fires on
    // fan/white-noise transients.  It may start the hidden stream early, but
    // must not make final delivery wait several seconds by itself.  Require
    // independent identity evidence before the separated result becomes an
    // authoritative, synchronous final path.
    if evidence.sustained_non_target || evidence.degraded_owner_tail {
        InterferenceDecision::AwaitSeparatedOwner
    } else {
        InterferenceDecision::PreservePrimary
    }
}

/// Decide whether a provider-certified, wake-bound owner track may rescue a
/// destructively incomplete separator result.
///
/// Physical overlap and a degraded owner tail are enough to run separation,
/// but neither is explicit evidence of another speaker.  A low-coverage
/// separated decode must not erase a complete owner track unless sustained
/// non-target identity evidence exists.
pub(crate) const fn certified_primary_rescues_separator_collapse(
    interference: InterferenceEvidence,
    primary_chars: usize,
    separated_chars: usize,
) -> bool {
    interference.physical_overlap
        && !interference.sustained_non_target
        && primary_chars > 0
        && separated_chars.saturating_mul(5) < primary_chars.saturating_mul(4)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WakeRecoveryEvidence {
    /// A cheap detector heard a partial/near wake phrase in the mixed stream.
    pub(crate) weak_phrase_hint: bool,
    /// The ordinary terminal path failed to recover a phrase, but persistent
    /// enrollment still considers the completed voice owner-compatible.
    pub(crate) terminal_owner_compatible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeRecoveryDecision {
    PreservePrimary,
    AwaitSeparatedOwner,
}

/// Decide whether the expensive enrolled-owner separator is justified for a
/// wake candidate. This is recovery selection only: the separated result must
/// still independently prove both the complete phrase and the enrolled owner.
pub(crate) const fn decide_wake_recovery(evidence: WakeRecoveryEvidence) -> WakeRecoveryDecision {
    if evidence.weak_phrase_hint || evidence.terminal_owner_compatible {
        WakeRecoveryDecision::AwaitSeparatedOwner
    } else {
        WakeRecoveryDecision::PreservePrimary
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SeparatedWakeEvidence {
    pub(crate) exact_phrase: bool,
    /// Identity was established on the undistorted source mixture.
    pub(crate) source_owner_compatible: bool,
    /// Or identity survived verification on the separated waveform.
    pub(crate) separated_owner_match: bool,
}

/// Separation may supply phrase evidence, but it may not manufacture identity.
/// Source-mixture identity is valid because neural separation can distort the
/// same speaker embedding that conditions the separator.
pub(crate) const fn separated_wake_can_activate(evidence: SeparatedWakeEvidence) -> bool {
    evidence.exact_phrase && (evidence.source_owner_compatible || evidence.separated_owner_match)
}

pub(crate) const OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED: u8 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OwnerOverlapPhraseEvidence {
    pub(crate) phrase_matched: bool,
    pub(crate) phrase_absent: bool,
    pub(crate) task_origin_bytes: usize,
    pub(crate) best_window_start: usize,
    pub(crate) best_distance: usize,
    pub(crate) transcript_chars: usize,
    pub(crate) phrase_chars: usize,
}

/// A mono overlap can erase either the beginning or the end of a short wake
/// phrase. Prefix-only heuristics therefore miss real owner speech such as the
/// installed session 1282 suffix crop. Keep this decision in the replayable
/// kernel and require start alignment, a bounded edit distance, body text, and
/// repeated independent confirmations at the caller.
pub(crate) const fn owner_overlap_degraded_phrase_evidence(
    evidence: OwnerOverlapPhraseEvidence,
) -> bool {
    let half_phrase = evidence.phrase_chars / 2;
    let retained_units = if half_phrase > 2 { half_phrase } else { 2 };
    let maximum_distance = evidence.phrase_chars.saturating_sub(retained_units);
    !evidence.phrase_matched
        && evidence.phrase_absent
        && evidence.task_origin_bytes == 0
        && evidence.best_window_start == 0
        && evidence.best_distance <= maximum_distance
        && evidence.transcript_chars >= evidence.phrase_chars.saturating_add(1)
}

pub(crate) const fn repeated_owner_overlap_wake_can_activate(
    source_owner_compatible: bool,
    confirmations: u8,
) -> bool {
    source_owner_compatible && confirmations >= OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED
}

/// A terminal capture can contain only one usable start-aligned overlap window:
/// later rolling windows may have already moved past the wake phrase.  When the
/// original (unseparated) waveform is still strongly owner-compatible, that
/// single full-buffer confirmation is safer than discarding an otherwise valid
/// wake.  This is intentionally stricter than the repeated recovery path and
/// is only called after terminal capture has supplied body text.
pub(crate) const fn terminal_owner_overlap_wake_can_activate(
    source_owner_compatible: bool,
    owner_score: f32,
    confirmations: u8,
) -> bool {
    source_owner_compatible && owner_score >= 0.40 && confirmations >= 1
}

/// Keep the looser two-unit phonetic rescue near the start of a search window.
/// Repeated overlapping ASR windows are correlated, so a two-unit neighbour
/// buried in ordinary speech cannot become wake evidence by repetition alone.
pub(crate) const fn owner_near_phrase_evidence_can_accumulate(
    phrase_absent: bool,
    best_distance: usize,
    best_window_start: usize,
    transcript_chars: usize,
    phrase_chars: usize,
) -> bool {
    phrase_absent
        && (best_distance <= 1 || (best_distance == 2 && best_window_start == 0))
        && transcript_chars > phrase_chars
}

/// Two eligible near-phrase observations plus a full enrolled-owner match can
/// release the wake. The observations may overlap and are not independent.
pub(crate) const fn repeated_owner_near_phrase_wake_can_activate(
    owner_matched: bool,
    owner_score: f32,
    confirmations: u8,
) -> bool {
    owner_matched && owner_score >= 0.42 && confirmations >= 2
}

/// 2026-09-23 干扰实机（24 判 5 过 19 吞）：重干扰混音让声纹整体失明——
/// 本人近音窗 0.04-0.30，媒体/旁人窗 0.01-0.11，两分布重叠，owner 门分不开。
/// 此路径把「近音持续确认」本身当唤醒意图：distance≤2 证据在滚动窗口
/// 累计满 RESCUE_CONFIRMATIONS 即放行，即使声纹 non-match——与冻结的
/// EnrolledNonMatch+phrase→Accept 开放策略同族（转写命中同样不查声纹），
/// 把同等待遇扩展给「持续近音」。误触面：旁人需跨 ≥4 个滚动窗持续发出
/// distance≤2 近音；自然语音落 3+（2026-09-20 0/43 复盘：34/43 正确
/// 拒识全在远处）。2026-09-23 15:37 实战首开即误报（剧集音频同窗滚动重听
/// 4 秒累计 5 票,确认并非独立证据;声纹 0.133 落在媒体带与本人带之间的灰区）
/// ——装机默认关闭,开启 LISTENER_ENABLE_SUSTAINED_NEAR_PHRASE_RESCUE=1;
/// 重设计须加"不同 window_start 独立确认 + owner 地板 0.15"并先过离线矩阵。
pub(crate) const OWNER_NEAR_PHRASE_RESCUE_CONFIRMATIONS: u8 = 4;

pub(crate) const fn sustained_near_phrase_wake_can_activate(confirmations: u8) -> bool {
    confirmations >= OWNER_NEAR_PHRASE_RESCUE_CONFIRMATIONS
}

pub(crate) const OPEN_NEAR_PHRASE_MAX_DISTANCE: usize = 2;
/// Same non-owner floor as the open-near tier: media-only windows read
/// 0.01-0.11 against the enrolled bank, the owner (even mixed/drifted) 0.2+.
/// This floor is what keeps the extraction recovery from burning CPU on
/// ambient media windows.
pub(crate) const MASKED_OWNER_PHRASE_MIN_VOICEPRINT_SCORE: f32 = 0.20;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct MaskedOwnerPhraseRecoveryEvidence {
    pub(crate) bank_enrolled: bool,
    /// Verification score of the (masked) wake audio against the bank.
    /// None = verifier unavailable; recovery fails closed.
    pub(crate) voiceprint_score: Option<f32>,
    pub(crate) best_distance: usize,
    pub(crate) transcript_chars: usize,
}

/// 2026-09-20 18:08 (session 2274297707): with interference playing, the wake
/// window opened on the media and the owner's phrase was fully masked — local
/// ASR transcribed unrelated garbage ("儿童衣服") at distance 4 and the KWS
/// model missed too, so the extraction recovery (built for exactly this
/// contamination) never started: it required phrase or owner evidence that
/// the masking had destroyed. Start the bounded extraction when the terminal
/// transcript heard real speech with no phrase shape and the wake audio still
/// clears the non-owner floor. Acceptance stays fully gated on the extracted
/// audio independently passing phrase + enrolled verification; starting the
/// recovery only spends CPU.
pub(crate) const fn masked_owner_phrase_recovery_should_start(
    evidence: MaskedOwnerPhraseRecoveryEvidence,
) -> bool {
    evidence.bank_enrolled
        && evidence.best_distance >= 3
        && evidence.transcript_chars >= 4
        && match evidence.voiceprint_score {
            Some(score) => score >= MASKED_OWNER_PHRASE_MIN_VOICEPRINT_SCORE,
            None => false,
        }
}
/// Same non-owner floor the local-phrase owner-gate recovery trusts
/// (OWNER_LOCAL_PHRASE_FALLBACK_MIN_SCORE). Media-only windows measure
/// 0.01-0.11 against the re-enrolled bank while the drifted owner reads
/// 0.2-0.44, so this boundary is the bystander guard for near-phrase wakes.
pub(crate) const OPEN_NEAR_PHRASE_MIN_VOICEPRINT_SCORE: f32 = 0.20;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct OpenNearPhraseWakeEvidence {
    /// Verification score of the wake audio against the enrolled bank.
    /// None means the verifier was unavailable; the tier fails closed.
    pub(crate) voiceprint_score: Option<f32>,
    pub(crate) task_origin_bytes: usize,
    pub(crate) best_window_start: usize,
    pub(crate) best_distance: usize,
    /// A two-unit neighbour needs at least the opening wake syllable. A
    /// head-aligned unrelated short sentence can otherwise pass the loose
    /// voiceprint floor at terminal (installed 2026-09-23 22:19).
    pub(crate) prefix_units: usize,
    pub(crate) transcript_chars: usize,
    pub(crate) phrase_chars: usize,
    /// Terminal (final) evaluation of the candidate window. Only then may a
    /// phrase-only transcript activate: mid-window, absence of body text is
    /// not decisive because the body may still arrive inside this window.
    pub(crate) terminal_window: bool,
}

/// 2026-09-20 08:37:58 (session 2274297156): the owner's accented "开始录音"
/// was transcribed "开su音…" — head-aligned, phonetic distance 2, body text
/// after it — and was rejected because every near-phrase tier required the
/// enrolled match the drifted bank could not produce, forcing the user to
/// repeat ("wake too slow"). Open-acceptance wakes already activate on an
/// exact phrase with no voiceprint floor at all, so this tier is strictly
/// stricter than the existing open path: head-aligned at the candidate origin,
/// bounded phonetic distance, body text, and a voiceprint floor at the
/// non-owner boundary. Media near-misses of the same distance ("开始上课")
/// read 0.01-0.11 and stay out.
///
/// 2026-09-21 10:16 (sessions 2274299279/2274299280): the bare-phrase gap.
/// The user says only the accented/clipped wake phrase and stops to wait for
/// the capsule: no body text can ever satisfy the body requirement inside
/// that window, so the terminal evaluation rejected it after the full window
/// lifetime and the capsule only appeared when the repeat's fresh window
/// accepted ("准备再说一遍的时候它弹出来了"). At the TERMINAL evaluation no
/// further audio can add body text, so a phrase-only transcript that still
/// heard almost the whole phrase (>= phrase_chars - 1 units, with the same
/// head alignment and distance bounds) may activate under the same
/// voiceprint floor. Mid-window calls keep requiring body text.
pub(crate) const fn open_near_phrase_wake_can_activate(
    evidence: OpenNearPhraseWakeEvidence,
) -> bool {
    let body_text = evidence.transcript_chars > evidence.phrase_chars;
    let phrase_only_terminal = evidence.terminal_window
        && evidence.transcript_chars + 1 >= evidence.phrase_chars;
    evidence.best_distance > 0
        && evidence.best_distance <= OPEN_NEAR_PHRASE_MAX_DISTANCE
        && (evidence.best_distance < 2 || evidence.prefix_units >= 1)
        && evidence.task_origin_bytes == 0
        && evidence.best_window_start == 0
        && (body_text || phrase_only_terminal)
        && match evidence.voiceprint_score {
            Some(score) => score >= OPEN_NEAR_PHRASE_MIN_VOICEPRINT_SCORE,
            None => false,
        }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LiveOwnerNearWakeEvidence {
    pub(crate) phrase_enrolled: bool,
    /// The result must be the one bounded expansion after an earlier strong
    /// prefix, never a single exploratory ASR guess.
    pub(crate) bounded_followup: bool,
    pub(crate) task_origin_bytes: usize,
    pub(crate) current_window_origin_bytes: usize,
    pub(crate) best_window_start: usize,
    pub(crate) prefix_units: usize,
    pub(crate) best_distance: usize,
    pub(crate) transcript_chars: usize,
    pub(crate) phrase_chars: usize,
}

/// A wake phrase may begin after firmware pre-roll or ambient speech, so
/// "start-aligned" is relative to the current rolling search window rather
/// than absolute candidate byte zero. Permit a live owner-gated attempt only
/// after a bounded expanding confirmation also contains body text. The normal
/// voiceprint arbitration remains mandatory at the caller.
pub(crate) const fn live_owner_near_wake_can_attempt(evidence: LiveOwnerNearWakeEvidence) -> bool {
    let retained = evidence.phrase_chars.saturating_sub(1);
    let minimum_prefix_units = if retained > 1 { retained } else { 1 };
    evidence.phrase_enrolled
        && evidence.bounded_followup
        && evidence.task_origin_bytes == evidence.current_window_origin_bytes
        && evidence.best_window_start == 0
        && evidence.prefix_units >= minimum_prefix_units
        && evidence.best_distance <= 1
        && evidence.transcript_chars > evidence.phrase_chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sustained_near_phrase_rescue_fires_only_on_persistent_confirmations() {
        // 2026-09-23 干扰实机：被吞样本 confirmations=4 ——门槛必须放它过；
        // 3 次（偶发近音）不放。owner 分不进签名：该路径就是在声纹
        // 失明时兜底的，任何 owner 条件都会把它变回死路。
        assert!(!sustained_near_phrase_wake_can_activate(0));
        assert!(!sustained_near_phrase_wake_can_activate(1));
        assert!(!sustained_near_phrase_wake_can_activate(3));
        assert!(sustained_near_phrase_wake_can_activate(4));
        assert!(sustained_near_phrase_wake_can_activate(7));
        assert_eq!(OWNER_NEAR_PHRASE_RESCUE_CONFIRMATIONS, 4);
    }

    #[test]
    fn target_speaker_endpoint_preview_reducer_rejects_stale_sessions_and_cross_source_duplicates()
    {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(first);

        assert_eq!(
            preview.observe_provisional(first, "主人正文"),
            Some("主人正文".to_string())
        );
        let authoritative = preview.observe_authoritative(first, "主人正文", false);
        assert!(authoritative.authoritative_changed);
        assert_eq!(authoritative.visible_update, None);

        preview.begin_session(second);
        assert_eq!(
            preview.observe_authoritative(first, "旧会话文字", true),
            RecordingPreviewReduction::default()
        );
        assert_eq!(preview.authoritative(second), None);
        assert_eq!(preview.visible(second), None);
    }

    #[test]
    fn target_speaker_endpoint_preview_reducer_final_supplement_settles_provisional_tail() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);

        let initial = preview.observe_authoritative(session_id, "主人第一句", false);
        assert_eq!(initial.visible_update.as_deref(), Some("主人第一句"));
        assert_eq!(
            preview.observe_provisional(session_id, "主人第一句旁人尾巴"),
            Some("主人第一句旁人尾巴".to_string())
        );

        let settled = preview.observe_authoritative(session_id, "主人第一句", true);
        assert!(!settled.authoritative_changed);
        assert_eq!(settled.visible_update, None);
        assert_eq!(
            preview.authoritative(session_id).as_deref(),
            Some("主人第一句")
        );
        assert_eq!(
            preview.visible(session_id).as_deref(),
            Some("主人第一句旁人尾巴"),
            "dictation must not retract a longer visible preview when owner-filtered text is shorter"
        );
    }

    #[test]
    fn empty_or_filtered_authoritative_update_cannot_restore_stale_preview() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "主人完整正文", true);
        preview.observe_provisional(session_id, "主人完整正文旁人尾巴");

        let cleared = preview.observe_authoritative(session_id, "", true);
        assert!(cleared.authoritative_changed);
        assert_eq!(preview.authoritative(session_id), None);
        assert_eq!(
            preview.visible(session_id).as_deref(),
            Some("主人完整正文旁人尾巴"),
            "the visual high-water mark may remain, but it is no longer final evidence"
        );

        assert!(!preview.invalidate_authoritative(session_id));
        assert_eq!(preview.authoritative(session_id), None);
    }

    #[test]
    fn explicit_preview_invalidation_rejects_late_callbacks_but_empty_terminal_does_not() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "主人前句", true);

        // An ordinary empty provider terminal is a missing revision, not an
        // ownership revocation. A later accepted revision may still arrive
        // while the coordinator keeps this session open.
        let cleared = preview.observe_authoritative(session_id, "", true);
        assert!(cleared.authoritative_changed);
        let resumed = preview.observe_authoritative(session_id, "主人后句", true);
        assert!(resumed.authoritative_changed);
        assert_eq!(
            preview.authoritative(session_id).as_deref(),
            Some("主人后句")
        );

        assert!(preview.invalidate_authoritative(session_id));
        assert_eq!(preview.authoritative(session_id), None);
        assert_eq!(
            preview.observe_authoritative(session_id, "旧 session 迟到正文", true),
            RecordingPreviewReduction::default()
        );
        assert_eq!(
            preview.observe_provisional(session_id, "旧 session 迟到旁路"),
            None
        );
        assert_eq!(preview.authoritative(session_id), None);
    }

    #[test]
    fn cleared_preview_does_not_admit_a_late_callback_as_a_new_session() {
        let old_session = uuid::Uuid::new_v4();
        let current_session = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(old_session);
        preview.observe_authoritative(old_session, "旧正文", true);
        assert!(preview.clear_session(old_session));

        assert_eq!(
            preview.observe_authoritative(old_session, "迟到旧正文", true),
            RecordingPreviewReduction::default()
        );
        assert_eq!(preview.authoritative(old_session), None);

        preview.begin_session(current_session);
        assert!(
            preview
                .observe_authoritative(current_session, "新 session 正文", true)
                .authoritative_changed
        );
        assert_eq!(
            preview.observe_authoritative(old_session, "再次迟到旧正文", true),
            RecordingPreviewReduction::default()
        );
        assert_eq!(
            preview.authoritative(current_session).as_deref(),
            Some("新 session 正文")
        );
    }

    #[test]
    fn punctuation_only_visible_revision_rearms_growth_clock() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "今天开会", false);
        let before = preview
            .last_visible_growth_at(session_id)
            .expect("body text arms the hang clock");
        std::thread::sleep(std::time::Duration::from_millis(5));
        preview.observe_provisional(session_id, "今天开会。");
        let after = preview
            .last_visible_growth_at(session_id)
            .expect("punctuation still arms the hang clock");
        assert!(
            after > before,
            "adding 。 must rearm the visible hang clock so a 1s body endpoint cannot fire from the previous CJK char"
        );
    }

    #[test]
    fn dictation_preview_keeps_high_water_when_filter_returns_shorter_owner_text() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        assert_eq!(
            preview
                .observe_authoritative(session_id, "今天天气很好我们去公园", false)
                .visible_update
                .as_deref(),
            Some("今天天气很好我们去公园")
        );
        let shrunk = preview.observe_authoritative(session_id, "今天天气很好", true);
        assert!(shrunk.authoritative_changed);
        assert_eq!(shrunk.visible_update, None);
        assert_eq!(
            preview.visible(session_id).as_deref(),
            Some("今天天气很好我们去公园")
        );
    }

    #[test]
    fn target_speaker_endpoint_preview_reducer_never_shrinks_to_shorter_provisional_text() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "主人完整正文", false);

        assert_eq!(preview.observe_provisional(session_id, "主人"), None);
        assert_eq!(preview.visible(session_id).as_deref(), Some("主人完整正文"));
    }

    #[test]
    fn provisional_preview_cannot_retract_a_longer_provisional_update() {
        let session_id = uuid::Uuid::new_v4();
        let mut preview = RecordingPreviewController::default();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "今天", false);
        preview.observe_provisional(session_id, "今天讨论发布以及所有已知问题");
        let growth = preview.last_visible_growth_at(session_id);
        assert_eq!(
            preview.observe_provisional(session_id, "今天讨论发布"),
            None
        );
        assert_eq!(
            preview.visible(session_id).as_deref(),
            Some("今天讨论发布以及所有已知问题")
        );
        assert_eq!(preview.last_visible_growth_at(session_id), growth);
        assert!(preview
            .observe_provisional(session_id, "今天讨论发布以及所有已知问题的修复")
            .is_some());
    }

    #[test]
    fn target_speaker_endpoint_recording_lifecycle_serializes_candidate_owner_stop_and_close() {
        let coordinator_id = uuid::Uuid::new_v4();
        let mut lifecycle = RecordingLifecycleController::default();
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Idle);
        assert!(!lifecycle.promote_candidate_to_owner(7, coordinator_id));
        assert!(lifecycle.begin_candidate(7));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::WakeCandidate);
        assert!(!lifecycle.begin_manual_owner(7, coordinator_id));
        assert!(lifecycle.promote_candidate_to_owner(7, coordinator_id));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
        assert!(lifecycle.commit_stop(coordinator_id));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Stopping);
        assert!(!lifecycle.commit_stop(coordinator_id));
        assert!(lifecycle.close_owner(coordinator_id));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Closed);
    }

    #[test]
    fn target_speaker_endpoint_owned_stop_cannot_reopen_another_stop_attempt() {
        let coordinator_id = uuid::Uuid::new_v4();
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(lifecycle.begin_manual_owner(9, coordinator_id));
        assert!(lifecycle.commit_stop_owned(coordinator_id, Some(42)));
        assert!(!lifecycle.reopen_after_failed_stop(coordinator_id));
        assert!(lifecycle.reopen_after_failed_stop_owned(coordinator_id, Some(42)));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
    }

    #[test]
    fn target_speaker_endpoint_recording_lifecycle_rejects_stale_sessions_and_reopens_only_failed_stop(
    ) {
        let current = uuid::Uuid::new_v4();
        let stale = uuid::Uuid::new_v4();
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(lifecycle.begin_manual_owner(11, current));
        assert!(!lifecycle.commit_stop(stale));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
        assert!(lifecycle.commit_stop(current));
        assert!(lifecycle.reopen_after_failed_stop(current));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
        assert!(!lifecycle.reopen_after_failed_stop(current));
    }

    #[test]
    fn target_speaker_endpoint_recording_lifecycle_keeps_closed_identity_as_stale_callback_tombstone(
    ) {
        let current = uuid::Uuid::new_v4();
        let stale = uuid::Uuid::new_v4();
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(lifecycle.begin_manual_owner(21, current));
        assert!(lifecycle.close_owner(current));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Closed);
        assert!(!lifecycle.commit_stop(current));
        assert!(!lifecycle.commit_stop(stale));
        assert!(lifecycle.begin_candidate(22));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::WakeCandidate);
    }

    #[test]
    fn target_speaker_endpoint_recording_lifecycle_binds_early_device_takeover_to_one_candidate() {
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(!lifecycle.note_device_key_takeover_intent());
        assert!(lifecycle.begin_candidate(31));
        assert!(!lifecycle.hidden_candidate_active());
        assert!(lifecycle.take_candidate_promotion(31));
        assert!(!lifecycle.take_candidate_promotion(31));
        assert!(!lifecycle.take_candidate_promotion(32));
    }

    #[test]
    fn target_speaker_endpoint_active_owner_cannot_leak_takeover_into_next_candidate() {
        let mut lifecycle = RecordingLifecycleController::default();
        let owner = uuid::Uuid::new_v4();
        assert!(lifecycle.begin_manual_owner(35, owner));
        assert!(!lifecycle.note_device_key_takeover_intent());
        assert!(lifecycle.close_owner(owner));
        assert!(lifecycle.begin_candidate(36));
        assert!(lifecycle.hidden_candidate_active());
        assert!(!lifecycle.take_candidate_promotion(36));
    }

    #[test]
    fn target_speaker_endpoint_recording_lifecycle_candidate_close_is_identity_scoped() {
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(lifecycle.begin_candidate(41));
        assert!(!lifecycle.close_candidate(40));
        assert_eq!(lifecycle.current_candidate_session_id(), Some(41));
        assert!(lifecycle.close_candidate(41));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Closed);
        assert!(!lifecycle.close_candidate(41));
        assert!(lifecycle.begin_candidate(42));
        assert!(!lifecycle.close_candidate(41));
        assert_eq!(lifecycle.current_candidate_session_id(), Some(42));
    }

    #[test]
    fn target_speaker_endpoint_rejected_candidate_stop_cannot_cut_a_new_owner() {
        let mut lifecycle = RecordingLifecycleController::default();
        assert!(lifecycle.begin_candidate(51));
        assert!(lifecycle.close_candidate(51));
        assert!(lifecycle.rejected_candidate_still_owns_transport_stop(51));
        assert!(!lifecycle.rejected_candidate_still_owns_transport_stop(50));

        let owner = uuid::Uuid::new_v4();
        assert!(lifecycle.begin_manual_owner(52, owner));
        assert!(!lifecycle.rejected_candidate_still_owns_transport_stop(51));
        assert!(!lifecycle.rejected_candidate_still_owns_transport_stop(52));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
    }

    #[test]
    fn actor_idle_can_release_a_stale_active_lock() {
        let mut lifecycle = RecordingLifecycleController::default();
        let owner = uuid::Uuid::new_v4();
        assert!(lifecycle.begin_candidate(845));
        assert!(lifecycle.promote_candidate_to_owner(845, owner));
        assert_eq!(lifecycle.state(), RecordingLifecycleState::Active);
        lifecycle.force_close_stale_lock();
        assert!(lifecycle.begin_candidate(846));
        assert_eq!(lifecycle.current_candidate_session_id(), Some(846));
    }

    #[test]
    fn unenrolled_phrase_can_wake_without_claiming_owner_verification() {
        let result = arbitrate_wake(
            PhraseSignal::LocalTranscript,
            OwnerAccessEvidence::OpenUnenrolled,
            false,
        );
        assert_eq!(result.decision, GateDecision::Accept);
        assert!(!result.owner_access.enrolled_owner_verified());
    }

    #[test]
    fn phrase_hit_accepts_even_when_first_voiceprint_misses() {
        let result = arbitrate_wake(
            PhraseSignal::LocalTranscript,
            OwnerAccessEvidence::EnrolledNonMatch,
            false,
        );
        assert_eq!(result.decision, GateDecision::Accept);
        assert!(!result.owner_access.enrolled_owner_verified());
        assert_eq!(
            arbitrate_wake(
                PhraseSignal::None,
                OwnerAccessEvidence::EnrolledNonMatch,
                true,
            )
            .decision,
            GateDecision::Reject
        );
    }

    #[test]
    fn enrolled_match_is_the_only_verified_owner_accept() {
        let matched = arbitrate_wake(
            PhraseSignal::KeywordModel,
            OwnerAccessEvidence::EnrolledMatch,
            false,
        );
        assert_eq!(matched.decision, GateDecision::Accept);
        assert!(matched.owner_access.enrolled_owner_verified());

        let open = arbitrate_wake(
            PhraseSignal::KeywordModel,
            OwnerAccessEvidence::OpenInactiveProfile,
            false,
        );
        assert_eq!(open.decision, GateDecision::Accept);
        assert!(!open.owner_access.enrolled_owner_verified());
    }

    #[test]
    fn profile_failure_never_degrades_to_open_gate() {
        assert_eq!(
            arbitrate_wake(
                PhraseSignal::LocalTranscript,
                OwnerAccessEvidence::Unavailable,
                false,
            )
            .decision,
            GateDecision::Pending
        );
        assert_eq!(
            arbitrate_wake(
                PhraseSignal::LocalTranscript,
                OwnerAccessEvidence::Unavailable,
                true,
            )
            .decision,
            GateDecision::Reject
        );
    }

    #[test]
    fn owner_evidence_alone_never_wakes() {
        assert_eq!(
            arbitrate_wake(
                PhraseSignal::None,
                OwnerAccessEvidence::EnrolledMatch,
                false,
            )
            .decision,
            GateDecision::Pending
        );
        assert_eq!(
            arbitrate_wake(PhraseSignal::None, OwnerAccessEvidence::EnrolledMatch, true,).decision,
            GateDecision::Reject,
            "speaker identity cannot authorize dictation without activation phrase evidence"
        );
    }

    #[test]
    fn provider_text_survives_missing_diarization_and_destructive_filtering() {
        // Redacted replay of installed session aa460e04: the provider reached
        // 49 chars while the ownership filter returned an empty candidate.
        let mut ledger = TranscriptEvidenceLedger::default();
        ledger.note_provider_revision("前三字", Some(2_100), false, false);
        let recognized = "甲".repeat(49);
        ledger.note_provider_revision(&recognized, Some(13_080), false, true);

        let committed = ledger
            .commit_once("", true)
            .expect("first terminal decision");
        assert_eq!(committed.source, CommitSource::ProviderRawFallback);
        assert_eq!(committed.text, recognized);
        assert_eq!(ledger.revision_count(), 2);
    }

    #[test]
    fn explicit_ownership_exclusion_blocks_raw_fallback() {
        let mut ledger = TranscriptEvidenceLedger::default();
        ledger.note_provider_revision("旁人正文", Some(4_000), true, true);
        let committed = ledger
            .commit_once("", false)
            .expect("first terminal decision");
        assert_eq!(committed.source, CommitSource::Empty);
        assert!(committed.text.is_empty());
    }

    #[test]
    fn transcript_delivery_is_exactly_once() {
        let mut ledger = TranscriptEvidenceLedger::default();
        ledger.note_provider_revision("主人正文", Some(3_000), false, true);
        assert!(ledger.commit_once("主人正文", true).is_some());
        assert!(ledger.commit_once("重复主人正文", true).is_none());
    }

    #[test]
    fn endpoint_waits_for_late_provider_revision_then_rearms() {
        // Redacted replay of f9472275: owner boundary stopped at 8650 ms,
        // owner-compatible speech reached 9500 ms, and provider text grew
        // 181 ms after the old endpoint had already stopped.
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(8_650),
            provider_coverage_ms: Some(9_000),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::CatchingUp
        );
        endpoint.note_text_revision();
        endpoint.arm(EndpointEvidence {
            provider_coverage_ms: Some(9_500),
            ..evidence
        });
        assert_eq!(
            endpoint.decide_stop(
                EndpointEvidence {
                    owner_watermark_ms: Some(9_500),
                    provider_coverage_ms: Some(9_500),
                    ..evidence
                },
                started + Duration::from_millis(181),
                Duration::from_millis(300),
            ),
            EndpointDecision::Hold
        );
    }

    #[test]
    fn target_speaker_endpoint_waits_for_in_flight_owner_analysis_without_renewing_owner_clock() {
        // Installed 2026-09-04 session: the public endpoint expired while a
        // 5.2 s local voiceprint window was already being classified.  The
        // result arrived only after stop had been committed.  Analysis is a
        // bounded barrier, not activity, so its deadline never moves when the
        // same single-flight chain is observed again.
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(3_122),
            provider_coverage_ms: Some(5_000),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        endpoint.note_owner_analysis_pending(true, started, Duration::from_millis(3_000));
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(1_000),
                Duration::from_millis(300),
            ),
            EndpointDecision::AwaitingOwnerAnalysis,
        );

        endpoint.note_owner_analysis_pending(
            true,
            started + Duration::from_millis(2_000),
            Duration::from_millis(3_000),
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(2_999),
                Duration::from_millis(300),
            ),
            EndpointDecision::AwaitingOwnerAnalysis,
            "repeated pending observations must not move the original bound",
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(3_000),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop,
            "a wedged analysis chain must not disable auto-end",
        );
    }

    #[test]
    fn target_speaker_endpoint_completed_owner_analysis_releases_barrier_immediately() {
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(3_122),
            provider_coverage_ms: Some(5_000),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        endpoint.note_owner_analysis_pending(true, started, Duration::from_millis(3_000));
        endpoint.note_owner_analysis_pending(
            false,
            started + Duration::from_millis(1_100),
            Duration::from_millis(3_000),
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(1_100),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop,
        );
    }

    #[test]
    fn preview_revision_does_not_permanently_disable_armed_endpoint() {
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_000),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        endpoint.note_text_revision();

        // A settled preview revision is not an owner-tail barrier. It must
        // commit immediately once the normal endpoint deadline is reached.
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(1),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop,
        );
    }

    #[test]
    fn settled_provider_revision_cannot_hold_quiet_enrolled_owner() {
        // Replay of the current installed failure: provider settled the final
        // row at 10.8s, local owner watermark was quiet at 7.6s, but preview
        // revisions kept the arbiter in Hold until the user clicked Stop.
        let started = Instant::now();
        let settled = EndpointEvidence {
            owner_watermark_ms: Some(10_052),
            provider_coverage_ms: Some(10_700),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(settled);
        endpoint.note_text_revision();
        assert_eq!(
            endpoint.decide_stop(
                settled,
                started + Duration::from_millis(900),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn installed_long_owner_tail_reaches_stop_after_preview_revision() {
        // Replay of the latest failed session f1b13e0f: the provider stopped
        // at 10.8 s while local speech reached 10.9 s, with a stable owner
        // boundary at 9.692 s. A preview revision must not strand this case
        // in Listening; it gets one bounded provider catch-up window.
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(9_692),
            provider_coverage_ms: Some(10_800),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        endpoint.note_text_revision();

        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::Hold
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(1),
                Duration::from_millis(300),
            ),
            EndpointDecision::CatchingUp
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(301),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn quiet_owner_endpoint_keeps_normal_latency() {
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(5_000),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, Instant::now(), Duration::from_millis(300)),
            EndpointDecision::Stop
        );
        assert_eq!(endpoint.state(), OwnerEndpointState::QuietPending);
        endpoint.reopen_after_failed_stop(evidence);
        assert_eq!(endpoint.state(), OwnerEndpointState::QuietPending);
    }

    #[test]
    fn pending_interference_does_not_hold_quiet_owner_forever() {
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_000),
            pending_provider_text: true,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, Instant::now(), Duration::from_millis(300)),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn pending_preview_revision_does_not_rearm_quiet_owner() {
        // Provider pending text may remain latched while late room-speech
        // revisions arrive. Once the owner tail is resolved, those revisions
        // must not turn an already armed endpoint back into Hold.
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_000),
            pending_provider_text: true,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: false,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        endpoint.note_text_revision();
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(900),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn pending_owner_tail_still_gets_catch_up_protection() {
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_200),
            pending_provider_text: true,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        assert_eq!(endpoint.state(), OwnerEndpointState::OwnerActive);
        endpoint.arm(evidence);
        assert_eq!(endpoint.state(), OwnerEndpointState::QuietPending);
        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::Hold
        );
    }

    #[test]
    fn unresolved_tail_is_bounded_when_provider_never_advances() {
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_200),
            pending_provider_text: false,
            latest_speech_confirmed_non_target: false,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::CatchingUp
        );
        assert_eq!(
            endpoint.decide_stop(
                evidence,
                started + Duration::from_millis(300),
                Duration::from_millis(300),
            ),
            EndpointDecision::Stop
        );
        assert_eq!(endpoint.state(), OwnerEndpointState::QuietPending);
    }

    #[test]
    fn confirmed_other_speaker_never_extends_owner_catch_up() {
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_100),
            pending_provider_text: true,
            latest_speech_confirmed_non_target: true,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, Instant::now(), Duration::from_millis(300)),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn confirmed_other_speaker_revision_does_not_starve_endpoint() {
        let started = Instant::now();
        let evidence = EndpointEvidence {
            owner_watermark_ms: Some(4_000),
            provider_coverage_ms: Some(4_100),
            pending_provider_text: true,
            latest_speech_confirmed_non_target: true,
            unresolved_owner_tail: true,
        };
        let mut endpoint = OwnerEndpointController::default();
        endpoint.arm(evidence);
        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::Stop
        );

        // A preview/provider update increments the shared text revision after
        // the candidate was armed.  It must not reopen the endpoint when the
        // newest speech is explicitly classified as another speaker.
        endpoint.reset();
        endpoint.arm(evidence);
        endpoint.note_text_revision();
        assert_eq!(
            endpoint.decide_stop(evidence, started, Duration::from_millis(300)),
            EndpointDecision::Stop
        );
    }

    #[test]
    fn firmware_lease_bridges_real_owner_provider_lag() {
        // LST-REC-054: local owner-compatible speech reached 5300 ms while
        // provider coverage was still 4350 ms.  Firmware stopped before the
        // provider added four more body characters 155 ms later.
        let evidence = FirmwareEndpointLeaseEvidence {
            visible_body: true,
            owner_established: true,
            owner_speech_watermark_ms: Some(5_300),
            provider_coverage_ms: Some(4_350),
            unresolved_owner_tail: true,
            latest_speech_confirmed_non_target: false,
        };
        assert_eq!(
            decide_firmware_endpoint_lease(evidence),
            FirmwareEndpointLeaseDecision::Renew
        );
        assert_eq!(
            decide_firmware_endpoint_lease(FirmwareEndpointLeaseEvidence {
                provider_coverage_ms: Some(5_300),
                ..evidence
            }),
            FirmwareEndpointLeaseDecision::Idle
        );
    }

    #[test]
    fn firmware_lease_never_uses_other_speaker_or_ownerless_noise() {
        let owner_tail = FirmwareEndpointLeaseEvidence {
            visible_body: true,
            owner_established: true,
            owner_speech_watermark_ms: Some(5_300),
            provider_coverage_ms: Some(4_350),
            unresolved_owner_tail: true,
            latest_speech_confirmed_non_target: false,
        };
        for evidence in [
            FirmwareEndpointLeaseEvidence {
                latest_speech_confirmed_non_target: true,
                ..owner_tail
            },
            FirmwareEndpointLeaseEvidence {
                owner_established: false,
                ..owner_tail
            },
            FirmwareEndpointLeaseEvidence {
                unresolved_owner_tail: false,
                ..owner_tail
            },
            FirmwareEndpointLeaseEvidence {
                visible_body: false,
                ..owner_tail
            },
            // Generic VAD/energy has no field in this contract. An update
            // carrying only room activity must therefore remain idle.
            FirmwareEndpointLeaseEvidence {
                owner_speech_watermark_ms: None,
                unresolved_owner_tail: false,
                ..owner_tail
            },
        ] {
            assert_eq!(
                decide_firmware_endpoint_lease(evidence),
                FirmwareEndpointLeaseDecision::Idle
            );
        }
    }

    #[test]
    fn clean_owner_always_preserves_primary_audio_path() {
        assert_eq!(
            decide_interference(InterferenceEvidence::default()),
            InterferenceDecision::PreservePrimary
        );
    }

    #[test]
    fn identity_interference_evidence_enables_separated_owner_path() {
        for evidence in [
            InterferenceEvidence {
                sustained_non_target: true,
                ..InterferenceEvidence::default()
            },
            InterferenceEvidence {
                degraded_owner_tail: true,
                ..InterferenceEvidence::default()
            },
        ] {
            assert_eq!(
                decide_interference(evidence),
                InterferenceDecision::AwaitSeparatedOwner
            );
        }
    }

    #[test]
    fn physical_residual_alone_never_blocks_certified_primary_delivery() {
        // Live C2 session bf6943ce: residual-only detection launched two
        // separator chunks (1728 + 1645 ms), then discarded its 10-char result
        // and retained the 30-char certified primary.  The same evidence must
        // now stay on the low-latency primary path.
        assert_eq!(
            decide_interference(InterferenceEvidence {
                physical_overlap: true,
                sustained_non_target: false,
                degraded_owner_tail: false,
            }),
            InterferenceDecision::PreservePrimary
        );
    }

    #[test]
    fn noise_only_separator_collapse_cannot_erase_certified_owner_text() {
        let noise_only = InterferenceEvidence {
            physical_overlap: true,
            sustained_non_target: false,
            degraded_owner_tail: false,
        };
        // Every destructive selection found in the current and rotated live
        // logs. These are independent sessions, not synthetic variations of
        // one sample; all had one wake-bound provider speaker and no explicit
        // non-target evidence.
        for (primary_chars, separated_chars) in [
            (61, 15),
            (52, 11),
            (37, 20),
            (98, 58),
            (77, 44),
            (100, 9),
            (56, 16),
        ] {
            assert!(certified_primary_rescues_separator_collapse(
                noise_only,
                primary_chars,
                separated_chars
            ));
        }
        assert!(!certified_primary_rescues_separator_collapse(
            noise_only, 100, 84
        ));
        assert!(certified_primary_rescues_separator_collapse(
            InterferenceEvidence {
                degraded_owner_tail: true,
                ..noise_only
            },
            56,
            20
        ));
    }

    #[test]
    fn explicit_other_speaker_evidence_keeps_separator_authoritative() {
        for interference in [
            InterferenceEvidence {
                physical_overlap: true,
                sustained_non_target: true,
                degraded_owner_tail: false,
            },
            InterferenceEvidence {
                physical_overlap: true,
                sustained_non_target: true,
                degraded_owner_tail: false,
            },
        ] {
            assert!(!certified_primary_rescues_separator_collapse(
                interference,
                100,
                9
            ));
        }
    }

    #[test]
    fn wake_separator_stays_off_without_phrase_or_owner_compatible_evidence() {
        assert_eq!(
            decide_wake_recovery(WakeRecoveryEvidence::default()),
            WakeRecoveryDecision::PreservePrimary
        );
    }

    #[test]
    fn wake_separator_recovers_either_partial_phrase_or_terminal_owner_evidence() {
        for evidence in [
            WakeRecoveryEvidence {
                weak_phrase_hint: true,
                terminal_owner_compatible: false,
            },
            WakeRecoveryEvidence {
                weak_phrase_hint: false,
                terminal_owner_compatible: true,
            },
        ] {
            assert_eq!(
                decide_wake_recovery(evidence),
                WakeRecoveryDecision::AwaitSeparatedOwner
            );
        }
    }

    #[test]
    fn separated_phrase_requires_independent_owner_evidence() {
        assert!(!separated_wake_can_activate(SeparatedWakeEvidence {
            exact_phrase: true,
            ..SeparatedWakeEvidence::default()
        }));
        assert!(separated_wake_can_activate(SeparatedWakeEvidence {
            exact_phrase: true,
            source_owner_compatible: true,
            separated_owner_match: false,
        }));
        assert!(separated_wake_can_activate(SeparatedWakeEvidence {
            exact_phrase: true,
            source_owner_compatible: false,
            separated_owner_match: true,
        }));
        assert!(!separated_wake_can_activate(SeparatedWakeEvidence {
            exact_phrase: false,
            source_owner_compatible: true,
            separated_owner_match: true,
        }));
    }

    #[test]
    fn installed_owner_suffix_crop_requires_three_confirmations_and_identity() {
        let session_1282 = OwnerOverlapPhraseEvidence {
            phrase_matched: false,
            phrase_absent: true,
            task_origin_bytes: 0,
            best_window_start: 0,
            best_distance: 2,
            transcript_chars: 11,
            phrase_chars: 4,
        };
        assert!(owner_overlap_degraded_phrase_evidence(session_1282));
        assert!(!repeated_owner_overlap_wake_can_activate(true, 2));
        assert!(!repeated_owner_overlap_wake_can_activate(false, 3));
        assert!(repeated_owner_overlap_wake_can_activate(true, 3));
        assert!(terminal_owner_overlap_wake_can_activate(true, 0.419, 1));
        assert!(!terminal_owner_overlap_wake_can_activate(true, 0.399, 1));
        assert!(!terminal_owner_overlap_wake_can_activate(false, 0.9, 1));
        assert!(repeated_owner_near_phrase_wake_can_activate(true, 0.502, 2));
        assert!(!repeated_owner_near_phrase_wake_can_activate(
            true, 0.419, 2
        ));
        assert!(!repeated_owner_near_phrase_wake_can_activate(true, 0.8, 1));
    }

    #[test]
    fn repeated_owner_near_phrase_excludes_buried_two_unit_neighbour() {
        // False wake 2356876213: four overlapping windows saw distance 2 at
        // window start 4 in ordinary speech, despite a valid owner voiceprint.
        assert!(!owner_near_phrase_evidence_can_accumulate(true, 2, 4, 12, 4));
        // A two-unit error at the beginning remains eligible for suffix crop.
        assert!(owner_near_phrase_evidence_can_accumulate(true, 2, 0, 12, 4));
        // A stronger one-unit match may still follow firmware pre-roll.
        assert!(owner_near_phrase_evidence_can_accumulate(true, 1, 4, 12, 4));
        assert!(!owner_near_phrase_evidence_can_accumulate(false, 1, 0, 12, 4));
        assert!(!owner_near_phrase_evidence_can_accumulate(true, 1, 0, 4, 4));
    }

    #[test]
    fn masked_owner_phrase_recovery_starts_only_for_owner_shaped_garbage() {
        use MaskedOwnerPhraseRecoveryEvidence as Evidence;
        // Session 2274297707 (2026-09-20 18:08): interference masked the wake
        // word into unrelated garbage ("儿童衣服", distance 4) while the owner's
        // mixed voice still cleared the non-owner floor.
        let masked_owner = Evidence {
            bank_enrolled: true,
            voiceprint_score: Some(0.27),
            best_distance: 4,
            transcript_chars: 4,
        };
        assert!(masked_owner_phrase_recovery_should_start(masked_owner));
        // Media-only windows stay below the floor; no bank, no verifier, no
        // real speech, and near-miss shapes (other tiers own them) stay out.
        assert!(!masked_owner_phrase_recovery_should_start(Evidence {
            voiceprint_score: Some(0.11),
            ..masked_owner
        }));
        assert!(!masked_owner_phrase_recovery_should_start(Evidence {
            bank_enrolled: false,
            ..masked_owner
        }));
        assert!(!masked_owner_phrase_recovery_should_start(Evidence {
            voiceprint_score: None,
            ..masked_owner
        }));
        assert!(!masked_owner_phrase_recovery_should_start(Evidence {
            transcript_chars: 2,
            ..masked_owner
        }));
        assert!(!masked_owner_phrase_recovery_should_start(Evidence {
            best_distance: 2,
            ..masked_owner
        }));
    }

    #[test]
    fn open_near_phrase_tier_separates_drifted_owner_from_media_by_voiceprint_floor() {
        use OpenNearPhraseWakeEvidence as Evidence;
        // Session 2274297156 (2026-09-20 08:37): accented "开su音…" wake —
        // head-aligned, distance 2, body text, bank drifted to non-match.
        let accented_owner = Evidence {
            voiceprint_score: Some(0.27),
            task_origin_bytes: 0,
            best_window_start: 0,
            best_distance: 2,
            prefix_units: 1,
            transcript_chars: 21,
            phrase_chars: 4,
            terminal_window: true,
        };
        assert!(open_near_phrase_wake_can_activate(accented_owner));
        // Media shares the phonetic shape ("开始上课…") but reads below the
        // non-owner floor; the verifier being unavailable fails closed.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: Some(0.11),
            ..accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: Some(0.199),
            ..accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: None,
            ..accented_owner
        }));
        // Head alignment, body text, and bounded distance are all required.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            best_window_start: 2,
            ..accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            task_origin_bytes: 2_560_000,
            ..accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            best_distance: 3,
            ..accented_owner
        }));
        // Mid-window, a phrase-only transcript still needs body text — the
        // body may yet arrive inside the open window.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            transcript_chars: 4,
            terminal_window: false,
            ..accented_owner
        }));
    }

    #[test]
    fn terminal_open_near_phrase_accepts_bare_accented_wake_without_body() {
        use OpenNearPhraseWakeEvidence as Evidence;
        // Sessions 2274299279/2274299280 (2026-09-21 10:16): the user said
        // only the accented wake phrase and stopped to wait for the capsule.
        // The phrase-only transcript could never satisfy the body requirement
        // inside that window, the terminal evaluation rejected it after the
        // full window lifetime, and the capsule only appeared when the repeat
        // opened a fresh window ("准备再说一遍的时候它弹出来了"). At terminal
        // the window is closed, so hearing almost the whole phrase head-
        // aligned under the same voiceprint floor may activate directly.
        let bare_accented_owner = Evidence {
            voiceprint_score: Some(0.27),
            task_origin_bytes: 0,
            best_window_start: 0,
            best_distance: 2,
            prefix_units: 1,
            transcript_chars: 4,
            phrase_chars: 4,
            terminal_window: true,
        };
        assert!(open_near_phrase_wake_can_activate(bare_accented_owner));
        // A terminal five-character near-neighbour with zero matching wake
        // prefix caused a false wake despite owner score 0.2186.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: Some(0.2186),
            prefix_units: 0,
            transcript_chars: 5,
            ..bare_accented_owner
        }));
        // A clipped head that still left phrase_chars - 1 units is the same
        // story; losing two units of a four-unit phrase is not a wake.
        assert!(open_near_phrase_wake_can_activate(Evidence {
            transcript_chars: 3,
            ..bare_accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            transcript_chars: 2,
            ..bare_accented_owner
        }));
        // The bystander guard does not depend on the body: media-bare near
        // misses stay below the floor and fail closed.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: Some(0.11),
            ..bare_accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            voiceprint_score: None,
            ..bare_accented_owner
        }));
        // Alignment and distance bounds apply identically.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            best_window_start: 2,
            ..bare_accented_owner
        }));
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            best_distance: 3,
            ..bare_accented_owner
        }));
        // An exact match never routes through this tier.
        assert!(!open_near_phrase_wake_can_activate(Evidence {
            best_distance: 0,
            ..bare_accented_owner
        }));
    }

    #[test]
    fn unrelated_wake_phrase_and_shifted_body_stay_rejected() {
        let unrelated_phrase = OwnerOverlapPhraseEvidence {
            phrase_matched: false,
            phrase_absent: true,
            task_origin_bytes: 0,
            best_window_start: 0,
            best_distance: 4,
            transcript_chars: 9,
            phrase_chars: 4,
        };
        assert!(!owner_overlap_degraded_phrase_evidence(unrelated_phrase));
        assert!(!owner_overlap_degraded_phrase_evidence(
            OwnerOverlapPhraseEvidence {
                best_distance: 2,
                task_origin_bytes: 32_000,
                ..unrelated_phrase
            }
        ));
    }

    #[test]
    fn open_session_wake_owned_body_recovers_unverified_cluster_split() {
        use OpenSessionWakeOwnedBodyEvidence as Evidence;
        // Session 0dbc59da (2026-09-20 20:47): enrolled non-match wake (open
        // acceptance), provider heard 44 body chars, the filter kept only the
        // wake phrase, and the delivery emptied after the wake-phrase strip.
        let drifted_owner = Evidence {
            tracking_enabled: true,
            wake_owner_verified: false,
            owner_isolation_frozen: false,
            hard_non_target_latched: false,
            filtered_is_wake_only: true,
            provider_has_wake_anchored_body: true,
        };
        assert!(open_session_wake_owned_body_can_recover(drifted_owner));
        // The E/F strict isolation is untouched for a bank-verified wake.
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            wake_owner_verified: true,
            ..drifted_owner
        }));
        // A latched hard NonTarget window (media, a real second speaker) keeps
        // its absolute veto even in an unverified session.
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            hard_non_target_latched: true,
            ..drifted_owner
        }));
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            owner_isolation_frozen: true,
            ..drifted_owner
        }));
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            tracking_enabled: false,
            ..drifted_owner
        }));
        // No collapse signature, no recovery: a filter that kept body text is
        // not the failure shape, and a provider without wake-anchored body has
        // nothing session-owned to recover.
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            filtered_is_wake_only: false,
            ..drifted_owner
        }));
        assert!(!open_session_wake_owned_body_can_recover(Evidence {
            provider_has_wake_anchored_body: false,
            ..drifted_owner
        }));
    }

    #[test]
    fn target_speaker_endpoint_final_arbitration_gives_foreign_tail_absolute_veto() {
        let every_recovery_path_open = FinalTranscriptEvidence {
            protocol_final: true,
            explicit_non_owner_tail: true,
            provider_raw_recovery_safe: true,
            provider_owner_recovery_safe: true,
            session_ledger_recovery_safe: true,
            optimistic_owner_recovery_safe: true,
        };
        assert_eq!(
            arbitrate_final_transcript(every_recovery_path_open),
            FinalTranscriptAuthority::SpeakerFiltered
        );
    }

    #[test]
    fn target_speaker_endpoint_final_arbitration_has_one_recovery_precedence() {
        let base = FinalTranscriptEvidence {
            protocol_final: true,
            explicit_non_owner_tail: false,
            provider_raw_recovery_safe: false,
            provider_owner_recovery_safe: false,
            session_ledger_recovery_safe: false,
            optimistic_owner_recovery_safe: false,
        };
        assert_eq!(
            arbitrate_final_transcript(base),
            FinalTranscriptAuthority::SpeakerFiltered
        );
        assert_eq!(
            arbitrate_final_transcript(FinalTranscriptEvidence {
                optimistic_owner_recovery_safe: true,
                ..base
            }),
            FinalTranscriptAuthority::OptimisticOwnerRecovery
        );
        assert_eq!(
            arbitrate_final_transcript(FinalTranscriptEvidence {
                session_ledger_recovery_safe: true,
                optimistic_owner_recovery_safe: true,
                ..base
            }),
            FinalTranscriptAuthority::SessionLedgerRecovery
        );
        assert_eq!(
            arbitrate_final_transcript(FinalTranscriptEvidence {
                provider_owner_recovery_safe: true,
                session_ledger_recovery_safe: true,
                optimistic_owner_recovery_safe: true,
                ..base
            }),
            FinalTranscriptAuthority::ProviderOwnerRecovery
        );
        assert_eq!(
            arbitrate_final_transcript(FinalTranscriptEvidence {
                provider_raw_recovery_safe: true,
                provider_owner_recovery_safe: true,
                optimistic_owner_recovery_safe: true,
                ..base
            }),
            FinalTranscriptAuthority::ProviderRawRecovery
        );
    }

    #[test]
    fn target_speaker_endpoint_product_final_blocks_unverified_recovery_under_interference() {
        let evidence = ProductFinalEvidence {
            target_filter_required: true,
            separated_owner_available: false,
            provider_primary_available: false,
            retained_audio_replay_available: true,
            debug_override_available: true,
            partial_preview_available: true,
        };
        assert_eq!(
            arbitrate_product_final(evidence),
            ProductFinalAuthority::Empty,
            "unverified replay and preview are blocked under isolation"
        );
        assert_eq!(
            arbitrate_product_final(ProductFinalEvidence {
                provider_primary_available: true,
                ..evidence
            }),
            ProductFinalAuthority::ProviderPrimary,
            "the sealed provider result may survive a separator outage"
        );
        assert_eq!(
            arbitrate_product_final(ProductFinalEvidence {
                separated_owner_available: true,
                provider_primary_available: true,
                ..evidence
            }),
            ProductFinalAuthority::SeparatedOwner
        );
    }

    #[test]
    fn target_speaker_endpoint_product_final_has_one_clean_recovery_precedence() {
        let all_recovery_candidates = ProductFinalEvidence {
            target_filter_required: false,
            separated_owner_available: false,
            provider_primary_available: false,
            retained_audio_replay_available: true,
            debug_override_available: true,
            partial_preview_available: true,
        };
        assert_eq!(
            arbitrate_product_final(all_recovery_candidates),
            ProductFinalAuthority::RetainedAudioReplay
        );
        assert_eq!(
            arbitrate_product_final(ProductFinalEvidence {
                retained_audio_replay_available: false,
                ..all_recovery_candidates
            }),
            ProductFinalAuthority::PartialPreviewRecovery
        );
        assert_eq!(
            arbitrate_product_final(ProductFinalEvidence {
                retained_audio_replay_available: false,
                debug_override_available: false,
                ..all_recovery_candidates
            }),
            ProductFinalAuthority::PartialPreviewRecovery
        );
    }

    #[test]
    fn product_final_prefers_provider_over_preview() {
        let evidence = ProductFinalEvidence {
            target_filter_required: false,
            separated_owner_available: false,
            provider_primary_available: true,
            retained_audio_replay_available: false,
            debug_override_available: false,
            partial_preview_available: true,
        };
        assert_eq!(
            arbitrate_product_final(evidence),
            ProductFinalAuthority::ProviderPrimary
        );
        assert_eq!(
            arbitrate_product_final(ProductFinalEvidence {
                target_filter_required: true,
                provider_primary_available: false,
                ..evidence
            }),
            ProductFinalAuthority::Empty,
            "preview-only recovery must be blocked under isolation"
        );
    }
}
