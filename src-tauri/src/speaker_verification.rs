use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceProfileState {
    #[default]
    Unchecked,
    Unenrolled,
    Ready,
    Incompatible,
    Corrupt,
    CredentialError,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentFailureStage {
    Preparation,
    Device,
    Capture,
    Phrase,
    Features,
    Storage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceprintStatus {
    pub available: bool,
    pub runtime_ready: bool,
    pub model_ready: bool,
    pub enrolled: bool,
    pub enrolled_phrase: Option<String>,
    pub requires_reenrollment: bool,
    pub profile_state: VoiceProfileState,
    pub state: String,
    pub progress: u8,
    pub capture_seconds_remaining: Option<u8>,
    pub capture_step: Option<u8>,
    pub capture_step_count: u8,
    pub capture_elapsed_ms: u32,
    pub step_speech_ms: Vec<u16>,
    pub signal_level: u8,
    pub capture_feedback: Option<String>,
    pub score: Option<f32>,
    pub threshold: f32,
    pub error: Option<String>,
    pub failure_stage: Option<EnrollmentFailureStage>,
    pub can_retry_save: bool,
    pub model_name: &'static str,
    pub runtime_version: &'static str,
    pub local_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationPolicy {
    OpenUnenrolled,
    OpenInactiveProfile,
    Enrolled,
}

#[derive(Debug, Clone, Copy)]
pub struct VerificationResult {
    /// Product gate result. Open policies may allow wake without proving owner
    /// identity; callers that need identity must use `owner_matched`.
    pub matched: bool,
    pub owner_matched: bool,
    pub score: f32,
    pub policy: VerificationPolicy,
}

impl VerificationResult {
    pub const fn enrolled_owner_matched(self) -> bool {
        self.owner_matched && matches!(self.policy, VerificationPolicy::Enrolled)
    }

    pub const fn policy_label(self) -> &'static str {
        match self.policy {
            VerificationPolicy::OpenUnenrolled => "open_unenrolled",
            VerificationPolicy::OpenInactiveProfile => "open_inactive_profile",
            VerificationPolicy::Enrolled => "enrolled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSpeakerProfile {
    embeddings: Arc<Vec<Vec<f32>>>,
    adaptive: bool,
}

impl SessionSpeakerProfile {
    pub fn is_adaptive(&self) -> bool {
        self.adaptive
    }

    /// Re-score the same independently measured source, without a second
    /// acoustic inference or weakening the existing speech-quality contract.
    pub(crate) fn classify_observation(&self, observation: &SessionSpeakerObservation) -> SessionSpeakerClassification {
        let score = self.embeddings.iter()
            .filter_map(|reference| normalized_cosine(reference, &observation.embedding))
            .max_by(f32::total_cmp).unwrap_or(f32::NAN);
        if !score.is_finite() || !observation.signal_quality_sufficient {
            SessionSpeakerClassification::Uncertain { score }
        } else {
            session_speaker_classification_for_evidence(score, observation.real_speech_ms)
        }
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_verified_source_context(&self, observation: &SessionSpeakerObservation) -> Self {
        let mut embeddings = self.embeddings.as_ref().clone();
        if observation.signal_quality_sufficient && observation.is_adaptation_candidate() {
            embeddings.push(observation.embedding.clone());
        }
        Self { embeddings: Arc::new(embeddings), adaptive: self.adaptive }
    }
}

/// Word identity's extra natural-speech reference is distinct from both wake
/// admission and endpoint absence. It lives in RAM, belongs to one protected
/// enrollment, and can only be acquired by the immutable enrolled bank itself.
#[cfg(target_os = "windows")]
#[derive(Clone)]
pub(crate) struct NativeWordSpeakerProfile {
    key: [u8; 32],
    base: SessionSpeakerProfile,
    words: SessionSpeakerProfile,
    pub(crate) has_qualified_reference: bool,
}

#[cfg(target_os = "windows")]
impl NativeWordSpeakerProfile {
    pub(crate) fn binding_key(&self) -> [u8; 32] { self.key }
    pub(crate) fn observe(&self, pcm: &[u8]) -> Result<SessionSpeakerObservation, String> {
        if pcm.len() > denzic_speaker_verification_v1_core::DEFAULT_MAX_CANDIDATE_MS as usize * 32 {
            return Err("native source exceeds the identity model's complete-input contract".into());
        }
        observe_session_speaker(&self.base, pcm)
    }

    pub(crate) fn classify(&self, observation: &SessionSpeakerObservation) -> SessionSpeakerClassification {
        self.words.classify_observation(observation)
    }
}

/// A bounded, non-persistent reference for ORIGINAL source chunks. The immutable
/// enrolled/wake profile remains the admission authority: context cannot enroll
/// itself, and a negative original-profile observation always keeps its veto.
/// This avoids both cross-phrase gaps and recursive learning of a room speaker.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedOriginalSourceContext {
    base: SessionSpeakerProfile,
    body_embeddings: Vec<Vec<f32>>,
}

impl VerifiedOriginalSourceContext {
    pub(crate) fn new(base: SessionSpeakerProfile) -> Option<Self> {
        (!base.is_adaptive()).then_some(Self { base, body_embeddings: Vec::new() })
    }

    pub(crate) fn certifies_original_pcm(&mut self, pcm: &[u8]) -> bool {
        self.certifies_original_pcm_with_quality(pcm, None)
    }

    pub(crate) fn certifies_original_pcm_with_quality(&mut self, pcm: &[u8], speech_upper_bound_ms: Option<usize>) -> bool {
        let Ok(mut observation) = observe_session_speaker(&self.base, pcm) else { return false; };
        observation.constrain_speech_duration(speech_upper_bound_ms);
        self.certifies_observation(&observation)
    }

    fn certifies_observation(&mut self, observation: &SessionSpeakerObservation) -> bool {
        if !observation.signal_quality_sufficient {
            return false;
        }
        let owner = match observation.classification {
            SessionSpeakerClassification::Target { .. } => true,
            SessionSpeakerClassification::NonTarget { .. } => false,
            SessionSpeakerClassification::Uncertain { .. } => self.body_embeddings.iter()
                .filter_map(|reference| normalized_cosine(reference, &observation.embedding))
                .any(|score| score >= SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE),
        };
        // Admission uses only this window's BASE observation. A context match
        // never supplies a new reference, and no protected enrollment is written.
        if observation.is_adaptation_candidate()
            && self.body_embeddings.len() < SESSION_SPEAKER_ADAPT_CONFIRMATIONS
            && self.body_embeddings.iter().all(|reference| {
                normalized_cosine(reference, &observation.embedding)
                    .is_none_or(|score| score < ADAPTIVE_BANK_REPRESENTED_MAX_SCORE)
            })
        {
            self.body_embeddings.push(observation.embedding.clone());
        }
        owner
    }
}

/// Separate ownership state for separated output. This can share the immutable
/// enrolled profile, but can never inherit original-mixture body embeddings.
pub(crate) struct VerifiedExtractedOutputContext {
    evidence: VerifiedOriginalSourceContext,
}

impl VerifiedExtractedOutputContext {
    pub(crate) fn new(base: SessionSpeakerProfile) -> Option<Self> {
        VerifiedOriginalSourceContext::new(base).map(|evidence| Self { evidence })
    }

    pub(crate) fn observe(&self, pcm: &[u8]) -> Result<SessionSpeakerObservation, String> {
        observe_session_speaker(&self.evidence.base, pcm)
    }

    pub(crate) fn classify(&mut self, observation: &SessionSpeakerObservation,
        speech_upper_bound_ms: Option<usize>, quality_available: bool,
    ) -> crate::speech_decision_kernel::SeparatedAudioOwnership {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::{Owner, Other, Pending};
        if !quality_available { return Pending; }
        let mut observation = observation.clone();
        observation.constrain_speech_duration(speech_upper_bound_ms);
        if !observation.signal_quality_sufficient { return Pending; }
        if self.evidence.certifies_observation(&observation) { Owner }
        else if matches!(observation.classification, SessionSpeakerClassification::NonTarget { .. }) { Other }
        else { Pending }
    }
}

#[cfg(test)]
mod original_source_context_tests {
    use super::*;

    fn context() -> VerifiedOriginalSourceContext {
        VerifiedOriginalSourceContext::new(SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]), adaptive: false,
        }).unwrap()
    }

    #[test]
    fn extracted_context_cannot_borrow_original_body_or_seed_from_low_quality() {
        use crate::speech_decision_kernel::SeparatedAudioOwnership::{Owner, Pending, Other};
        let mut original = context();
        let seed = observation(SessionSpeakerClassification::Target { score: 0.63 }, vec![1.0,0.0]);
        original.certifies_observation(&seed);
        let mut extracted = VerifiedExtractedOutputContext::new(original.base.clone()).unwrap();
        let uncertain = observation(SessionSpeakerClassification::Uncertain { score: 0.36 }, vec![0.6,0.8]);
        assert_eq!(extracted.classify(&uncertain, Some(2242), true), Pending);
        assert_eq!(extracted.classify(&seed, Some(400), true), Pending);
        assert_eq!(extracted.classify(&seed, Some(2700), false), Pending);
        assert!(extracted.evidence.body_embeddings.is_empty());
        assert_eq!(extracted.classify(&seed, Some(2770), true), Owner);
        assert_eq!(extracted.classify(&uncertain, Some(2242), true), Owner);
        assert_eq!(extracted.evidence.body_embeddings.len(), 1);
        assert_eq!(extracted.classify(&seed, Some(0), true), Pending,
            "no speech in extracted PCM must not inherit speech from the mixture");
        let other = observation(SessionSpeakerClassification::NonTarget { score: 0.05 }, vec![1.0,0.0]);
        assert_eq!(extracted.classify(&other, Some(1200), true), Other);
    }

    fn observation(classification: SessionSpeakerClassification, embedding: Vec<f32>) -> SessionSpeakerObservation {
        SessionSpeakerObservation {
            classification, embedding, real_speech_ms: 1_200,
            signal_quality_sufficient: true,
            transcript_speaker_evidence: crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
        }
    }

    #[test]
    fn independent_original_owner_context_covers_a_cross_phrase_gap() {
        let mut context = context();
        assert!(!context.certifies_observation(&observation(
            SessionSpeakerClassification::Uncertain { score: 0.35 }, vec![0.6, 0.8])));
        assert!(context.certifies_observation(&observation(
            SessionSpeakerClassification::Target { score: 0.64 }, vec![1.0, 0.0])));
        assert!(context.certifies_observation(&observation(
            SessionSpeakerClassification::Uncertain { score: 0.35 }, vec![0.6, 0.8])));
        assert_eq!(context.base.embeddings.len(), 1, "enrolled/wake authority remains immutable");
    }

    #[test]
    fn context_matches_cannot_recursively_enroll_a_room_speaker() {
        let mut context = context();
        context.certifies_observation(&observation(
            SessionSpeakerClassification::Target { score: 0.64 }, vec![1.0, 0.0]));
        assert!(context.certifies_observation(&observation(
            SessionSpeakerClassification::Uncertain { score: 0.35 }, vec![0.6, 0.8])));
        assert_eq!(context.body_embeddings.len(), 1);
        assert!(!context.certifies_observation(&observation(
            SessionSpeakerClassification::Uncertain { score: 0.25 }, vec![0.0, 1.0])));
    }

    #[test]
    fn base_negative_and_insufficient_signal_keep_their_veto_over_context() {
        let mut context = context();
        context.certifies_observation(&observation(
            SessionSpeakerClassification::Target { score: 0.64 }, vec![1.0, 0.0]));
        assert!(!context.certifies_observation(&observation(
            SessionSpeakerClassification::NonTarget { score: 0.05 }, vec![1.0, 0.0])));
        let mut weak = observation(SessionSpeakerClassification::Target { score: 0.93 }, vec![0.0, 1.0]);
        weak.signal_quality_sufficient = false;
        assert!(!context.certifies_observation(&weak));
        assert_eq!(context.body_embeddings.len(), 1);
    }

    #[test]
    fn continuous_speech_bound_removes_short_pause_identity_authority() {
        let mut sample = observation(SessionSpeakerClassification::NonTarget { score: 0.093767904 }, vec![1.0, 0.0]);
        sample.transcript_speaker_evidence = crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget;
        sample.constrain_speech_duration(Some(898));
        assert_eq!(sample.real_speech_ms, 898);
        assert!(!sample.signal_quality_sufficient);
        assert!(matches!(sample.classification, SessionSpeakerClassification::Uncertain { .. }));
        assert_eq!(sample.transcript_speaker_evidence, crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive);

        let mut owner = observation(SessionSpeakerClassification::Target { score: 0.8 }, vec![1.0, 0.0]);
        owner.constrain_speech_duration(Some(400));
        assert!(matches!(owner.classification, SessionSpeakerClassification::Target { .. }), "short owner evidence remains advisory, never Other");
        assert!(!owner.signal_quality_sufficient);
        assert!(!owner.is_adaptation_candidate());
    }

    #[test]
    fn speech_bound_never_promotes_identity_or_invents_missing_silence() {
        let mut foreign = observation(SessionSpeakerClassification::NonTarget { score: 0.05 }, vec![0.0, 1.0]);
        foreign.constrain_speech_duration(Some(1200));
        assert!(foreign.signal_quality_sufficient);
        assert!(matches!(foreign.classification, SessionSpeakerClassification::NonTarget { .. }));
        let mut uncertain = observation(SessionSpeakerClassification::Uncertain { score: 0.3 }, vec![1.0, 0.0]);
        uncertain.constrain_speech_duration(None);
        assert_eq!(uncertain.real_speech_ms, 1200);
        assert!(matches!(uncertain.classification, SessionSpeakerClassification::Uncertain { .. }));
    }

    #[test]
    fn source_context_is_bounded_and_open_wake_profiles_cannot_create_one() {
        let mut context = context();
        for embedding in [vec![1.0, 0.0], vec![0.7, 0.714], vec![0.0, 1.0]] {
            context.certifies_observation(&observation(
                SessionSpeakerClassification::Target { score: 0.64 }, embedding));
        }
        assert_eq!(context.body_embeddings.len(), SESSION_SPEAKER_ADAPT_CONFIRMATIONS);
        assert!(VerifiedOriginalSourceContext::new(SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]), adaptive: true,
        }).is_none());
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionSpeakerClassification {
    Target { score: f32 },
    NonTarget { score: f32 },
    Uncertain { score: f32 },
}

#[derive(Debug, Clone)]
pub struct SessionSpeakerObservation {
    pub classification: SessionSpeakerClassification,
    pub real_speech_ms: usize,
    /// Only adequately voiced windows may imply interference from score drift.
    pub signal_quality_sufficient: bool,
    /// Qualified negative evidence stays distinct from coarse classification.
    /// Short hints can corroborate provider diarization; only the strong tier
    /// can qualify negative original-window presence. Neither tier by itself
    /// may erase owner text or override pending-body/receipt STOP protection.
    pub transcript_speaker_evidence: crate::speech_decision_kernel::TranscriptSpeakerEvidence,
    embedding: Vec<f32>,
}

#[derive(Debug, Default)]
pub struct SessionSpeakerAdaptationGate {
    consecutive_candidates: Vec<(u64, SessionSpeakerObservation)>,
    pending_candidates: std::collections::VecDeque<(u64, SessionSpeakerObservation)>,
    bootstrap_candidates: Vec<SessionSpeakerObservation>,
    bootstrap_pending: Vec<SessionSpeakerObservation>,
    bootstrap_consecutive_non_target: usize,
    bootstrap_disqualified: bool,
    bootstrap_complete: bool,
}

pub(crate) const SESSION_SPEAKER_MIN_NON_TARGET_MS: usize = 1_000;
// The ESP32 microphone's idle floor is normally around 60-150 RMS while real
// speech windows in installed sessions are in the thousands.  A flat idle
// window used to satisfy the relative VAD (all frames are equally "active"),
// produce a low cosine score, and become a confident NonTarget observation.
// Identity decisions need an absolute signal floor as well: below this level
// the sample remains advisory/Uncertain and cannot end or erase a session.
const SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS: f32 = 512.0;
const SESSION_SPEAKER_ADAPT_MIN_SCORE: f32 = 0.50;
const SESSION_SPEAKER_ADAPT_CONFIRMATIONS: usize = 2;
const SESSION_SPEAKER_BOOTSTRAP_CONFIRMATIONS: usize = 3;
const SESSION_SPEAKER_ADAPT_PENDING_LIMIT: usize = 6;
const SESSION_SPEAKER_MAX_EMBEDDINGS: usize = 4;

impl SessionSpeakerClassification {
    pub fn score(self) -> f32 {
        match self {
            Self::Target { score } | Self::NonTarget { score } | Self::Uncertain { score } => score,
        }
    }
}

impl SessionSpeakerObservation {
    /// A coarse mismatch is a candidate, not confirmed foreign identity.
    /// Keep the existing independently qualified negative-evidence tier at
    /// this boundary; signal strength alone cannot upgrade an inconclusive
    /// identity result. Recording still separately protects pending body text.
    pub(crate) fn endpoint_presence_identity(&self) -> crate::personal_vad::PresenceState {
        use crate::personal_vad::PresenceState;
        match self.classification {
            SessionSpeakerClassification::Target { .. } => PresenceState::Owner,
            SessionSpeakerClassification::NonTarget { .. }
                if self.signal_quality_sufficient
                    && self.transcript_speaker_evidence
                        == crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget =>
            {
                PresenceState::ConfirmedOther
            }
            _ => PresenceState::Unknown,
        }
    }

    /// Continuous VAD may disprove the energy-only duration. Missing VAD is
    /// not silence, and even a complete VAD interval cannot grant identity.
    pub(crate) fn constrain_speech_duration(&mut self, upper_bound_ms: Option<usize>) {
        let Some(upper_bound_ms) = upper_bound_ms else { return; };
        self.real_speech_ms = self.real_speech_ms.min(upper_bound_ms);
        if self.real_speech_ms < SESSION_SPEAKER_MIN_NON_TARGET_MS {
            self.signal_quality_sufficient = false;
            if let SessionSpeakerClassification::NonTarget { score } = self.classification {
                self.classification = SessionSpeakerClassification::Uncertain { score };
            }
            self.transcript_speaker_evidence = crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive;
        }
    }

    fn is_adaptation_candidate(&self) -> bool {
        self.real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS
            && matches!(
                self.classification,
                SessionSpeakerClassification::Target { score }
                    if score >= SESSION_SPEAKER_ADAPT_MIN_SCORE
            )
    }

    fn is_bootstrap_candidate(&self) -> bool {
        self.real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS
            && matches!(
                self.classification,
                SessionSpeakerClassification::Target { .. }
            )
    }

    /// Embedding of the observed window, for bank-admission callers only.
    /// Attribution callers must keep consuming the classification: an
    /// embedding must never influence whether text survives a session.
    pub fn embedding(&self) -> &[f32] {
        &self.embedding
    }
}

impl SessionSpeakerAdaptationGate {
    pub fn note(&mut self, audio_end_ms: u64, observation: SessionSpeakerObservation) {
        if !self.bootstrap_complete && !self.bootstrap_disqualified {
            match observation.classification {
                SessionSpeakerClassification::Target { .. }
                    if observation.is_bootstrap_candidate() =>
                {
                    self.bootstrap_consecutive_non_target = 0;
                    self.bootstrap_candidates.push(observation.clone());
                    if self.bootstrap_candidates.len() >= SESSION_SPEAKER_BOOTSTRAP_CONFIRMATIONS {
                        self.bootstrap_pending = std::mem::take(&mut self.bootstrap_candidates);
                    }
                }
                SessionSpeakerClassification::NonTarget { .. } => {
                    self.bootstrap_candidates.clear();
                    self.bootstrap_consecutive_non_target =
                        self.bootstrap_consecutive_non_target.saturating_add(1);
                    if self.bootstrap_consecutive_non_target >= 2 {
                        self.bootstrap_disqualified = true;
                    }
                }
                SessionSpeakerClassification::Target { .. } => {
                    // 未满一整窗(<1000ms)的 Target：不算一格，但也不清空连击——
                    // 它仍是本人证据。LST-REC-025 只让 Uncertain/NonTarget 打断
                    // bootstrap 连击；2026-08-07 07:23 session 里 800/600ms 的
                    // Target 窗把连击清零，bootstrap 从未成立，后段声纹漂移冻结
                    // 在 8 字（「你看一」事故）。
                }
                _ => {
                    self.bootstrap_candidates.clear();
                    self.bootstrap_consecutive_non_target = 0;
                }
            }
        }
        if !observation.is_adaptation_candidate() {
            self.consecutive_candidates.clear();
            return;
        }
        self.consecutive_candidates
            .push((audio_end_ms, observation));
        if self.consecutive_candidates.len() < SESSION_SPEAKER_ADAPT_CONFIRMATIONS {
            return;
        }
        self.pending_candidates
            .extend(self.consecutive_candidates.drain(..));
        while self.pending_candidates.len() > SESSION_SPEAKER_ADAPT_PENDING_LIMIT {
            self.pending_candidates.pop_front();
        }
    }

    pub fn promote_covered(
        &mut self,
        profile: &mut SessionSpeakerProfile,
        stable_target_end_ms: Option<u64>,
    ) -> usize {
        if !self.bootstrap_complete && !self.bootstrap_pending.is_empty() {
            let added = bootstrap_session_speaker_profile(profile, &self.bootstrap_pending);
            self.bootstrap_pending.clear();
            if added > 0 {
                self.bootstrap_complete = true;
                return added;
            }
        }
        let Some(stable_target_end_ms) = stable_target_end_ms else {
            return 0;
        };
        let mut covered = Vec::new();
        while self
            .pending_candidates
            .front()
            .is_some_and(|(audio_end_ms, _)| *audio_end_ms <= stable_target_end_ms)
        {
            if let Some((_, observation)) = self.pending_candidates.pop_front() {
                covered.push(observation);
            }
        }
        adapt_session_speaker_profile(profile, &covered)
    }
}

fn bootstrap_session_speaker_profile(
    profile: &mut SessionSpeakerProfile,
    observations: &[SessionSpeakerObservation],
) -> usize {
    if !profile.adaptive || profile.embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS {
        return 0;
    }
    let expected_dimension = profile.embeddings.first().map_or(0, Vec::len);
    let available = SESSION_SPEAKER_MAX_EMBEDDINGS.saturating_sub(profile.embeddings.len());
    let additions = observations
        .iter()
        .filter(|observation| {
            observation.is_bootstrap_candidate()
                && observation.embedding.len() == expected_dimension
        })
        .take(available)
        .map(|observation| observation.embedding.clone())
        .collect::<Vec<_>>();
    let count = additions.len();
    Arc::make_mut(&mut profile.embeddings).extend(additions);
    count
}

fn adapt_session_speaker_profile(
    profile: &mut SessionSpeakerProfile,
    observations: &[SessionSpeakerObservation],
) -> usize {
    if !profile.adaptive || profile.embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS {
        return 0;
    }
    let expected_dimension = profile.embeddings.first().map_or(0, Vec::len);
    let available = SESSION_SPEAKER_MAX_EMBEDDINGS.saturating_sub(profile.embeddings.len());
    let additions = observations
        .iter()
        .filter(|observation| {
            observation.is_adaptation_candidate()
                && observation.embedding.len() == expected_dimension
        })
        .take(available)
        .map(|observation| observation.embedding.clone())
        .collect::<Vec<_>>();
    let count = additions.len();
    Arc::make_mut(&mut profile.embeddings).extend(additions);
    count
}

/// Add the already-verified wake voice as a session-only reference for an
/// enrolled owner. Persistent enrollment may have been recorded at another
/// distance, volume, or on another day; comparing natural dictation only with
/// those old wake-phrase samples left otherwise valid owner speech in the
/// Uncertain band for an entire session.
///
/// This deliberately does not change `adaptive`: enrolled profiles still
/// cannot learn from later body windows, and this exemplar is never persisted.
fn append_verified_wake_session_exemplar(
    embeddings: &mut Vec<Vec<f32>>,
    verified_wake_embedding: Vec<f32>,
) -> bool {
    if embeddings.is_empty()
        || embeddings.len() >= SESSION_SPEAKER_MAX_EMBEDDINGS
        || verified_wake_embedding.len() != embeddings[0].len()
        || verified_wake_embedding.is_empty()
    {
        return false;
    }
    embeddings.push(verified_wake_embedding);
    true
}

/// Compose the session-local speaker reference from the persisted owner bank
/// and the wake exemplar that anchors the session, and label which path
/// supplied the anchor.
///
/// Both paths take the UNION of bank + fresh anchor. The offline matrix
/// (2026-09-21, session a9a10419 vs 77d4646b/042be35a/39a5c9d0) settled the
/// composition question with measured numbers:
///
/// - The owner's body windows score mid-band (0.19-0.56) against EVERY
///   reference — anchor, wake bank, free bank alike. The bank "failing" the
///   wake (below 0.42 on a short utterance) does not mean it fails the body:
///   body-vs-bank ran 0.28-0.49 in the very session whose wake it rejected
///   (short-phrase embeddings are noisier than 7s of body speech — that gap
///   is the enrollment/verification window mismatch, not bank invalidity).
/// - A single anchor-only window at 0.187 (t=5470 in a9a10419) crossed the
///   0.20 hard-NonTarget floor, latched the non-target veto, and blocked the
///   open-session body recovery → 48 chars collapsed to the wake phrase →
///   "没有识别到语音". With the union the same window scored 0.267 via the
///   wake bank and stayed Uncertain. `observe_session_speaker` scores by max
///   cosine, so the union can only lift owner windows, never lower them.
/// - Real media tails measured 0.005-0.14 against BOTH the anchor and the
///   same-day bank — the "bank-like media rides the bank" risk never
///   materialized in the field; the observed failures are all owner-side.
///
/// The bank is still not a gate: the wake open-accepts on exact phrase, the
/// body is session-owned once the wake passed (tid), and adaptation only
/// writes on bank-verified evidence. Whoever passed the exact-phrase gate
/// owns the session (r40, tik d4c45b6d: dropping the anchor entirely for
/// open acceptance let the drifted bank turn every body window strong
/// NonTarget and `no_body_3000ms` cut the read).
fn session_target_with_wake_anchor(
    mut bank: Vec<Vec<f32>>,
    verified_wake_embedding: Vec<f32>,
    live_wake_matches_enrolled_owner: bool,
) -> (Vec<Vec<f32>>, &'static str) {
    if append_verified_wake_session_exemplar(&mut bank, verified_wake_embedding) {
        return (
            bank,
            if live_wake_matches_enrolled_owner {
                "enrolled_match"
            } else {
                "open_acceptance"
            },
        );
    }
    (bank, "bank_only")
}

/// 银行自适应 (persisted-bank adaptation) constants and pure decision logic.
///
/// The persisted owner bank is written by guided enrollment today, so a bank
/// recorded on one quiet morning drifts away from the same voice on other
/// days (installed 2026-09-18/19: 0.58-0.68 at enrollment → ~0.20 the next
/// morning) and the only remedy has been "record it again". Adaptation
/// instead lets the bank refresh itself from live audio the bank ALREADY
/// verified — never from open-acceptance wakes, never from windows the bank
/// did not confidently claim (the `ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE`
/// floor sits above the Target threshold, so a media-mixed 0.26-0.44 window
/// can never enter the bank either).
///
/// Slot policy: slot 0 of each bank keeps the original guided-enrollment
/// anchor forever (a mis-admitted exemplar can therefore never fully own the
/// bank), and only the churn slot tracks the freshest verified voice. A
/// candidate that is already well represented (similarity >=
/// `ADAPTIVE_BANK_REPRESENTED_MAX_SCORE` against ANY slot) is skipped, which
/// is also the write-churn control.
const ADAPTIVE_BANK_REPRESENTED_MAX_SCORE: f32 = 0.80;
pub(crate) const ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE: f32 = 0.60;
const ADAPTIVE_BANK_CHURN_SLOT: usize = 1;

fn normalized_cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.is_empty() || left.len() != right.len() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return None;
    }
    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

/// May a live exemplar that this session just verified refresh the persisted
/// bank? Requires at least the anchor plus one more slot, matching embedding
/// dimensions across the whole bank, and no existing slot that already
/// represents the candidate closely.
pub(crate) fn adaptive_bank_should_refresh(bank: &[Vec<f32>], candidate: &[f32]) -> bool {
    !candidate.is_empty()
        && bank.len() > ADAPTIVE_BANK_CHURN_SLOT
        && bank.iter().all(|slot| slot.len() == candidate.len())
        && bank.iter().all(|slot| {
            normalized_cosine(slot, candidate)
                .is_none_or(|similarity| similarity < ADAPTIVE_BANK_REPRESENTED_MAX_SCORE)
        })
}

// 会话分段分类阈值。2026-09-23 71f64ac1 矩阵重校（ef_session_score_bands_
// matrix，12 会话真实 WAV + 当晚干扰实测）：本人的连续说话声对"档案=
// 3 条 TTS 银行 + 1 条当会话唤醒锚点"的 max-cosine 活在 0.42–0.59，跨会话
// 峰值 0.36–0.55——0.55 的确信线本人几乎永远够不着（当晚本人全程
// Uncertain → owner_isolation 冻结在旁人插话后永不解除，解冻要求连续
// Target 窗）。旧线防的 0.426–0.58"第二个人"是 pre-2026-09-18 TTS 银行
// 时代的测量（volcengine.rs 注释 anchor-era 实测旁人 <=0.11，当晚真实
// 旁人 <=0.28）。线收到 0.40：本人带 0.42+ 全覆盖，旁人余量 0.12，
// NonTarget 线 0.20 不动。中间分仍作 Uncertain，绝不因 Uncertain 冻结
// 截断正文。
const SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE: f32 = 0.40;
const SESSION_SPEAKER_NON_TARGET_MAX_SCORE: f32 = 0.20;

fn session_speaker_classification_for_score(score: f32) -> SessionSpeakerClassification {
    if score >= SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE {
        SessionSpeakerClassification::Target { score }
    } else if score <= SESSION_SPEAKER_NON_TARGET_MAX_SCORE {
        SessionSpeakerClassification::NonTarget { score }
    } else {
        SessionSpeakerClassification::Uncertain { score }
    }
}

fn session_speaker_classification_for_evidence(
    score: f32,
    real_speech_ms: usize,
) -> SessionSpeakerClassification {
    let classification = session_speaker_classification_for_score(score);
    if real_speech_ms < SESSION_SPEAKER_MIN_NON_TARGET_MS
        && matches!(
            classification,
            SessionSpeakerClassification::NonTarget { .. }
        )
    {
        SessionSpeakerClassification::Uncertain { score }
    } else {
        classification
    }
}

fn session_speaker_signal_quality_sufficient(real_speech_ms: usize, peak_rms: f32) -> bool {
    real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS
        && peak_rms >= SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS
}

fn session_speaker_classification_for_signal(
    score: f32,
    real_speech_ms: usize,
    peak_rms: f32,
) -> SessionSpeakerClassification {
    let classification = session_speaker_classification_for_evidence(score, real_speech_ms);
    if peak_rms < SESSION_SPEAKER_MIN_IDENTITY_PEAK_RMS {
        SessionSpeakerClassification::Uncertain { score }
    } else {
        classification
    }
}

#[cfg(target_os = "windows")]
mod platform {
    #[cfg(test)]
    use super::session_speaker_classification_for_evidence;
    use super::{
        session_speaker_classification_for_signal, SessionSpeakerClassification,
        SessionSpeakerObservation, SessionSpeakerProfile, VerificationPolicy, VerificationResult,
        EnrollmentFailureStage, VoiceProfileState, VoiceprintStatus,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    #[cfg(test)]
    use denzic_speaker_verification_v1_core::DEFAULT_SCORE_MILLI;
    use denzic_speaker_verification_v1_core::{
        Action, CandidateOrigin, Input, Machine, Verdict, DEFAULT_MAX_CANDIDATE_MS,
    };
    use libloading::Library;
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::borrow::Cow;
    use std::ffi::{c_char, c_void, CString};
    use std::fs;
    use std::io::{BufReader, Read};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    const RUNTIME_VERSION: &str = "1.13.1";
    const MODEL_NAME: &str = "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const MODEL_SHA256: &str = "F682B514C05D947EE3FA91CD6EC6C5C7543479A128373FA29B1FAEDCCD21FD11";
    const TARGET_SPEAKER_MODEL_SHA256: &str =
        "9CEC30564E3A87746BDD44108779EDDF5323CD9ED4BD0966B64883A6EEBBF46E";
    const TARGET_SPEAKER_EMBEDDING_DIMENSION: usize = 192;
    const RUNTIME_ARCHIVE_SHA256: &str =
        "6760B0E25EAAD0DADFFBA9029B1270778E0DBFD43F314CF070B2F9C1DCB4AF25";
    const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";
    const RUNTIME_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.1/sherpa-onnx-v1.13.1-win-x64-shared-MD-Release-no-tts.tar.bz2";
    const KEYRING_SERVICE: &str = "com.listener.type.voiceprint";
    const KEYRING_ACCOUNT: &str = "owner-template-v1";
    const KEYRING_SUPPLEMENTAL_ACCOUNT_PREFIX: &str = "owner-template-v2-";
    // v3 enrollment stores independent wake and session-comparison banks. Keep
    // the existing account prefix so upgrades can read/delete v1/v2 material,
    // but reserve enough protected entries for both banks.
    const MAX_SUPPLEMENTAL_TEMPLATES: usize = 7;
    const SAMPLE_RATE: i32 = 16_000;
    const VERIFICATION_MIN_SPEECH_MS: usize = 1_000;
    // Xiaomi-style guided enrollment: three independent wake-phrase slots. Keep
    // one continuous device capture so the guidance does not introduce BLE
    // start/stop races. The same three samples also seed the session-comparison
    // bank, so model implementation details do not add a fourth user step.
    const ENROLLMENT_WAKE_STEP_SECONDS: u64 = 3;
    const ENROLLMENT_WAKE_STEPS: u64 = 3;
    const ENROLLMENT_STEP_COUNT: usize = 3;
    const ENROLLMENT_WAKE_SECONDS: u64 = ENROLLMENT_WAKE_STEP_SECONDS * ENROLLMENT_WAKE_STEPS;
    const ENROLLMENT_SECONDS: u64 = ENROLLMENT_WAKE_SECONDS;
    // The host stops at the nine-second boundary, but the last BLE audio packet
    // can arrive just before the control STOP (the observed failure was 20 ms
    // short with zero missing packets). Tolerate only that bounded transport
    // tail and pad it with silence; a genuinely missing third phrase still goes
    // through the per-step speech and KWS gates below.
    const ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS: usize = 250;
    const ENROLLMENT_MIN_SECONDS: usize = 5;
    const ENROLLMENT_FRAME_MS: usize = 100;
    const ENROLLMENT_MIN_ACTIVE_FRAMES: usize = 18;
    const TEMPLATE_WINDOW_MS: usize = 1_600;
    const ENROLLMENT_TEMPLATE_WINDOW_MS: usize = 1_000;
    const DUAL_TEMPLATE_WINDOWS_PER_BANK: usize = 3;
    const ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP: usize = 6;
    // Product sensitivity: platform DEFAULT_SCORE_MILLI is 500 (0.50). Real-owner
    // wake in mild noise often scores ~0.43–0.55; 0.50 cut too many true hits.
    // Keep below same-speaker unit-test floor and well above typical non-owner.
    const VERIFICATION_THRESHOLD: f32 = 0.42;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CaptureState {
        Idle,
        Preparing,
        Armed,
        Capturing,
        Processing,
        Complete,
        Error,
    }

    impl CaptureState {
        fn label(self) -> &'static str {
            match self {
                Self::Idle => "idle",
                Self::Preparing => "preparing",
                Self::Armed => "armed",
                Self::Capturing => "capturing",
                Self::Processing => "processing",
                Self::Complete => "complete",
                Self::Error => "error",
            }
        }
    }

    #[derive(Default)]
    struct State {
        capture: Option<CaptureState>,
        progress: u8,
        error: Option<String>,
        failure_stage: Option<EnrollmentFailureStage>,
        pending_template: Option<SpeakerTemplate>,
        enrollment_generation: u64,
        native_word_reference: Option<([u8; 32], Vec<f32>)>,
        last_score: Option<f32>,
        template: Option<SpeakerTemplate>,
        template_checked: bool,
        profile_state: VoiceProfileState,
        enrollment_phrase: Option<String>,
        enrollment_capture_started: Option<std::time::Instant>,
        enrollment_live: EnrollmentLiveState,
        runtime: Option<Arc<SpeakerRuntime>>,
    }

    #[derive(Default)]
    struct EnrollmentLiveState {
        analyzed_bytes: usize,
        step_speech_ms: [u16; 3],
        signal_level: u8,
        last_frame_active: bool,
        last_frame_clipped: bool,
    }

    static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));
    static INFERENCE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct StoredTemplate {
        version: u8,
        model_sha256: String,
        dimension: usize,
        embedding_base64: String,
        #[serde(default)]
        phrase: Option<String>,
        #[serde(default)]
        invalidated: bool,
        #[serde(default)]
        purpose: Option<TemplatePurpose>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frontend_id: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum TemplatePurpose {
        WakePhrase,
        FreeSpeech,
        TargetSpeaker,
        PersonalActivity,
    }

    #[derive(Debug, Clone)]
    struct SpeakerTemplate {
        wake_embeddings: Vec<Vec<f32>>,
        session_embeddings: Vec<Vec<f32>>,
        target_speaker_embedding: Option<Vec<f32>>,
        personal_activity_reference: Option<crate::personal_vad::PersonalVadReference>,
        phrase: Option<String>,
        invalidated: bool,
    }

    #[repr(C)]
    struct ExtractorConfig {
        model: *const c_char,
        num_threads: i32,
        debug: i32,
        provider: *const c_char,
    }

    type CreateExtractor = unsafe extern "C" fn(*const ExtractorConfig) -> *const c_void;
    type DestroyExtractor = unsafe extern "C" fn(*const c_void);
    type ExtractorDim = unsafe extern "C" fn(*const c_void) -> i32;
    type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
    type AcceptWaveform = unsafe extern "C" fn(*const c_void, i32, *const f32, i32);
    type InputFinished = unsafe extern "C" fn(*const c_void);
    type IsReady = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
    type ComputeEmbedding = unsafe extern "C" fn(*const c_void, *const c_void) -> *const f32;
    type DestroyEmbedding = unsafe extern "C" fn(*const f32);
    type DestroyStream = unsafe extern "C" fn(*const c_void);

    struct SpeakerRuntime {
        _onnx: Library,
        _providers: Library,
        _sherpa: Library,
        extractor: *const c_void,
        dimension: usize,
        create_stream: CreateStream,
        accept_waveform: AcceptWaveform,
        input_finished: InputFinished,
        is_ready: IsReady,
        compute_embedding: ComputeEmbedding,
        destroy_embedding: DestroyEmbedding,
        destroy_stream: DestroyStream,
        destroy_extractor: DestroyExtractor,
    }

    unsafe impl Send for SpeakerRuntime {}
    unsafe impl Sync for SpeakerRuntime {}

    impl Drop for SpeakerRuntime {
        fn drop(&mut self) {
            unsafe { (self.destroy_extractor)(self.extractor) };
        }
    }

    impl SpeakerRuntime {
        fn load(root: &Path) -> Result<Self, String> {
            Self::load_model(root, &root.join(MODEL_NAME))
        }

        fn load_model(root: &Path, model_path: &Path) -> Result<Self, String> {
            let onnx_path = root.join("onnxruntime.dll");
            let providers_path = root.join("onnxruntime_providers_shared.dll");
            let sherpa_path = root.join("sherpa-onnx-c-api.dll");
            let model = CString::new(model_path.to_string_lossy().as_bytes())
                .map_err(|_| "voiceprint model path contains an invalid character".to_string())?;
            let provider = CString::new("cpu").expect("literal has no nul");

            unsafe {
                let onnx = Library::new(&onnx_path)
                    .map_err(|err| format!("load onnxruntime.dll failed: {err}"))?;
                let providers = Library::new(&providers_path)
                    .map_err(|err| format!("load onnxruntime providers failed: {err}"))?;
                let sherpa = Library::new(&sherpa_path)
                    .map_err(|err| format!("load sherpa-onnx C API failed: {err}"))?;
                let create_extractor: CreateExtractor = *sherpa
                    .get(b"SherpaOnnxCreateSpeakerEmbeddingExtractor\0")
                    .map_err(|err| format!("missing speaker extractor API: {err}"))?;
                let destroy_extractor: DestroyExtractor = *sherpa
                    .get(b"SherpaOnnxDestroySpeakerEmbeddingExtractor\0")
                    .map_err(|err| format!("missing speaker extractor destroy API: {err}"))?;
                let extractor_dim: ExtractorDim = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorDim\0")
                    .map_err(|err| format!("missing speaker dimension API: {err}"))?;
                let create_stream: CreateStream = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorCreateStream\0")
                    .map_err(|err| format!("missing speaker stream API: {err}"))?;
                let accept_waveform: AcceptWaveform = *sherpa
                    .get(b"SherpaOnnxOnlineStreamAcceptWaveform\0")
                    .map_err(|err| format!("missing waveform API: {err}"))?;
                let input_finished: InputFinished = *sherpa
                    .get(b"SherpaOnnxOnlineStreamInputFinished\0")
                    .map_err(|err| format!("missing input-finished API: {err}"))?;
                let is_ready: IsReady = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorIsReady\0")
                    .map_err(|err| format!("missing speaker ready API: {err}"))?;
                let compute_embedding: ComputeEmbedding = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding\0")
                    .map_err(|err| format!("missing speaker compute API: {err}"))?;
                let destroy_embedding: DestroyEmbedding = *sherpa
                    .get(b"SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding\0")
                    .map_err(|err| format!("missing speaker embedding destroy API: {err}"))?;
                let destroy_stream: DestroyStream = *sherpa
                    .get(b"SherpaOnnxDestroyOnlineStream\0")
                    .map_err(|err| format!("missing speaker stream destroy API: {err}"))?;
                let config = ExtractorConfig {
                    model: model.as_ptr(),
                    num_threads: 1,
                    debug: 0,
                    provider: provider.as_ptr(),
                };
                let extractor = create_extractor(&config);
                if extractor.is_null() {
                    return Err("voiceprint model initialization failed".to_string());
                }
                let dimension = extractor_dim(extractor);
                if dimension <= 0 || dimension > 4096 {
                    destroy_extractor(extractor);
                    return Err(format!(
                        "voiceprint model returned invalid dimension: {dimension}"
                    ));
                }
                Ok(Self {
                    _onnx: onnx,
                    _providers: providers,
                    _sherpa: sherpa,
                    extractor,
                    dimension: dimension as usize,
                    create_stream,
                    accept_waveform,
                    input_finished,
                    is_ready,
                    compute_embedding,
                    destroy_embedding,
                    destroy_stream,
                    destroy_extractor,
                })
            }
        }

        fn embedding(&self, pcm: &[u8]) -> Result<Vec<f32>, String> {
            let _inference_guard = INFERENCE_LOCK.lock();
            let samples = prepare_embedding_samples(pcm)?;
            let sample_count =
                i32::try_from(samples.len()).map_err(|_| "voiceprint audio is too long")?;
            unsafe {
                let stream = (self.create_stream)(self.extractor);
                if stream.is_null() {
                    return Err("create voiceprint feature stream failed".to_string());
                }
                (self.accept_waveform)(stream, SAMPLE_RATE, samples.as_ptr(), sample_count);
                (self.input_finished)(stream);
                if (self.is_ready)(self.extractor, stream) == 0 {
                    (self.destroy_stream)(stream);
                    return Err("voiceprint audio has insufficient speech".to_string());
                }
                let raw = (self.compute_embedding)(self.extractor, stream);
                if raw.is_null() {
                    (self.destroy_stream)(stream);
                    return Err("voiceprint feature extraction failed".to_string());
                }
                let mut embedding = std::slice::from_raw_parts(raw, self.dimension).to_vec();
                (self.destroy_embedding)(raw);
                (self.destroy_stream)(stream);
                normalize(&mut embedding)?;
                Ok(embedding)
            }
        }
    }

    fn prepare_embedding_samples(pcm: &[u8]) -> Result<Vec<f32>, String> {
        let samples = pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
            .collect::<Vec<_>>();
        let minimum_speech_samples = SAMPLE_RATE as usize * VERIFICATION_MIN_SPEECH_MS / 1000;
        if samples.len() < minimum_speech_samples {
            return Err(format!(
                "voiceprint audio is shorter than {VERIFICATION_MIN_SPEECH_MS} ms"
            ));
        }
        Ok(samples)
    }

    fn normalize(values: &mut [f32]) -> Result<(), String> {
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return Err("voiceprint feature is invalid".to_string());
        }
        for value in values {
            *value /= norm;
        }
        Ok(())
    }

    fn cosine(left: &[f32], right: &[f32]) -> Result<f32, String> {
        if left.len() != right.len() || left.is_empty() {
            return Err("voiceprint dimensions do not match".to_string());
        }
        Ok(left.iter().zip(right).map(|(a, b)| a * b).sum())
    }

    fn frame_rms(pcm: &[u8], frame_bytes: usize) -> Vec<f32> {
        pcm.chunks(frame_bytes)
            .map(|frame| {
                let mut sum = 0.0f64;
                let mut count = 0usize;
                for sample in frame.chunks_exact(2) {
                    let value = i16::from_le_bytes([sample[0], sample[1]]) as f64;
                    sum += value * value;
                    count += 1;
                }
                if count == 0 {
                    0.0
                } else {
                    (sum / count as f64).sqrt() as f32
                }
            })
            .collect()
    }

    fn enrollment_step_for_elapsed_ms(elapsed_ms: usize) -> usize {
        (elapsed_ms / (ENROLLMENT_WAKE_STEP_SECONDS as usize * 1_000))
            .min(ENROLLMENT_STEP_COUNT - 1)
    }

    fn enrollment_step_target_ms(_step: usize) -> u16 {
        (ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP * ENROLLMENT_FRAME_MS) as u16
    }

    fn enrollment_capture_feedback(
        step: usize,
        elapsed_ms: usize,
        live: &EnrollmentLiveState,
    ) -> &'static str {
        if live.last_frame_clipped {
            "too_loud"
        } else if live.step_speech_ms[step] >= enrollment_step_target_ms(step) {
            "good"
        } else if live.last_frame_active {
            "hearing"
        } else if elapsed_ms > step * ENROLLMENT_WAKE_STEP_SECONDS as usize * 1_000 + 1_000
            && live.step_speech_ms[step] < 200
        {
            "too_quiet"
        } else {
            "waiting"
        }
    }

    /// Update the enrollment wizard with real signal evidence while the actor keeps
    /// buffering one continuous BLE session. This is deliberately lightweight: it
    /// analyzes each 100 ms frame once and never runs speaker/KWS inference on the
    /// streaming actor.
    pub fn observe_enrollment_capture(pcm: &[u8]) {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1_000;
        let mut state = STATE.lock();
        if state.capture != Some(CaptureState::Capturing) {
            return;
        }
        let mut offset = state.enrollment_live.analyzed_bytes;
        while offset.saturating_add(frame_bytes) <= pcm.len() {
            let frame = &pcm[offset..offset + frame_bytes];
            let rms = frame_rms(frame, frame_bytes)
                .first()
                .copied()
                .unwrap_or(0.0);
            let peak = frame
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
                .max()
                .unwrap_or(0);
            let step = enrollment_step_for_elapsed_ms(offset / 32);
            let active = rms >= 60.0;
            if active {
                state.enrollment_live.step_speech_ms[step] = state.enrollment_live.step_speech_ms
                    [step]
                    .saturating_add(ENROLLMENT_FRAME_MS as u16);
            }
            state.enrollment_live.signal_level = ((rms / 12.0).round() as u16).min(100) as u8;
            state.enrollment_live.last_frame_active = active;
            state.enrollment_live.last_frame_clipped = peak >= 32_000;
            offset += frame_bytes;
        }
        state.enrollment_live.analyzed_bytes = offset;
    }

    fn active_speech_bounds(
        pcm: &[u8],
        frame_bytes: usize,
        min_peak_rms: f32,
    ) -> Result<(usize, usize, usize, f32, f32, f32), String> {
        let frame_rms = frame_rms(pcm, frame_bytes);
        if frame_rms.is_empty() {
            return Err("voiceprint audio is empty".to_string());
        }
        let peak_rms = frame_rms.iter().copied().fold(0.0f32, f32::max);
        if peak_rms < min_peak_rms {
            return Err("voiceprint audio contains no clear speech".to_string());
        }
        let mut sorted_rms = frame_rms.clone();
        sorted_rms.sort_by(f32::total_cmp);
        let reference_index = (sorted_rms.len() * 85 / 100).min(sorted_rms.len() - 1);
        let reference_rms = sorted_rms[reference_index];
        let active_threshold = (reference_rms * 0.25).max(16.0);
        let active = frame_rms
            .iter()
            .enumerate()
            .filter_map(|(index, rms)| (*rms >= active_threshold).then_some(index))
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Err("voiceprint audio contains no active speech".to_string());
        }
        Ok((
            active[0],
            active[active.len() - 1],
            active.len(),
            peak_rms,
            reference_rms,
            active_threshold,
        ))
    }

    fn enrollment_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let min_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_MIN_SECONDS;
        if pcm.len() < min_bytes {
            return Err("声纹录制过短，请用自然语速连续说三遍唤醒词。".to_string());
        }

        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, active_frames, peak_rms, reference_rms, active_threshold) =
            active_speech_bounds(pcm, frame_bytes, 60.0)
                .map_err(|_| "没有检测到清晰人声，请靠近设备并连续说三遍唤醒词。")?;
        log::info!(
            "[speaker-verification] enrollment audio quality frames={} active_frames={} peak_rms={peak_rms:.1} reference_rms={reference_rms:.1} threshold_rms={active_threshold:.1}",
            pcm.len().div_ceil(frame_bytes),
            active_frames
        );
        if active_frames < ENROLLMENT_MIN_ACTIVE_FRAMES {
            return Err("有效人声太短，请用自然语速完整说三遍唤醒词。".to_string());
        }

        let first = first_active.saturating_sub(2);
        let last = (last_active + 3).min(pcm.len().div_ceil(frame_bytes));
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        Ok(&pcm[start..end])
    }

    fn evenly_spaced_windows(pcm: &[u8], window_ms: usize, count: usize) -> Vec<&[u8]> {
        let window_bytes = SAMPLE_RATE as usize * 2 * window_ms / 1000;
        if count == 0 || pcm.len() <= window_bytes {
            return vec![pcm];
        }
        let last_start = pcm.len().saturating_sub(window_bytes) & !1usize;
        (0..count)
            .map(|index| {
                let start = if count == 1 {
                    last_start / 2
                } else {
                    (last_start * index / (count - 1)) & !1usize
                };
                &pcm[start..start + window_bytes]
            })
            .collect()
    }

    #[derive(Debug)]
    struct EnrollmentTemplateWindows {
        wake: Vec<Vec<u8>>,
        session: Vec<Vec<u8>>,
    }

    fn enrollment_wake_step_slices(wake_pcm: &[u8]) -> Result<Vec<&[u8]>, String> {
        let step_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_WAKE_STEP_SECONDS as usize;
        let required_bytes = step_bytes * ENROLLMENT_WAKE_STEPS as usize;
        if wake_pcm.len() < required_bytes {
            return Err("三次唤醒词录制不完整，请重新录制。".to_string());
        }
        Ok((0..ENROLLMENT_WAKE_STEPS as usize)
            .map(|index| {
                let start = index * step_bytes;
                &wake_pcm[start..start + step_bytes]
            })
            .collect())
    }

    fn normalized_enrollment_wake_pcm(pcm: &[u8]) -> Result<Cow<'_, [u8]>, String> {
        let required_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_WAKE_SECONDS as usize;
        if pcm.len() >= required_bytes {
            return Ok(Cow::Borrowed(&pcm[..required_bytes]));
        }

        let tolerance_bytes =
            SAMPLE_RATE as usize * 2 * ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS / 1_000;
        let missing_bytes = required_bytes - pcm.len();
        if missing_bytes > tolerance_bytes {
            return Err("声纹录制提前结束，请按提示完整说三次唤醒词。".to_string());
        }

        let mut normalized = Vec::with_capacity(required_bytes);
        normalized.extend_from_slice(pcm);
        normalized.resize(required_bytes, 0);
        log::info!(
            "[speaker-verification] tolerated bounded enrollment transport tail missing_bytes={} missing_ms={}",
            missing_bytes,
            missing_bytes / 32
        );
        Ok(Cow::Owned(normalized))
    }

    fn quality_wake_step_window(pcm: &[u8], step: usize) -> Result<Vec<u8>, String> {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let compacted = compact_active_speech_frames(pcm, frame_bytes)
            .map_err(|_| format!("第 {step} 次没有检测到清晰唤醒词，请靠近设备重新录制。"))?;
        let active_frames = compacted.len() / frame_bytes;
        if active_frames < ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP {
            return Err(format!(
                "第 {step} 次唤醒词太短，请自然、完整地说出唤醒词。"
            ));
        }

        let window_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_TEMPLATE_WINDOW_MS / 1000;
        let mut window = Vec::with_capacity(window_bytes);
        while window.len() < window_bytes {
            let remaining = window_bytes - window.len();
            window.extend_from_slice(&compacted[..compacted.len().min(remaining)]);
        }
        Ok(window)
    }

    fn compact_active_speech_frames(pcm: &[u8], frame_bytes: usize) -> Result<Vec<u8>, String> {
        let (_, _, _, _, _, active_threshold) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let rms = frame_rms(pcm, frame_bytes);
        let mut compacted = Vec::with_capacity(pcm.len());
        for (index, frame) in pcm.chunks(frame_bytes).enumerate() {
            if rms
                .get(index)
                .is_some_and(|value| *value >= active_threshold)
            {
                compacted.extend_from_slice(frame);
            }
        }
        Ok(compacted)
    }

    /// Preserve continuous source speech for session identity and activity.
    fn quality_session_step_span(pcm: &[u8], step: usize) -> Result<Vec<u8>, String> {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first, last, active_frames, _, _, _) = active_speech_bounds(pcm, frame_bytes, 20.0)
            .map_err(|_| format!("第 {step} 次没有检测到清晰唤醒词，请靠近设备重新录制。"))?;
        if active_frames < ENROLLMENT_WAKE_MIN_ACTIVE_FRAMES_PER_STEP {
            return Err(format!(
                "第 {step} 次唤醒词太短，请自然、完整地说出唤醒词。"
            ));
        }
        let start = first.saturating_sub(1) * frame_bytes;
        let end = ((last + 2) * frame_bytes).min(pcm.len()) & !1usize;
        // A valid guided phrase may contain only 600 ms of active speech, while
        // the identity encoder accepts at least 1 s of source audio. Extend into
        // the original slot's surrounding context, never repeat the phrase to
        // satisfy that encoder contract or discard any part of the speech span.
        let minimum_bytes = SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000;
        let span_bytes = (end - start).max(minimum_bytes);
        let start = (start.saturating_sub((span_bytes - (end - start)) / 2) & !1usize)
            .min((pcm.len() & !1usize).saturating_sub(span_bytes));
        Ok(pcm[start..start + span_bytes].to_vec())
    }

    fn enrollment_template_windows(pcm: &[u8]) -> Result<EnrollmentTemplateWindows, String> {
        enrollment_speech_window(pcm)?;
        let wake_pcm = normalized_enrollment_wake_pcm(pcm)?;
        let wake = enrollment_wake_step_slices(wake_pcm.as_ref())?
            .into_iter()
            .enumerate()
            .map(|(index, step_pcm)| quality_wake_step_window(step_pcm, index + 1))
            .collect::<Result<Vec<_>, _>>()?;
        // Preserve the existing wake exemplars. Session encoders own their input
        // separately: the same three quality-gated recordings, including internal
        // low-energy speech and the complete tail, without synthetic repetition.
        // The visible per-step speech minimum is identical for both banks.
        let session = enrollment_wake_step_slices(wake_pcm.as_ref())?
            .into_iter()
            .enumerate()
            .map(|(index, step_pcm)| quality_session_step_span(step_pcm, index + 1))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EnrollmentTemplateWindows { wake, session })
    }

    fn verification_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let pcm = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let minimum_bytes = SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000;
        if pcm.len() < minimum_bytes {
            return Err(format!(
                "voiceprint audio is shorter than {VERIFICATION_MIN_SPEECH_MS} ms"
            ));
        }

        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, _, _, _, _) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let frame_count = pcm.len().div_ceil(frame_bytes);
        let mut first = first_active.saturating_sub(1);
        let mut last = (last_active + 2).min(frame_count);
        // The final analysis frame may be partial. Counting frames made a nominal
        // ten-frame window as short as 901 ms when speech ended at the live PCM
        // tail, so the embedding layer rejected an otherwise ready owner check
        // and the wake gate waited for the next 2.4 s retry snapshot. Expand by
        // real bytes instead; the caller has already established that the full
        // candidate contains at least `minimum_bytes`.
        while (last * frame_bytes)
            .min(pcm.len())
            .saturating_sub(first * frame_bytes)
            < minimum_bytes
        {
            if first > 0 {
                first -= 1;
            } else if last < frame_count {
                last += 1;
            } else {
                break;
            }
        }
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        Ok(&pcm[start..end])
    }

    fn session_speaker_speech_window(pcm: &[u8]) -> Result<&[u8], String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let pcm = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1000;
        let (first_active, last_active, _, _, _, _) = active_speech_bounds(pcm, frame_bytes, 20.0)?;
        let frame_count = pcm.len().div_ceil(frame_bytes);
        let first = first_active.saturating_sub(1);
        let last = (last_active + 2).min(frame_count);
        let start = first * frame_bytes;
        let end = (last * frame_bytes).min(pcm.len()) & !1usize;
        let speech = &pcm[start..end];
        if speech.is_empty() {
            return Err("voiceprint audio contains no active speech".to_string());
        }
        Ok(speech)
    }

    fn session_speaker_signal_metrics(pcm: &[u8]) -> Result<(usize, f32, f32), String> {
        let frame_bytes = SAMPLE_RATE as usize * 2 * ENROLLMENT_FRAME_MS / 1_000;
        let (_, _, active_frames, peak_rms, reference_rms, _) =
            active_speech_bounds(pcm, frame_bytes, 20.0)?;
        Ok((active_frames * ENROLLMENT_FRAME_MS, peak_rms, reference_rms))
    }

    fn verification_template_windows(pcm: &[u8]) -> Result<Vec<&[u8]>, String> {
        let max_bytes = DEFAULT_MAX_CANDIDATE_MS as usize * 32;
        let raw = &pcm[..pcm.len().min(max_bytes) & !1usize];
        let speech = verification_speech_window(pcm)?;
        let mut windows = vec![raw];
        if speech.as_ptr() != raw.as_ptr() || speech.len() != raw.len() {
            windows.push(speech);
        }
        if speech.len() > SAMPLE_RATE as usize * 2 * TEMPLATE_WINDOW_MS / 1000 {
            windows.extend(evenly_spaced_windows(speech, TEMPLATE_WINDOW_MS, 2));
        }
        Ok(windows)
    }

    fn pad_session_speaker_pcm(pcm: &[u8]) -> Vec<u8> {
        let minimum_bytes = SAMPLE_RATE as usize * 2 * VERIFICATION_MIN_SPEECH_MS / 1000;
        if pcm.len() >= minimum_bytes || pcm.is_empty() {
            return pcm.to_vec();
        }
        let mut padded = Vec::with_capacity(minimum_bytes);
        while padded.len() < minimum_bytes {
            let remaining = minimum_bytes - padded.len();
            padded.extend_from_slice(&pcm[..pcm.len().min(remaining)]);
        }
        padded
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let file = fs::File::open(path).map_err(|err| format!("read download failed: {err}"))?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|err| format!("hash download failed: {err}"))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("{:X}", hasher.finalize()))
    }

    fn download_verified(url: &str, destination: &Path, expected: &str) -> Result<(), String> {
        if destination.exists() && sha256(destination)? == expected {
            return Ok(());
        }
        let temp = destination.with_extension("download");
        let response = reqwest::blocking::get(url)
            .map_err(|err| format!("download voiceprint component failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("voiceprint download returned an error: {err}"))?;
        let bytes = response
            .bytes()
            .map_err(|err| format!("read voiceprint download failed: {err}"))?;
        fs::write(&temp, &bytes)
            .map_err(|err| format!("write voiceprint component failed: {err}"))?;
        let actual = sha256(&temp)?;
        if actual != expected {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "voiceprint component hash mismatch: expected={expected} actual={actual}"
            ));
        }
        fs::rename(&temp, destination)
            .map_err(|err| format!("install voiceprint component failed: {err}"))
    }

    fn extract_runtime(root: &Path, archive_path: &Path) -> Result<(), String> {
        let required = [
            "onnxruntime.dll",
            "onnxruntime_providers_shared.dll",
            "sherpa-onnx-c-api.dll",
        ];
        if required.iter().all(|name| root.join(name).exists()) {
            return Ok(());
        }
        let file =
            fs::File::open(archive_path).map_err(|err| format!("open runtime failed: {err}"))?;
        let decoder = bzip2::read::BzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive
            .entries()
            .map_err(|err| format!("read runtime archive failed: {err}"))?
        {
            let mut entry = entry.map_err(|err| format!("read runtime entry failed: {err}"))?;
            let path = entry
                .path()
                .map_err(|err| format!("read runtime path failed: {err}"))?;
            let Some(name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if required.contains(&name.as_str())
                && path.components().any(|part| part.as_os_str() == "lib")
            {
                entry
                    .unpack(root.join(&name))
                    .map_err(|err| format!("install runtime {name} failed: {err}"))?;
            }
        }
        if required.iter().all(|name| root.join(name).exists()) {
            Ok(())
        } else {
            Err("voiceprint runtime archive is missing required DLLs".to_string())
        }
    }

    fn ensure_runtime_assets() -> Result<std::path::PathBuf, String> {
        let root = crate::persistence::speaker_verification_root()
            .map_err(|err| format!("create voiceprint model directory failed: {err}"))?;
        let runtime_ready = [
            "onnxruntime.dll",
            "onnxruntime_providers_shared.dll",
            "sherpa-onnx-c-api.dll",
        ]
        .iter()
        .all(|name| root.join(name).exists());
        if runtime_ready {
            return Ok(root);
        }
        let archive_path = root.join(format!("sherpa-onnx-{RUNTIME_VERSION}.tar.bz2"));
        download_verified(RUNTIME_URL, &archive_path, RUNTIME_ARCHIVE_SHA256)?;
        extract_runtime(&root, &archive_path)?;
        Ok(root)
    }

    fn ensure_runtime() -> Result<Arc<SpeakerRuntime>, String> {
        if let Some(runtime) = STATE.lock().runtime.clone() {
            return Ok(runtime);
        }
        let root = ensure_runtime_assets()?;
        download_verified(MODEL_URL, &root.join(MODEL_NAME), MODEL_SHA256)?;
        let runtime = Arc::new(SpeakerRuntime::load(&root)?);
        STATE.lock().runtime = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    fn keyring_entry_for(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|err| format!("open system voiceprint credential failed: {err}"))
    }

    fn keyring_entry() -> Result<keyring::Entry, String> {
        keyring_entry_for(KEYRING_ACCOUNT)
    }

    fn supplemental_keyring_entry(index: usize) -> Result<keyring::Entry, String> {
        keyring_entry_for(&format!("{KEYRING_SUPPLEMENTAL_ACCOUNT_PREFIX}{index}"))
    }

    struct VoiceProfileStore;
    impl crate::protected_voice_profile::CredentialStore for VoiceProfileStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            match keyring_entry_for(account)?.get_secret() {
                Ok(value) => crate::protected_voice_profile::decode_windows_credential(&value).map(Some),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(format!("read protected voice profile failed: {error}")),
            }
        }
        fn set(&self, account: &str, value: &str) -> Result<(), String> {
            let blob = crate::protected_voice_profile::encode_windows_credential(value)?;
            keyring_entry_for(account)?.set_secret(&blob).map_err(|error| format!("save protected voice profile failed: {error}"))
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            match keyring_entry_for(account)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(format!("delete protected voice profile failed: {error}")),
            }
        }
    }

    fn delete_stored_credentials() -> Result<(), String> {
        crate::protected_voice_profile::delete(&VoiceProfileStore)?;
        match keyring_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(err) => {
                return Err(format!("delete system voiceprint credential failed: {err}"));
            }
        }
        for index in 0..MAX_SUPPLEMENTAL_TEMPLATES {
            match supplemental_keyring_entry(index)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => {
                    return Err(format!(
                        "delete supplemental system voiceprint credential failed: {err}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn encode_template(
        embedding: &[f32],
        phrase: &str,
        invalidated: bool,
        purpose: TemplatePurpose,
    ) -> Result<String, String> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for value in embedding {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let model_sha256 = match purpose {
            TemplatePurpose::WakePhrase | TemplatePurpose::FreeSpeech => MODEL_SHA256,
            TemplatePurpose::TargetSpeaker => TARGET_SPEAKER_MODEL_SHA256,
            TemplatePurpose::PersonalActivity => crate::personal_vad::ENCODER_SHA,
        };
        serde_json::to_string(&StoredTemplate {
            version: 5,
            model_sha256: model_sha256.to_string(),
            dimension: embedding.len(),
            embedding_base64: BASE64.encode(bytes),
            phrase: Some(phrase.to_string()),
            invalidated,
            purpose: Some(purpose),
            frontend_id: (purpose == TemplatePurpose::PersonalActivity).then(|| crate::speaker_identity_eres::FRONTEND_ID.to_string()),
        })
        .map_err(|err| format!("encode voiceprint template failed: {err}"))
    }

    fn decode_template(value: &str) -> Result<SpeakerTemplate, String> {
        let stored: StoredTemplate = serde_json::from_str(value)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if !matches!(stored.version, 1 | 2 | 3 | 4 | 5) {
            return Err("voiceprint template is incompatible with the current model".to_string());
        }
        let expected_model_sha256 = match stored.purpose {
            Some(TemplatePurpose::TargetSpeaker) => TARGET_SPEAKER_MODEL_SHA256,
            Some(TemplatePurpose::PersonalActivity) => crate::personal_vad::ENCODER_SHA,
            _ => MODEL_SHA256,
        };
        if stored.model_sha256 != expected_model_sha256 {
            return Err("voiceprint template is incompatible with the current model".to_string());
        }
        if stored.version >= 2
            && stored
                .phrase
                .as_ref()
                .is_none_or(|phrase| phrase.trim().is_empty())
        {
            return Err("voiceprint template wake phrase is invalid".to_string());
        }
        let bytes = BASE64
            .decode(stored.embedding_base64)
            .map_err(|err| format!("voiceprint template damaged: {err}"))?;
        if bytes.len() != stored.dimension * 4 || stored.dimension == 0 {
            return Err("voiceprint template dimension is invalid".to_string());
        }
        let mut embedding = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        if embedding.iter().any(|value| !value.is_finite()) {
            return Err("voiceprint template contains a non-finite value".to_string());
        }
        let personal_activity_reference = if stored.purpose == Some(TemplatePurpose::PersonalActivity) {
            Some(crate::personal_vad::PersonalVadReference::from_bound_values(
                &stored.model_sha256, stored.frontend_id.as_deref().unwrap_or_default(), embedding.clone(),
            )?)
        } else { None };
        if stored.purpose == Some(TemplatePurpose::TargetSpeaker) {
            if embedding.len() != TARGET_SPEAKER_EMBEDDING_DIMENSION {
                return Err("target-speaker template dimension is invalid".to_string());
            }
        } else {
            normalize(&mut embedding)?;
        }
        let (wake_embeddings, session_embeddings, target_speaker_embedding) = match stored.purpose {
            Some(TemplatePurpose::WakePhrase) => (vec![embedding], Vec::new(), None),
            Some(TemplatePurpose::FreeSpeech) => (Vec::new(), vec![embedding], None),
            Some(TemplatePurpose::TargetSpeaker) => (Vec::new(), Vec::new(), Some(embedding)),
            Some(TemplatePurpose::PersonalActivity) => (Vec::new(), Vec::new(), None),
            None => {
                // v1/v2 had one undifferentiated bank. Preserve compatibility
                // until the owner re-enrolls with the dual-template prompt.
                (vec![embedding.clone()], vec![embedding], None)
            }
        };
        Ok(SpeakerTemplate {
            wake_embeddings,
            session_embeddings,
            target_speaker_embedding,
            personal_activity_reference,
            phrase: stored.phrase,
            invalidated: stored.invalidated,
        })
    }

    fn encode_profile_entries(template: &SpeakerTemplate) -> Result<Vec<String>, String> {
        let phrase = template
            .phrase
            .as_deref()
            .ok_or_else(|| "voiceprint template wake phrase is missing".to_string())?;
        if template.wake_embeddings.is_empty() || template.session_embeddings.is_empty() {
            return Err("voiceprint template requires wake and session banks".to_string());
        }
        #[cfg(feature = "target-speaker-extraction")]
        if template.target_speaker_embedding.is_none() {
            return Err("voiceprint template requires a target-speaker bank".to_string());
        }
        let encoded = template
            .wake_embeddings
            .iter()
            .map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::WakePhrase,
                )
            })
            .chain(template.session_embeddings.iter().map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::FreeSpeech,
                )
            }))
            .chain(template.target_speaker_embedding.iter().map(|embedding| {
                encode_template(
                    embedding,
                    phrase,
                    template.invalidated,
                    TemplatePurpose::TargetSpeaker,
                )
            }))
            .chain(template.personal_activity_reference.iter().map(|reference| {
                encode_template(reference.values(), phrase, template.invalidated, TemplatePurpose::PersonalActivity)
            }))
            .collect::<Result<Vec<_>, _>>()?;
        if encoded.len() > MAX_SUPPLEMENTAL_TEMPLATES + 1 {
            return Err("voiceprint template contains too many protected entries".to_string());
        }
        // Check the actual binary representation before staging any entry.
        for value in &encoded {
            crate::protected_voice_profile::encode_windows_credential(value)?;
        }
        Ok(encoded)
    }

    fn persist_template(template: &SpeakerTemplate) -> Result<(), String> {
        let encoded = encode_profile_entries(template)?;
        // One commit publishes every model bank. A failed write retains the
        // old generation; a save retry retains only derived features in RAM.
        crate::protected_voice_profile::save(&VoiceProfileStore, &encoded)
    }

    fn current_processing_generation(state: &State, phrase: &str, generation: u64) -> bool {
        state.enrollment_generation == generation
            && state.capture == Some(CaptureState::Processing)
            && state.enrollment_phrase.as_deref() == Some(phrase)
    }

    fn decode_committed_profile(values: &[String]) -> Result<SpeakerTemplate, String> {
        let mut entries = values.iter();
        let mut template = decode_template(entries.next().ok_or("protected voice profile is empty")?)?;
        for value in entries {
            let extra = decode_template(value)?;
            if extra.phrase != template.phrase || extra.invalidated != template.invalidated {
                return Err("protected voice profile generation has mixed phrase bindings".into());
            }
            template.wake_embeddings.extend(extra.wake_embeddings);
            template.session_embeddings.extend(extra.session_embeddings);
            if let Some(reference) = extra.personal_activity_reference {
                if template.personal_activity_reference.replace(reference).is_some() {
                    return Err("protected voice profile has duplicate personal activity references".into());
                }
            }
            if let Some(reference) = extra.target_speaker_embedding {
                if template.target_speaker_embedding.replace(reference).is_some() {
                    return Err("protected voice profile has duplicate target speaker references".into());
                }
            }
        }
        if template.wake_embeddings.is_empty() || template.session_embeddings.is_empty() {
            return Err("protected voice profile is missing its wake or session bank".into());
        }
        Ok(template)
    }

    fn load_template_locked(state: &mut State) {
        if state.template_checked {
            return;
        }
        state.template_checked = true;
        match crate::protected_voice_profile::load(&VoiceProfileStore) {
            Ok(Some(values)) => {
                let result = decode_committed_profile(&values);
                match result {
                    Ok(template) => { state.template = Some(template); state.profile_state = VoiceProfileState::Ready; },
                    Err(error) => { state.profile_state = VoiceProfileState::Corrupt; state.error = Some(error); },
                }
                return;
            }
            Err(error) => {
                state.profile_state = VoiceProfileState::CredentialError;
                state.error = Some(error);
                return;
            }
            Ok(None) => {}
        }
        let result = keyring_entry().and_then(|entry| match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(format!("read system voiceprint credential failed: {err}")),
        });
        match result {
            Ok(Some(value)) => match decode_template(&value) {
                Ok(mut template) => {
                    for index in 0..MAX_SUPPLEMENTAL_TEMPLATES {
                        let supplemental = supplemental_keyring_entry(index).and_then(|entry| {
                            match entry.get_password() {
                                Ok(value) => Ok(Some(value)),
                                Err(keyring::Error::NoEntry) => Ok(None),
                                Err(err) => Err(format!(
                                    "read supplemental voiceprint credential failed: {err}"
                                )),
                            }
                        });
                        match supplemental {
                            Ok(Some(value)) => match decode_template(&value) {
                                Ok(extra)
                                    if extra.phrase == template.phrase
                                        && extra.invalidated == template.invalidated =>
                                {
                                    template.wake_embeddings.extend(extra.wake_embeddings);
                                    template.session_embeddings.extend(extra.session_embeddings);
                                    if let Some(target) = extra.target_speaker_embedding {
                                        if template.target_speaker_embedding.is_none() {
                                            template.target_speaker_embedding = Some(target);
                                        } else {
                                            log::warn!(
                                                "[speaker-verification] ignored duplicate target-speaker template index={index}"
                                            );
                                        }
                                    }
                                }
                                Ok(_) => log::warn!(
                                    "[speaker-verification] ignored supplemental owner template index={index}: wake phrase binding differs"
                                ),
                                Err(err) => {
                                    log::warn!(
                                        "[speaker-verification] ignored supplemental owner template index={index}: {err}"
                                    );
                                }
                            },
                            Ok(None) => {}
                            Err(err) => log::warn!(
                                "[speaker-verification] supplemental owner template unavailable index={index}: {err}"
                            ),
                        }
                    }
                    state.template = Some(template);
                    state.profile_state = VoiceProfileState::Ready;
                }
                Err(err) => {
                    state.profile_state = if err.contains("incompatible") {
                        VoiceProfileState::Incompatible
                    } else {
                        VoiceProfileState::Corrupt
                    };
                    state.error = Some(err);
                }
            },
            Ok(None) => state.profile_state = VoiceProfileState::Unenrolled,
            Err(err) => {
                state.profile_state = VoiceProfileState::CredentialError;
                state.error = Some(err);
            }
        }
    }

    fn bind_legacy_template_locked(state: &mut State, phrase: &str) {
        let Some(template) = state.template.as_mut() else {
            return;
        };
        if template.phrase.is_some() {
            return;
        }
        template.phrase = Some(phrase.to_string());
        if let Err(err) = persist_template(template) {
            template.phrase = None;
            state.profile_state = VoiceProfileState::CredentialError;
            state.error = Some(format!("绑定旧声纹到当前唤醒词失败: {err}"));
            return;
        }
        log::info!(
            "[speaker-verification] bound legacy owner template to current wake phrase={phrase}"
        );
    }

    fn load_template_for_phrase_locked(state: &mut State, phrase: &str) {
        load_template_locked(state);
        bind_legacy_template_locked(state, phrase);
    }

    fn template_matches_phrase(template: &SpeakerTemplate, phrase: &str) -> bool {
        !template.invalidated && template.phrase.as_deref() == Some(phrase)
    }

    fn assets_ready() -> (bool, bool) {
        let Ok(root) = crate::persistence::speaker_verification_root() else {
            return (false, false);
        };
        (
            root.join("sherpa-onnx-c-api.dll").exists() && root.join("onnxruntime.dll").exists(),
            root.join(MODEL_NAME).exists(),
        )
    }

    pub fn status_for_phrase(wake_phrase: &str) -> VoiceprintStatus {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)
            .unwrap_or_else(|_| wake_phrase.trim().to_string());
        let (runtime_ready, model_ready) = assets_ready();
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        let enrolled_phrase = state
            .template
            .as_ref()
            .and_then(|template| template.phrase.clone());
        let enrolled = state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase));
        let requires_reenrollment = state.template.as_ref().is_some_and(|template| {
            !template_matches_phrase(template, &phrase)
                || (cfg!(feature = "target-speaker-extraction")
                    && template.target_speaker_embedding.is_none())
        });
        let capture_seconds_remaining = if state.capture == Some(CaptureState::Capturing) {
            state.enrollment_capture_started.map(|started| {
                ENROLLMENT_SECONDS
                    .saturating_sub(started.elapsed().as_secs())
                    .min(u8::MAX as u64) as u8
            })
        } else {
            None
        };
        let capture_elapsed_ms =
            (state.enrollment_live.analyzed_bytes / 32).min(u32::MAX as usize) as u32;
        let capture_step_index = enrollment_step_for_elapsed_ms(capture_elapsed_ms as usize);
        let capture_step = matches!(
            state.capture,
            Some(CaptureState::Capturing | CaptureState::Processing)
        )
        .then_some((capture_step_index + 1) as u8);
        let capture_feedback = (state.capture == Some(CaptureState::Capturing)).then(|| {
            enrollment_capture_feedback(
                capture_step_index,
                capture_elapsed_ms as usize,
                &state.enrollment_live,
            )
            .to_string()
        });
        let progress = capture_seconds_remaining
            .map(|remaining| {
                let elapsed = ENROLLMENT_SECONDS.saturating_sub(remaining as u64);
                20u64.saturating_add(elapsed.saturating_mul(50) / ENROLLMENT_SECONDS) as u8
            })
            .unwrap_or(state.progress);
        VoiceprintStatus {
            available: true,
            runtime_ready,
            model_ready,
            enrolled,
            enrolled_phrase,
            requires_reenrollment,
            profile_state: state.profile_state,
            state: state
                .capture
                .unwrap_or(CaptureState::Idle)
                .label()
                .to_string(),
            progress,
            capture_seconds_remaining,
            capture_step,
            capture_step_count: ENROLLMENT_STEP_COUNT as u8,
            capture_elapsed_ms,
            step_speech_ms: state.enrollment_live.step_speech_ms.to_vec(),
            signal_level: state.enrollment_live.signal_level,
            capture_feedback,
            score: state.last_score,
            threshold: VERIFICATION_THRESHOLD,
            error: state.error.clone(),
            failure_stage: state.failure_stage,
            can_retry_save: state.capture == Some(CaptureState::Error)
                && state.failure_stage == Some(EnrollmentFailureStage::Storage)
                && state.pending_template.as_ref().is_some_and(|template| template_matches_phrase(template, &phrase)),
            model_name: MODEL_NAME,
            runtime_version: RUNTIME_VERSION,
            local_only: true,
        }
    }

    pub fn status() -> VoiceprintStatus {
        status_for_phrase("开始录音")
    }

    #[cfg(test)]
    pub(crate) fn has_bound_template_without_mutation(wake_phrase: &str) -> bool {
        let Ok(phrase) = crate::wake_phrase::normalize_configured_phrase(wake_phrase) else {return false;};
        let mut state=STATE.lock();
        load_template_locked(&mut state);
        state.template.as_ref().is_some_and(|template| template_matches_phrase(template,&phrase))
    }

    pub fn is_enrolled_for_phrase(wake_phrase: &str) -> bool {
        let Ok(phrase) = crate::wake_phrase::normalize_configured_phrase(wake_phrase) else {
            return false;
        };
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase))
    }

    pub fn prepare_for_phrase(wake_phrase: &str) -> Result<(), String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let started = std::time::Instant::now();
        ensure_runtime()?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        if !state
            .template
            .as_ref()
            .is_some_and(|template| template_matches_phrase(template, &phrase))
        {
            return Err("voiceprint is not enrolled".to_string());
        }
        log::info!(
            "[speaker-verification] owner gate prepared phrase={} elapsed_ms={}",
            phrase,
            started.elapsed().as_millis()
        );
        Ok(())
    }

    pub(crate) fn prepare_runtime_assets() -> Result<(), String> {
        ensure_runtime_assets().map(|_| ())
    }

    pub fn start_enrollment(wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let wake_phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        log::info!(
            "[speaker-verification] enrollment requested for configured wake phrase={wake_phrase}"
        );
        {
            let mut state = STATE.lock();
            if matches!(
                state.capture,
                Some(CaptureState::Preparing | CaptureState::Armed | CaptureState::Capturing | CaptureState::Processing)
            ) {
                return Err("voiceprint enrollment is already active".to_string());
            }
            state.capture = Some(CaptureState::Preparing);
            state.progress = 5;
            state.error = None;
            state.failure_stage = None;
            state.pending_template = None;
            state.enrollment_generation = state.enrollment_generation.wrapping_add(1);
            state.last_score = None;
            state.enrollment_phrase = Some(wake_phrase.clone());
            state.enrollment_capture_started = None;
            state.enrollment_live = EnrollmentLiveState::default();
        }
        if let Err(err) = ensure_runtime() {
            mark_error(&err, EnrollmentFailureStage::Preparation);
            return Err(err);
        }
        {
            let mut state = STATE.lock();
            state.capture = Some(CaptureState::Armed);
            state.progress = 20;
        }
        if let Err(err) =
            crate::embedded_ble::send_recording_control_enrollment(Duration::from_secs(4))
        {
            mark_error(&err, EnrollmentFailureStage::Device);
            return Err(err);
        }
        Ok(status_for_phrase(&wake_phrase))
    }

    fn enrollment_capture_needs_host_stop(capture: Option<CaptureState>) -> bool {
        matches!(capture, Some(CaptureState::Armed | CaptureState::Capturing))
    }

    pub fn take_enrollment_arm() -> bool {
        let armed = {
            let mut state = STATE.lock();
            if state.capture == Some(CaptureState::Armed) {
                state.capture = Some(CaptureState::Capturing);
                state.progress = 35;
                state.enrollment_capture_started = Some(std::time::Instant::now());
                Some(state.enrollment_generation)
            } else {
                None
            }
        };
        if let Some(generation) = armed {
            // The capture clock starts only after the dedicated device session
            // actually arrives. BLE recovery or hidden-candidate preemption in
            // the Preparing/Armed phase must not shorten the owner sample.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(ENROLLMENT_SECONDS));
                let capture_still_active = {
                    let state = STATE.lock();
                    state.enrollment_generation == generation
                        && enrollment_capture_needs_host_stop(state.capture)
                };
                if !capture_still_active {
                    log::info!(
                        "[speaker-verification] enrollment stop timer skipped because device session already completed"
                    );
                    return;
                }
                if let Err(err) =
                    crate::embedded_ble::send_recording_control_stop(Duration::from_secs(4))
                {
                    mark_error(&format!("stop voiceprint enrollment failed: {err}"), EnrollmentFailureStage::Device);
                }
            });
        }
        armed.is_some()
    }

    pub fn begin_enrollment_processing() -> Option<u64> {
        let mut state = STATE.lock();
        if matches!(
            state.capture,
            Some(CaptureState::Armed | CaptureState::Capturing)
        ) {
            state.capture = Some(CaptureState::Processing);
            state.progress = 70;
            state.enrollment_capture_started = None;
            return Some(state.enrollment_generation);
        }
        None
    }

    fn publish_pending_template(
        state: &mut State,
        save: impl FnOnce(&SpeakerTemplate) -> Result<(), String>,
    ) -> Result<(), String> {
        let template = state.pending_template.as_ref()
            .ok_or_else(|| "没有可保存的声纹，请重新录入。".to_string())?;
        if let Err(error) = save(template) {
            state.capture = Some(CaptureState::Error);
            state.error = Some(error.clone());
            state.failure_stage = Some(EnrollmentFailureStage::Storage);
            state.enrollment_capture_started = None;
            // Keep derived features only, so a vault failure can be retried
            // without recapturing speech or replacing the active owner bank.
            return Err(error);
        }
        state.template = state.pending_template.take();
        state.template_checked = true;
        state.profile_state = VoiceProfileState::Ready;
        state.capture = Some(CaptureState::Complete);
        state.progress = 100;
        state.last_score = None;
        state.error = None;
        state.failure_stage = None;
        state.enrollment_phrase = None;
        state.enrollment_capture_started = None;
        Ok(())
    }

    pub fn retry_enrollment_save(wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        if state.capture != Some(CaptureState::Error)
            || state.failure_stage != Some(EnrollmentFailureStage::Storage)
            || !state.pending_template.as_ref().is_some_and(|template| template_matches_phrase(template, &phrase))
        {
            return Err("没有与当前唤醒词对应的待保存声纹。".into());
        }
        state.capture = Some(CaptureState::Processing);
        state.progress = 95;
        let result = publish_pending_template(&mut state, persist_template);
        drop(state);
        result?;
        log::info!("[speaker-verification] enrollment saved from retained features without new audio");
        Ok(status_for_phrase(&phrase))
    }

    pub fn finish_enrollment(pcm: &[u8], wake_phrase: &str, generation: u64) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        {
            let state = STATE.lock();
            if !current_processing_generation(&state, &phrase, generation) {
                return Err("这次声纹录制已取消或唤醒词已改变。".to_string());
            }
        }
        let features_error = |message| (EnrollmentFailureStage::Features, message);
        let capture_error = |message| (EnrollmentFailureStage::Capture, message);
        let result: Result<SpeakerTemplate, (EnrollmentFailureStage, String)> = (|| {
            let runtime = ensure_runtime().map_err(features_error)?;
            let windows = enrollment_template_windows(pcm).map_err(capture_error)?;
            // Reject silence/short speech before invoking KWS, then validate the
            // phrase before persisting either template bank.
            let wake_pcm = normalized_enrollment_wake_pcm(pcm).map_err(capture_error)?;
            for (index, step_pcm) in enrollment_wake_step_slices(wake_pcm.as_ref()).map_err(capture_error)?
                .iter()
                .enumerate()
            {
                crate::wake_phrase::calibrate(step_pcm, &phrase).map_err(|_| {
                    (EnrollmentFailureStage::Phrase, format!(
                        "第 {} 次没有识别到“{}”，请按每一步提示清晰重录。",
                        index + 1,
                        phrase
                    ))
                })?;
            }
            let wake_embeddings = windows
                .wake
                .iter()
                .map(|speech| {
                    runtime
                        .embedding(speech)
                        .map_err(|err| format!("唤醒词声纹特征提取失败，请重新录制：{err}"))
                })
                .collect::<Result<Vec<_>, _>>().map_err(features_error)?;
            let session_embeddings = windows
                .session
                .iter()
                .map(|speech| {
                    runtime
                        .embedding(speech)
                        .map_err(|err| format!("会话声纹特征提取失败，请重新录制：{err}"))
                })
                .collect::<Result<Vec<_>, _>>().map_err(features_error)?;
            #[cfg(feature = "target-speaker-extraction")]
            let target_speaker_embedding = {
                let target_enrollment_pcm = windows.session.concat();
                crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(
                    &target_enrollment_pcm,
                )
                .map(Some)
                .map_err(|err| features_error(format!("多人隔离声纹特征提取失败：{err}")))?
            };
            #[cfg(not(feature = "target-speaker-extraction"))]
            let target_speaker_embedding = None;
            Ok(SpeakerTemplate {
                wake_embeddings,
                session_embeddings,
                target_speaker_embedding,
                personal_activity_reference: Some(crate::personal_vad::enrollment_reference(&windows.session.concat())
                    .map_err(|error| features_error(format!("本人活动声纹特征提取失败：{error}")))?),
                phrase: Some(phrase.clone()),
                invalidated: false,
            })
        })();
        let mut state = STATE.lock();
        if !current_processing_generation(&state, &phrase, generation) {
            return Err("这次声纹录制已取消或唤醒词已改变。".into());
        }
        let result = match result {
            Ok(template) => {
                state.pending_template = Some(template);
                publish_pending_template(&mut state, persist_template)
            }
            Err((stage, error)) => {
                state.capture = Some(CaptureState::Error);
                state.failure_stage = Some(stage);
                state.error = Some(error.clone());
                Err(error)
            }
        };
        drop(state);
        result?;
        Ok(status_for_phrase(&phrase))
    }

    pub fn verify(pcm: &[u8], wake_phrase: &str) -> Result<VerificationResult, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        // 小爱同学模式：未注册主人声纹 → 不做说话人校验，唤醒词命中即放行（任何人可唤醒）；
        // 注册后才执行声纹匹配（仅主人能唤醒）。
        // 兼带修复旧逻辑的坑：以前未注册时这里返回 Err，导致 gate 把"开始录音"判定为
        // voiceprint_verification_failed 而拒唤醒——没录声纹反而完全唤醒不了。
        // 未注册时直接短路返回，避免无谓加载 ONNX runtime/模型（省几百 ms 延迟 + 网络下载）。
        let (template, profile_state, profile_error) = {
            let mut state = STATE.lock();
            load_template_for_phrase_locked(&mut state, &phrase);
            (
                state.template.clone(),
                state.profile_state,
                state.error.clone(),
            )
        };
        let template = match template {
            Some(template) if template_matches_phrase(&template, &phrase) => template,
            None if profile_state == VoiceProfileState::Unenrolled => {
                log::info!(
                    "[speaker-verification] owner not enrolled for phrase={} — open gate (any speaker may wake), pcm_ms={}",
                    phrase,
                    pcm.len() / 32
                );
                return Ok(VerificationResult {
                    matched: true,
                    owner_matched: false,
                    score: 0.0,
                    policy: VerificationPolicy::OpenUnenrolled,
                });
            }
            None => {
                let detail = profile_error.unwrap_or_else(|| {
                    format!("voiceprint profile unavailable: {profile_state:?}")
                });
                log::warn!(
                    "[speaker-verification] owner profile unavailable phrase={} profile_state={profile_state:?} — fail closed: {detail}",
                    phrase
                );
                return Err(detail);
            }
            Some(template) => {
                log::info!(
                    "[speaker-verification] owner template inactive for phrase={} enrolled_phrase={} invalidated={} — open gate until re-enrollment",
                    phrase,
                    template.phrase.as_deref().unwrap_or("-"),
                    template.invalidated
                );
                return Ok(VerificationResult {
                    matched: true,
                    owner_matched: false,
                    score: 0.0,
                    policy: VerificationPolicy::OpenInactiveProfile,
                });
            }
        };
        let runtime = ensure_runtime()?;
        let candidate_windows = verification_template_windows(pcm)?;
        let inference_started = std::time::Instant::now();
        let candidate_embeddings = candidate_windows
            .iter()
            .map(|candidate| runtime.embedding(candidate))
            .collect::<Result<Vec<_>, _>>()?;
        let inference_ms = inference_started.elapsed().as_millis();
        let score = template
            .wake_embeddings
            .iter()
            .flat_map(|enrolled| {
                candidate_embeddings
                    .iter()
                    .filter_map(|candidate| cosine(enrolled, candidate).ok())
            })
            .max_by(f32::total_cmp)
            .ok_or_else(|| "voiceprint dimensions do not match".to_string())?;
        let verdict = if score >= VERIFICATION_THRESHOLD {
            Verdict::Match
        } else {
            Verdict::NonMatch
        };
        let decision = Machine::default().step(Input {
            enabled: true,
            enrolled: true,
            origin: CandidateOrigin::Automatic,
            candidate_complete: true,
            candidate_ms: (pcm.len() / 32) as u32,
            verdict,
        });
        log::info!(
            "[speaker-verification] compared enrolled_templates={} candidate_windows={} speech_ms={} inference_ms={inference_ms} score={score:.6}",
            template.wake_embeddings.len(),
            candidate_embeddings.len(),
            candidate_windows[0].len() / 32
        );
        STATE.lock().last_score = Some(score);
        let matched = matches!(decision.action, Action::Release);
        Ok(VerificationResult {
            matched,
            owner_matched: matched,
            score,
            policy: VerificationPolicy::Enrolled,
        })
    }

    #[cfg(feature = "target-speaker-extraction")]
    pub fn target_speaker_embedding_for_phrase(
        wake_phrase: &str,
    ) -> Result<Option<Vec<f32>>, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        Ok(state
            .template
            .as_ref()
            .filter(|template| template_matches_phrase(template, &phrase))
            .and_then(|template| template.target_speaker_embedding.clone()))
    }

    pub(crate) fn personal_activity_reference_for_phrase(wake_phrase: &str) -> Result<Option<crate::personal_vad::PersonalVadReference>, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        if matches!(state.profile_state, VoiceProfileState::Corrupt | VoiceProfileState::CredentialError | VoiceProfileState::Incompatible) {
            return Err("the protected personal activity profile is unavailable".into());
        }
        Ok(state.template.as_ref().filter(|template| template_matches_phrase(template, &phrase))
            .and_then(|template| template.personal_activity_reference.clone()))
    }

    // ---- 银行自适应 (persisted-bank adaptation) ----

    static ADAPTIVE_BANK_LAST_WRITE: Lazy<Mutex<Option<std::time::Instant>>> =
        Lazy::new(|| Mutex::new(None));
    /// At most one persisted-bank write per interval across both banks. The
    /// represented-similarity skip already stops most writes; this caps
    /// keyring churn even when the voice moves daily.
    const ADAPTIVE_BANK_MIN_WRITE_INTERVAL_MS: u128 = 600_000;

    #[derive(Clone, Copy)]
    enum AdaptiveBankKind {
        Wake,
        Session,
    }

    impl AdaptiveBankKind {
        fn label(self) -> &'static str {
            match self {
                Self::Wake => "wake",
                Self::Session => "session",
            }
        }
    }

    /// Refresh one persisted bank slot with a live exemplar this session
    /// already verified. Slot 0 (the guided-enrollment anchor) is never
    /// replaced; only the churn slot tracks the freshest voice. The caller
    /// must have established owner identity through the bank itself (an
    /// enrolled-match wake, or a >= ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE
    /// window) — open acceptance must never reach this write.
    fn adapt_owner_bank_locked(
        state: &mut State,
        phrase: &str,
        embedding: &[f32],
        kind: AdaptiveBankKind,
    ) -> Result<bool, String> {
        load_template_for_phrase_locked(state, phrase);
        let mut template = state
            .template
            .clone()
            .filter(|template| template_matches_phrase(template, phrase) && !template.invalidated)
            .ok_or_else(|| "voiceprint bank is not enrolled for this phrase".to_string())?;
        let bank = match kind {
            AdaptiveBankKind::Wake => &mut template.wake_embeddings,
            AdaptiveBankKind::Session => &mut template.session_embeddings,
        };
        if !super::adaptive_bank_should_refresh(bank, embedding) {
            return Ok(false);
        }
        bank[super::ADAPTIVE_BANK_CHURN_SLOT] = embedding.to_vec();
        persist_template(&template)?;
        state.template = Some(template);
        Ok(true)
    }

    fn adapt_owner_bank(
        wake_phrase: &str,
        embedding: &[f32],
        kind: AdaptiveBankKind,
    ) -> Result<bool, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        if embedding.is_empty() {
            return Ok(false);
        }
        if ADAPTIVE_BANK_LAST_WRITE.lock().is_some_and(|last| {
            last.elapsed().as_millis() < ADAPTIVE_BANK_MIN_WRITE_INTERVAL_MS
        }) {
            return Ok(false);
        }
        let mut state = STATE.lock();
        let result = adapt_owner_bank_locked(&mut state, &phrase, embedding, kind);
        if matches!(result, Ok(true)) {
            *ADAPTIVE_BANK_LAST_WRITE.lock() = Some(std::time::Instant::now());
        }
        match result {
            Ok(true) => log::info!(
                "[speaker-verification] owner bank adapted kind={} churn_slot={} embedding_dim={} phrase={}",
                kind.label(),
                super::ADAPTIVE_BANK_CHURN_SLOT,
                embedding.len(),
                phrase
            ),
            Ok(false) => {}
            Err(ref err) => log::warn!(
                "[speaker-verification] owner bank adaptation skipped kind={}: {err}",
                kind.label()
            ),
        }
        result
    }

    pub fn adapt_owner_wake_bank(
        wake_phrase: &str,
        verified_wake_embedding: &[f32],
    ) -> Result<bool, String> {
        adapt_owner_bank(wake_phrase, verified_wake_embedding, AdaptiveBankKind::Wake)
    }

    pub fn adapt_owner_session_bank(
        wake_phrase: &str,
        body_embedding: &[f32],
    ) -> Result<bool, String> {
        adapt_owner_bank(wake_phrase, body_embedding, AdaptiveBankKind::Session)
    }

    /// Read the protected session bank without learning from this recording or
    /// treating an open wake as an identity match. Anonymous diarization binds
    /// a channel only after independently comparing its audio with this bank.
    pub(crate) fn enrolled_session_profile(
        wake_phrase: &str,
    ) -> Result<Option<SessionSpeakerProfile>, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        // Diagnostics may read an already-bound bank, but must not trigger the
        // legacy phrase-binding path, which writes protected credentials.
        load_template_locked(&mut state);
        Ok(state.template.as_ref()
            .filter(|template| template_matches_phrase(template, &phrase))
            .map(|template| SessionSpeakerProfile {
                embeddings: Arc::new(template.session_embeddings.clone()),
                adaptive: false,
            }))
    }

    fn native_word_profile_key(state: &State, phrase: &str) -> Option<[u8; 32]> {
        let template = state.template.as_ref().filter(|t| template_matches_phrase(t, phrase) && !t.invalidated)?;
        if template.session_embeddings.is_empty() { return None; }
        let mut hash = Sha256::new();
        hash.update(MODEL_SHA256.as_bytes());
        hash.update(state.enrollment_generation.to_le_bytes());
        hash.update(phrase.as_bytes());
        for vector in &template.session_embeddings {
            hash.update((vector.len() as u64).to_le_bytes());
            for value in vector { hash.update(value.to_bits().to_le_bytes()); }
        }
        Some(hash.finalize().into())
    }

    #[test]
    fn native_word_reference_binding_retires_on_enrollment_phrase_bank_and_deletion() {
        let mut state=State {template:Some(SpeakerTemplate {
            wake_embeddings:vec![vec![1.0,0.0]],session_embeddings:vec![vec![1.0,0.0]],
            target_speaker_embedding:None,personal_activity_reference:None,phrase:Some("开始录音".into()),invalidated:false,
        }),..Default::default()};
        let original=native_word_profile_key(&state,"开始录音").unwrap();
        state.native_word_reference=Some((original,vec![1.0,0.0]));
        assert_eq!(native_word_profile_key(&state,"other phrase"),None);
        state.template.as_mut().unwrap().session_embeddings[0][1]=0.5;
        assert_ne!(native_word_profile_key(&state,"开始录音"),Some(original));
        state.template.as_mut().unwrap().session_embeddings[0][1]=0.0;
        retire_pending_enrollment(&mut state);
        assert!(state.native_word_reference.is_none());
        assert_ne!(native_word_profile_key(&state,"开始录音"),Some(original));
        state.template=None;
        assert_eq!(native_word_profile_key(&state,"开始录音"),None);
    }

    pub(crate) fn native_word_profile_for_phrase(wake_phrase: &str) -> Result<Option<super::NativeWordSpeakerProfile>, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        load_template_locked(&mut state);
        let Some(key) = native_word_profile_key(&state, &phrase) else {
            state.native_word_reference = None;
            return Ok(None);
        };
        if state.native_word_reference.as_ref().is_some_and(|(bound, _)| bound != &key) {
            state.native_word_reference = None;
        }
        let base = SessionSpeakerProfile {
            embeddings: Arc::new(state.template.as_ref().unwrap().session_embeddings.clone()), adaptive: false,
        };
        let mut words = base.clone();
        let has_qualified_reference = state.native_word_reference.as_ref().is_some_and(|(bound, vector)| {
            if bound == &key { Arc::make_mut(&mut words.embeddings).push(vector.clone()); true } else { false }
        });
        Ok(Some(super::NativeWordSpeakerProfile { key, base, words, has_qualified_reference }))
    }

    pub(crate) fn native_word_profile_is_current(wake_phrase: &str, profile: &super::NativeWordSpeakerProfile) -> bool {
        native_word_binding_is_current(wake_phrase, profile.key)
    }

    pub(crate) fn native_word_binding_is_current(wake_phrase: &str, key: [u8; 32]) -> bool {
        let Ok(phrase) = crate::wake_phrase::normalize_configured_phrase(wake_phrase) else { return false; };
        native_word_profile_key(&STATE.lock(), &phrase) == Some(key)
    }

    /// Source purity is established by the native-word caller, before this
    /// independent base-bank admission. A reference match cannot learn again.
    pub(crate) fn acquire_native_word_reference(wake_phrase: &str, profile: &super::NativeWordSpeakerProfile,
        observation: &SessionSpeakerObservation) -> bool {
        if profile.has_qualified_reference || !observation.signal_quality_sufficient
            || !matches!(profile.base.classify_observation(observation), SessionSpeakerClassification::Target { .. }) {
            return false;
        }
        let Ok(phrase) = crate::wake_phrase::normalize_configured_phrase(wake_phrase) else { return false; };
        let mut state = STATE.lock();
        if native_word_profile_key(&state, &phrase) != Some(profile.key) { return false; }
        if state.native_word_reference.as_ref().is_some_and(|(key, _)| key == &profile.key) { return false; }
        state.native_word_reference = Some((profile.key, observation.embedding.clone()));
        true
    }

    pub(crate) fn endpoint_identity_verifier_for_phrase(
        wake_phrase: &str,
    ) -> Result<Option<crate::personal_vad::PresenceVerifier>, String> {
        let Some(profile) = enrolled_session_profile(wake_phrase)? else { return Ok(None); };
        Ok(Some(endpoint_identity_verifier(profile)))
    }

    /// Freeze both model-specific references under the same profile lock. A
    /// concurrent enrollment/delete cannot pair different profile generations.
    pub(crate) fn endpoint_presence_references_for_phrase(
        wake_phrase: &str,
    ) -> Result<(Option<crate::personal_vad::PersonalVadReference>, Option<crate::personal_vad::PresenceVerifier>, Option<(String, [u8; 32])>), String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let mut state = STATE.lock();
        load_template_for_phrase_locked(&mut state, &phrase);
        if matches!(state.profile_state, VoiceProfileState::Corrupt | VoiceProfileState::CredentialError | VoiceProfileState::Incompatible) {
            return Err("the protected endpoint identity profile is unavailable".into());
        }
        let Some(template) = state.template.as_ref().filter(|template| template_matches_phrase(template, &phrase)) else {
            return Ok((None, None, None));
        };
        let reference = template.personal_activity_reference.clone();
        let profile = SessionSpeakerProfile {
            embeddings: Arc::new(template.session_embeddings.clone()),
            adaptive: false,
        };
        let binding = native_word_profile_key(&state, &phrase).map(|key| (phrase, key));
        Ok((reference, (!profile.embeddings.is_empty()).then(|| endpoint_identity_verifier(profile)), binding))
    }

    fn endpoint_identity_verifier(profile: SessionSpeakerProfile) -> crate::personal_vad::PresenceVerifier {
        Box::new(move |pcm| {
            use crate::personal_vad::{PresenceIdentityObservation, PresenceState};
            let observation = observe_session_speaker(&profile, pcm)?;
            Ok(PresenceIdentityObservation {
                projection: match observation.classification {
                    SessionSpeakerClassification::Target { .. } => PresenceState::Owner,
                    SessionSpeakerClassification::NonTarget { .. } if observation.signal_quality_sufficient => PresenceState::ConfirmedOther,
                    _ => PresenceState::Unknown,
                },
                endpoint: observation.endpoint_presence_identity(),
            })
        })
    }

    pub fn session_profile_from_wake(
        pcm: &[u8],
        wake_end_seconds: f32,
        wake_phrase: &str,
        live_wake_matches_enrolled_owner: bool,
    ) -> Result<SessionSpeakerProfile, String> {
        session_profile_from_wake_impl(pcm, wake_end_seconds, wake_phrase, live_wake_matches_enrolled_owner, true)
    }

    #[cfg(test)]
    pub fn session_profile_snapshot_from_wake(
        pcm: &[u8], wake_end_seconds: f32, wake_phrase: &str, owner_verified: bool,
    ) -> Result<SessionSpeakerProfile, String> {
        session_profile_from_wake_impl(pcm, wake_end_seconds, wake_phrase, owner_verified, false)
    }

    fn session_profile_from_wake_impl(
        pcm: &[u8], wake_end_seconds: f32, wake_phrase: &str,
        live_wake_matches_enrolled_owner: bool, persist_adaptation: bool,
    ) -> Result<SessionSpeakerProfile, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let enrolled = {
            let mut state = STATE.lock();
            load_template_for_phrase_locked(&mut state, &phrase);
            state
                .template
                .as_ref()
                .filter(|template| template_matches_phrase(template, &phrase))
                .map(|template| template.session_embeddings.clone())
        };
        if let Some(enrolled) = enrolled {
            let bank_exemplars = enrolled.len();
            match verified_wake_session_embedding(pcm, wake_end_seconds) {
                Ok((verified_wake_embedding, source_speech_ms, model_input_ms, inference_ms)) => {
                    // 银行自适应 wake echo: only a wake the persisted bank
                    // itself matched may refresh the wake bank — an
                    // open-acceptance wake (bank did not match) must never
                    // write, the same contract the 2026-09-17 bank-purifying
                    // hardening established. Rate-limited and
                    // similarity-gated inside; failures never affect this
                    // session's profile.
                    if live_wake_matches_enrolled_owner && persist_adaptation {
                        let _ = adapt_owner_wake_bank(&phrase, &verified_wake_embedding);
                    }
                    let (embeddings, wake_anchor) = super::session_target_with_wake_anchor(
                        enrolled,
                        verified_wake_embedding,
                        live_wake_matches_enrolled_owner,
                    );
                    log::info!(
                        "[speaker-verification] enrolled session target prepared phrase={} wake_end_ms={} speech_ms={} model_input_ms={} inference_ms={inference_ms} persisted_exemplars={bank_exemplars} session_embeddings={} wake_anchor={wake_anchor} live_wake_owner_matched={live_wake_matches_enrolled_owner}",
                        phrase,
                        (wake_end_seconds * 1000.0).round() as u64,
                        source_speech_ms,
                        model_input_ms,
                        embeddings.len(),
                    );
                    return Ok(SessionSpeakerProfile {
                        embeddings: Arc::new(embeddings),
                        adaptive: false,
                    });
                }
                Err(err) => log::warn!(
                    "[speaker-verification] live verified wake exemplar unavailable; preserving enrolled session bank: {err}"
                ),
            }
            return Ok(SessionSpeakerProfile {
                embeddings: Arc::new(enrolled),
                adaptive: false,
            });
        }
        let (verified_wake_embedding, source_speech_ms, model_input_ms, inference_ms) =
            verified_wake_session_embedding(pcm, wake_end_seconds)?;
        log::info!(
            "[speaker-verification] ephemeral session target prepared phrase={} wake_end_ms={} speech_ms={} model_input_ms={} inference_ms={inference_ms}",
            phrase,
            (wake_end_seconds * 1000.0).round() as u64,
            source_speech_ms,
            model_input_ms,
        );
        Ok(SessionSpeakerProfile {
            embeddings: Arc::new(vec![verified_wake_embedding]),
            adaptive: true,
        })
    }

    fn verified_wake_session_embedding(
        pcm: &[u8],
        wake_end_seconds: f32,
    ) -> Result<(Vec<f32>, usize, usize, u128), String> {
        // An unavailable activation boundary is not a zero-length wake. The
        // caller may supply a separately confirmed identity window; otherwise
        // retain the enrolled bank without inventing a 250 ms live exemplar.
        if !wake_end_seconds.is_finite()
            || wake_end_seconds <= 0.0
            || wake_end_seconds as f64 * 32_000.0 > pcm.len() as f64 + 1.0
        {
            return Err("wake speaker reference has no valid audio boundary".into());
        }
        let wake_end_bytes =
            ((wake_end_seconds.max(0.0) * 32_000.0).round() as usize).min(pcm.len()) & !1usize;
        let profile_end_bytes = wake_end_bytes.saturating_add(250 * 32).min(pcm.len()) & !1usize;
        let profile_window_bytes = TEMPLATE_WINDOW_MS * 32;
        let profile_start = profile_end_bytes.saturating_sub(profile_window_bytes) & !1usize;
        let focused = pcm
            .get(profile_start..profile_end_bytes)
            .ok_or_else(|| "wake speaker window is outside candidate audio".to_string())?;
        let speech = session_speaker_speech_window(focused)?;
        let source_speech_ms = speech.len() / 32;
        let model_pcm = pad_session_speaker_pcm(speech);
        let model_input_ms = model_pcm.len() / 32;
        let runtime = ensure_runtime()?;
        let inference_started = std::time::Instant::now();
        let embedding = runtime.embedding(&model_pcm)?;
        Ok((
            embedding,
            source_speech_ms,
            model_input_ms,
            inference_started.elapsed().as_millis(),
        ))
    }

    pub fn observe_session_speaker(
        profile: &SessionSpeakerProfile,
        pcm: &[u8],
    ) -> Result<SessionSpeakerObservation, String> {
        let runtime = ensure_runtime()?;
        let speech = session_speaker_speech_window(pcm)?;
        let speech_span_ms = speech.len() / 32;
        let (real_speech_ms, peak_rms, reference_rms) = session_speaker_signal_metrics(speech)?;
        let model_pcm = pad_session_speaker_pcm(speech);
        let inference_started = std::time::Instant::now();
        let candidate = runtime.embedding(&model_pcm)?;
        let inference_ms = inference_started.elapsed().as_millis();
        let score = profile
            .embeddings
            .iter()
            .filter_map(|target| cosine(target, &candidate).ok())
            .max_by(f32::total_cmp)
            .ok_or_else(|| "session speaker dimensions do not match".to_string())?;
        // Keep a deliberate uncertainty band around the accepted owner threshold.
        // Repeat-padding is valid for the embedding model, but a subsecond body
        // fragment does not contain enough independent speech to exclude the
        // stabilized speaker.
        let classification =
            session_speaker_classification_for_signal(score, real_speech_ms, peak_rms);
        let transcript_speaker_evidence =
            crate::speech_decision_kernel::classify_transcript_speaker_evidence(
                score,
                real_speech_ms,
                peak_rms,
            );
        log::info!(
            "[speaker-verification] local session sample real_speech_ms={real_speech_ms} speech_span_ms={speech_span_ms} model_input_ms={} peak_rms={peak_rms:.1} reference_rms={reference_rms:.1} inference_ms={inference_ms} score={score:.6} classification={classification:?} transcript_speaker_evidence={}",
            model_pcm.len() / 32,
            transcript_speaker_evidence.label()
        );
        Ok(SessionSpeakerObservation {
            classification,
            real_speech_ms,
            signal_quality_sufficient: super::session_speaker_signal_quality_sufficient(
                real_speech_ms,
                peak_rms,
            ),
            transcript_speaker_evidence,
            embedding: candidate,
        })
    }

    pub fn classify_session_speaker(
        profile: &SessionSpeakerProfile,
        pcm: &[u8],
    ) -> Result<SessionSpeakerClassification, String> {
        observe_session_speaker(profile, pcm).map(|observation| observation.classification)
    }

    fn invalidate_loaded_profile_for_phrase_change(
        state: &mut State,
        persist: impl FnOnce(&SpeakerTemplate) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(mut template) = state.template.clone() {
            template.invalidated = true;
            persist(&template)?;
            state.template = Some(template);
        }
        // A failed first enrollment has a derived draft but no committed
        // template. Phrase ownership must retire in that state as well.
        state.capture = Some(CaptureState::Idle);
        state.progress = 0;
        state.last_score = None;
        state.error = None;
        state.enrollment_phrase = None;
        state.enrollment_capture_started = None;
        state.enrollment_live = EnrollmentLiveState::default();
        retire_pending_enrollment(state);
        Ok(())
    }

    pub fn invalidate_for_phrase_change(
        previous_phrase: &str,
        next_phrase: &str,
    ) -> Result<(), String> {
        let previous = crate::wake_phrase::normalize_configured_phrase(previous_phrase)?;
        let next = crate::wake_phrase::normalize_configured_phrase(next_phrase)?;
        if previous == next {
            return Ok(());
        }
        let mut state = STATE.lock();
        if matches!(
            state.capture,
            Some(
                CaptureState::Preparing
                    | CaptureState::Armed
                    | CaptureState::Capturing
                    | CaptureState::Processing
            )
        ) {
            return Err("声纹录制进行中，完成或取消后才能更换唤醒词。".to_string());
        }
        load_template_for_phrase_locked(&mut state, &previous);
        invalidate_loaded_profile_for_phrase_change(&mut state, persist_template)?;
        log::info!(
            "[speaker-verification] enrollment retired after wake phrase change previous={} next={} committed_profile_present={}; re-enrollment required",
            previous,
            next,
            state.template.is_some(),
        );
        Ok(())
    }

    pub fn enrollment_should_process() -> bool {
        matches!(
            STATE.lock().capture,
            Some(CaptureState::Armed | CaptureState::Capturing | CaptureState::Processing)
        )
    }

    pub fn cancel_enrollment(wake_phrase: &str) -> Result<VoiceprintStatus, String> {
        let phrase = crate::wake_phrase::normalize_configured_phrase(wake_phrase)?;
        let should_stop = {
            let mut state = STATE.lock();
            let should_stop = enrollment_capture_needs_host_stop(state.capture);
            state.capture = Some(CaptureState::Idle);
            state.progress = 0;
            state.error = None;
            state.enrollment_phrase = None;
            state.enrollment_capture_started = None;
            state.enrollment_live = EnrollmentLiveState::default();
            retire_pending_enrollment(&mut state);
            should_stop
        };
        if should_stop {
            crate::embedded_ble::send_recording_control_stop(Duration::from_secs(4))?;
        }
        log::info!("[speaker-verification] enrollment cancelled by owner");
        Ok(status_for_phrase(&phrase))
    }

    pub fn delete_template() -> Result<VoiceprintStatus, String> {
        let mut state = STATE.lock();
        // Serialize deletion with a successful enrollment commit/retry.
        delete_stored_credentials()?;
        state.template = None;
        state.template_checked = true;
        state.profile_state = VoiceProfileState::Unenrolled;
        state.capture = Some(CaptureState::Idle);
        state.progress = 0;
        state.last_score = None;
        state.error = None;
        state.enrollment_phrase = None;
        state.enrollment_capture_started = None;
        state.enrollment_live = EnrollmentLiveState::default();
        retire_pending_enrollment(&mut state);
        drop(state);
        Ok(status())
    }

    fn retire_pending_enrollment(state: &mut State) {
        state.enrollment_generation = state.enrollment_generation.wrapping_add(1);
        state.native_word_reference = None;
        state.pending_template = None;
        state.failure_stage = None;
    }

    fn mark_error(error: &str, stage: EnrollmentFailureStage) {
        let mut state = STATE.lock();
        state.capture = Some(CaptureState::Error);
        state.error = Some(error.to_string());
        state.failure_stage = Some(stage);
        state.enrollment_capture_started = None;
    }

    pub fn fail_enrollment(error: &str) {
        mark_error(error, EnrollmentFailureStage::Device);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use super::super::{
            session_target_with_wake_anchor, SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE,
            SESSION_SPEAKER_MAX_EMBEDDINGS, SESSION_SPEAKER_NON_TARGET_MAX_SCORE,
        };

        fn synthetic_complete_profile(phrase: &str) -> SpeakerTemplate {
            SpeakerTemplate {
                wake_embeddings: vec![vec![0.125; 192]; 3],
                session_embeddings: vec![vec![0.25; 192]; 3],
                target_speaker_embedding: Some(vec![0.5; 192]),
                personal_activity_reference: Some(crate::personal_vad::PersonalVadReference::from_bound_values(
                    crate::personal_vad::ENCODER_SHA,
                    crate::speaker_identity_eres::FRONTEND_ID,
                    vec![0.75; 192],
                ).unwrap()),
                phrase: Some(phrase.into()),
                invalidated: false,
            }
        }

        #[test]
        fn enrollment_storage_every_model_bank_fits_the_windows_binary_capacity() {
            for phrase in ["开始录音", "Start recording", "日本語 한국어 🎙️"] {
                let values = encode_profile_entries(&synthetic_complete_profile(phrase)).unwrap();
                assert_eq!(values.len(), 8);
                let mut reproduces_old_failure = false;
                for value in values {
                    reproduces_old_failure |= value.encode_utf16().count() * 2 > 2560;
                    let blob = crate::protected_voice_profile::encode_windows_credential(&value).unwrap();
                    assert_eq!(crate::protected_voice_profile::decode_windows_credential(&blob).unwrap(), value);
                }
                assert!(reproduces_old_failure, "the realistic profile must exercise the reported password limit");
            }
        }

        #[test]
        fn enrollment_storage_failure_can_retry_features_without_losing_the_old_profile() {
            let mut state = State {
                template: Some(synthetic_complete_profile("old phrase")),
                pending_template: Some(synthetic_complete_profile("开始录音")),
                capture: Some(CaptureState::Processing),
                ..State::default()
            };
            assert!(publish_pending_template(&mut state, |_| Err("vault unavailable".into())).is_err());
            assert_eq!(state.template.as_ref().unwrap().phrase.as_deref(), Some("old phrase"));
            assert!(state.pending_template.is_some());
            assert_eq!(state.failure_stage, Some(EnrollmentFailureStage::Storage));
            publish_pending_template(&mut state, |template| {
                assert_eq!(template.phrase.as_deref(), Some("开始录音"));
                Ok(())
            }).unwrap();
            assert_eq!(state.capture, Some(CaptureState::Complete));
            assert_eq!(state.template.as_ref().unwrap().phrase.as_deref(), Some("开始录音"));
            assert!(state.pending_template.is_none());
            assert!(state.failure_stage.is_none());
        }

        #[test]
        fn enrollment_phrase_change_retires_failed_first_enrollment_without_a_saved_profile() {
            let mut state = State {
                enrollment_generation: 7,
                enrollment_phrase: Some("开始录音".into()),
                capture: Some(CaptureState::Error),
                failure_stage: Some(EnrollmentFailureStage::Storage),
                error: Some("vault unavailable".into()),
                pending_template: Some(synthetic_complete_profile("开始录音")),
                ..State::default()
            };
            invalidate_loaded_profile_for_phrase_change(&mut state, |_| {
                panic!("no committed profile needs persistence")
            }).unwrap();
            assert!(state.template.is_none());
            assert!(state.pending_template.is_none());
            assert!(state.enrollment_phrase.is_none());
            assert!(state.failure_stage.is_none());
            assert!(state.error.is_none());
            assert_eq!(state.capture, Some(CaptureState::Idle));
            assert_eq!(state.enrollment_generation, 8);
        }

        #[test]
        fn enrollment_phrase_change_invalidates_committed_profile_and_retires_the_draft() {
            let mut state = State {
                enrollment_generation: 7,
                template: Some(synthetic_complete_profile("开始录音")),
                pending_template: Some(synthetic_complete_profile("开始录音")),
                capture: Some(CaptureState::Error),
                failure_stage: Some(EnrollmentFailureStage::Storage),
                ..State::default()
            };
            let mut writes = 0;
            invalidate_loaded_profile_for_phrase_change(&mut state, |profile| {
                writes += 1;
                assert!(profile.invalidated);
                Ok(())
            }).unwrap();
            assert_eq!(writes, 1);
            assert!(state.template.as_ref().unwrap().invalidated);
            assert!(state.pending_template.is_none());
            assert!(state.failure_stage.is_none());
            assert_eq!(state.enrollment_generation, 8);
        }

        #[test]
        fn enrollment_phrase_change_failed_invalidation_preserves_current_profile_and_draft() {
            let mut state = State {
                enrollment_generation: 7,
                template: Some(synthetic_complete_profile("开始录音")),
                pending_template: Some(synthetic_complete_profile("开始录音")),
                capture: Some(CaptureState::Error),
                failure_stage: Some(EnrollmentFailureStage::Storage),
                ..State::default()
            };
            assert!(invalidate_loaded_profile_for_phrase_change(&mut state,
                |_| Err("vault unavailable".into())).is_err());
            assert!(!state.template.as_ref().unwrap().invalidated);
            assert!(state.pending_template.is_some());
            assert_eq!(state.failure_stage, Some(EnrollmentFailureStage::Storage));
            assert_eq!(state.capture, Some(CaptureState::Error));
            assert_eq!(state.enrollment_generation, 7);
        }

        #[test]
        fn enrollment_storage_cancelled_generation_cannot_publish_or_reuse_retired_features() {
            let mut state = State {
                enrollment_generation: 7,
                enrollment_phrase: Some("开始录音".into()),
                capture: Some(CaptureState::Processing),
                pending_template: Some(synthetic_complete_profile("开始录音")),
                template: Some(synthetic_complete_profile("old phrase")),
                ..State::default()
            };
            assert!(current_processing_generation(&state, "开始录音", 7));
            retire_pending_enrollment(&mut state);
            assert!(!current_processing_generation(&state, "开始录音", 7));
            assert!(state.pending_template.is_none());
            assert_eq!(state.template.as_ref().unwrap().phrase.as_deref(), Some("old phrase"));
            assert!(!current_processing_generation(&state, "changed phrase", 8));
        }

        #[test]
        #[ignore = "writes only synthetic profiles into unique temporary Windows credential names"]
        fn enrollment_storage_real_windows_vault_roundtrip_and_legacy_password_compatibility() {
            use crate::protected_voice_profile::CredentialStore;
            use std::cell::RefCell;
            struct ScopedStore { prefix: String, names: RefCell<std::collections::HashSet<String>> }
            impl ScopedStore {
                fn name(&self, account: &str) -> String {
                    let name = format!("{}-{account}", self.prefix);
                    self.names.borrow_mut().insert(name.clone());
                    name
                }
            }
            impl CredentialStore for ScopedStore {
                fn get(&self, account: &str) -> Result<Option<String>, String> { VoiceProfileStore.get(&self.name(account)) }
                fn set(&self, account: &str, value: &str) -> Result<(), String> { VoiceProfileStore.set(&self.name(account), value) }
                fn delete(&self, account: &str) -> Result<(), String> { VoiceProfileStore.delete(&self.name(account)) }
            }
            impl Drop for ScopedStore {
                fn drop(&mut self) {
                    for name in self.names.get_mut().iter() { let _ = VoiceProfileStore.delete(name); }
                }
            }
            let store = ScopedStore {
                prefix: format!("enrollment-storage-repair-test-{}", uuid::Uuid::new_v4()),
                names: RefCell::new(std::collections::HashSet::new()),
            };
            let legacy = r#"{"phrase":"旧声纹 日本語 한국어 🎙️"}"#;
            keyring_entry_for(&store.name("legacy")).unwrap().set_password(legacy).unwrap();
            assert_eq!(store.get("legacy").unwrap().as_deref(), Some(legacy));
            let values = encode_profile_entries(&synthetic_complete_profile("开始录音")).unwrap();
            let formerly_oversized = values.iter().find(|value| value.encode_utf16().count() * 2 > 2560).unwrap();
            assert!(matches!(keyring_entry_for(&store.name("old-password-limit")).unwrap().set_password(formerly_oversized), Err(keyring::Error::TooLong(_, _))));
            crate::protected_voice_profile::save(&store, &values).unwrap();
            let loaded = crate::protected_voice_profile::load(&store).unwrap().unwrap();
            assert_eq!(loaded, values);
            let profile = decode_committed_profile(&loaded).unwrap();
            assert_eq!(profile.wake_embeddings.len(), 3);
            assert_eq!(profile.session_embeddings.len(), 3);
            assert!(profile.personal_activity_reference.is_some());
            let updated = encode_profile_entries(&synthetic_complete_profile("Start recording")).unwrap();
            crate::protected_voice_profile::save(&store, &updated).unwrap();
            assert_eq!(crate::protected_voice_profile::load(&store).unwrap().unwrap(), updated);
            crate::protected_voice_profile::delete(&store).unwrap();
            assert!(crate::protected_voice_profile::load(&store).unwrap().is_none());
            for name in store.names.borrow().iter() { VoiceProfileStore.delete(name).unwrap(); }
            assert!(store.names.borrow().iter().all(|name| VoiceProfileStore.get(name).unwrap().is_none()));
            println!("Windows vault: old password limit reproduced; eight binary banks, commit, legacy Unicode read, upgrade, delete and cleanup passed");
        }

        #[test]
        fn missing_or_invalid_wake_boundary_cannot_create_a_live_voice_exemplar() {
            let pcm = vec![0u8; 3_000 * 32];
            for end in [0.0, -1.0, f32::NAN, f32::INFINITY, 3.1] {
                let error = verified_wake_session_embedding(&pcm, end).unwrap_err();
                assert!(error.contains("no valid audio boundary"));
            }
        }

        /// Offline matrix: read a canonical 16 kHz mono s16le WAV (the diag
        /// writer and recordings both emit this shape).
        #[cfg(target_os = "windows")]
        fn read_pcm16_wav(path: &str) -> Vec<u8> {
            let bytes = std::fs::read(path).expect("wav readable");
            let data_at = bytes
                .windows(4)
                .position(|w| w == b"data")
                .expect("data chunk");
            let len = u32::from_le_bytes([
                bytes[data_at + 4],
                bytes[data_at + 5],
                bytes[data_at + 6],
                bytes[data_at + 7],
            ]) as usize;
            bytes[data_at + 8..data_at + 8 + len].to_vec()
        }

        #[test]
        #[ignore = "requires a fixed diarization-window manifest and protected enrolled voiceprint"]
        fn recorded_diarization_identity_probe() {
            let input = std::env::var("LISTENER_DIAR_IDENTITY_INPUT").unwrap();
            let output = std::env::var("LISTENER_DIAR_IDENTITY_OUTPUT").unwrap();
            let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
            let phrase = fixture["wake_phrase"].as_str().unwrap();
            let profile = enrolled_session_profile(phrase).unwrap().expect("protected enrolled profile");
            assert!(!profile.is_adaptive());
            let rows: Vec<_> = fixture["windows"].as_array().unwrap().iter().map(|window| {
                let pcm = std::fs::read(window["pcm_path"].as_str().unwrap()).unwrap();
                let start = window["start_ms"].as_u64().unwrap() as usize * 32;
                let end = window["end_ms"].as_u64().unwrap() as usize * 32;
                let observed = observe_session_speaker(&profile, &pcm[start..end]).unwrap();
                serde_json::json!({
                    "id": window["id"], "channel": window["channel"],
                    "start_ms": window["start_ms"], "end_ms": window["end_ms"],
                    "score": observed.classification.score(),
                    "classification": format!("{:?}", observed.classification),
                    "real_speech_ms": observed.real_speech_ms,
                    "signal_quality_sufficient": observed.signal_quality_sufficient,
                    "transcript_speaker_evidence": observed.transcript_speaker_evidence.label(),
                })
            }).collect();
            // No embeddings or protected credential contents leave the process.
            std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        }

        #[test]
        #[ignore = "requires selected native source intervals and current protected model-specific references; diagnostic only"]
        fn recorded_native_row_protected_identity_probe() {
            let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(
                std::env::var("LISTENER_NATIVE_ROW_IDENTITY_INPUT").unwrap()).unwrap()).unwrap();
            let phrase = crate::wake_phrase::normalize_configured_phrase(fixture["wake_phrase"].as_str().unwrap()).unwrap();
            // Snapshot both references without the legacy phrase-binding write,
            // adaptive bank updates, or exporting the protected vectors.
            let (profile, reference) = {
                let mut state = STATE.lock();
                load_template_locked(&mut state);
                let template = state.template.as_ref().filter(|template|
                    template_matches_phrase(template, &phrase)).expect("already-bound protected profile");
                (SessionSpeakerProfile {
                    embeddings: Arc::new(template.session_embeddings.clone()), adaptive: false,
                }, template.personal_activity_reference.clone().expect("protected activity reference"))
            };
            let qualified_profile = fixture["qualified_owner_reference_pcm_path"].as_str().map(|path| {
                let pcm = std::fs::read(path).unwrap();
                let observed = observe_session_speaker(&profile, &pcm).unwrap();
                assert!(observed.signal_quality_sufficient
                    && matches!(observed.classification, SessionSpeakerClassification::Target { .. }),
                    "candidate reference must independently match the current protected bank with sufficient speech");
                let mut candidate = profile.clone();
                Arc::make_mut(&mut candidate.embeddings).push(observed.embedding);
                candidate
            });
            // Diagnostic-only comparison: the automatically qualified source
            // is evaluated alone as well as in the product's max-reference bank.
            // It never replaces or persists the product reference.
            let natural_profile = qualified_profile.as_ref().map(|candidate| SessionSpeakerProfile {
                embeddings:Arc::new(vec![candidate.embeddings.last().unwrap().clone()]),adaptive:false,
            });
            let rows: Vec<_> = fixture["windows"].as_array().unwrap().iter().map(|window| {
                let pcm = std::fs::read(window["pcm_path"].as_str().unwrap()).unwrap();
                let start = window["start_ms"].as_u64().unwrap() as usize * 32;
                let end = window["end_ms"].as_u64().unwrap() as usize * 32;
                let mut row = serde_json::json!({"id":window["id"], "channel":window["channel"],
                    "start_ms":window["start_ms"], "end_ms":window["end_ms"], "text":window["text"],
                    "expected_source":window["expected_source"]});
                if start >= end || end > pcm.len() {
                    row["unavailable"] = serde_json::json!("exact source interval unavailable; no clamp or padding");
                    return row;
                }
                let source = &pcm[start..end];
                match observe_session_speaker(&profile, source) {
                    Ok(observed) => {
                        row["identity_score"] = serde_json::json!(observed.classification.score());
                        row["identity_classification"] = serde_json::json!(format!("{:?}", observed.classification));
                        row["real_speech_ms"] = serde_json::json!(observed.real_speech_ms);
                        row["signal_quality_sufficient"] = serde_json::json!(observed.signal_quality_sufficient);
                        row["transcript_speaker_evidence"] = serde_json::json!(observed.transcript_speaker_evidence.label());
                    }
                    Err(error) => { row["identity_unavailable"] = serde_json::json!(error); }
                }
                if let Some(candidate) = qualified_profile.as_ref() {
                    match observe_session_speaker(candidate, source) {
                        Ok(observed) => {
                            row["qualified_reference_identity_score"] = serde_json::json!(observed.classification.score());
                            row["qualified_reference_identity_classification"] = serde_json::json!(format!("{:?}", observed.classification));
                            row["qualified_reference_signal_quality_sufficient"] = serde_json::json!(observed.signal_quality_sufficient);
                        }
                        Err(error) => { row["qualified_reference_identity_unavailable"] = serde_json::json!(error); }
                    }
                }
                if let Some(candidate) = natural_profile.as_ref() {
                    match observe_session_speaker(candidate, source) {
                        Ok(observed) => {
                            row["natural_only_score"] = serde_json::json!(observed.classification.score());
                            row["natural_only_classification"] = serde_json::json!(format!("{:?}",observed.classification));
                            row["natural_only_evidence"] = serde_json::json!(observed.transcript_speaker_evidence.label());
                        }
                        Err(error) => { row["natural_only_unavailable"] = serde_json::json!(error); }
                    }
                }
                if fixture["skip_activity_encoder"].as_bool()!=Some(true) {
                match crate::personal_vad::enrollment_reference(source) {
                    Ok(candidate) => {
                        let score: f32 = reference.values().iter().zip(candidate.values()).map(|(a,b)| a*b).sum();
                        row["activity_encoder_cosine"] = serde_json::json!(score);
                    }
                    Err(error) => { row["activity_encoder_unavailable"] = serde_json::json!(error); }
                }
                }
                row
            }).collect();
            std::fs::write(std::env::var("LISTENER_NATIVE_ROW_IDENTITY_OUTPUT").unwrap(),
                serde_json::to_vec_pretty(&serde_json::json!({"rows":rows,
                    "reference":"current immutable protected registration; model-specific vectors remain in process",
                    "cloud_calls":0,"enrollment_writes":0,"editor_writes":0,"may_delete_text":false})).unwrap()).unwrap();
        }

        #[test]
        #[ignore = "requires exact original verification PCM and current protected wake bank"]
        fn recorded_current_wake_identity_probe() {
            use sha2::{Digest,Sha256};
            let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(
                std::env::var("LISTENER_CURRENT_WAKE_INPUT").unwrap()).unwrap()).unwrap();
            let phrase = crate::wake_phrase::normalize_configured_phrase(fixture["wake_phrase"].as_str().unwrap()).unwrap();
            // Assert an already-bound template before calling production verify:
            // this excludes its legacy phrase-binding persistence branch.
            {
                let mut state = STATE.lock();
                load_template_locked(&mut state);
                assert!(state.template.as_ref().is_some_and(|t|template_matches_phrase(t,&phrase)));
            }
            let pcm=std::fs::read(fixture["verification_pcm_path"].as_str().unwrap()).unwrap();
            let hash=format!("{:x}",Sha256::digest(&pcm));
            assert_eq!(hash,fixture["verification_pcm_sha256"].as_str().unwrap());
            let result=verify(&pcm,&phrase).unwrap();
            let enrolled=matches!(result.policy,VerificationPolicy::Enrolled);
            let report=serde_json::json!({"schema":"listener.current-bank-exact-wake-replay.v1",
                "pcm_sha256":hash,"pcm_ms":pcm.len()/32,"score":result.score,
                "threshold":VERIFICATION_THRESHOLD,"matched":result.matched,
                "owner_matched":result.owner_matched,"enrolled_policy":enrolled,
                "scope":"Exact archived original verification input against current already-bound protected wake bank. No template mutation, adaptation, added reference or new enrollment."});
            std::fs::write(std::env::var("LISTENER_CURRENT_WAKE_OUTPUT").unwrap(),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            assert!(enrolled,"open wake never proves Owner");
        }

        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        #[test]
        #[ignore = "requires frozen source manifest and protected reference; no production routing"]
        fn recorded_selective_overlap_audio_probe() {
            use sha2::{Digest, Sha256};
            use crate::asr::target_speaker_extraction::{TargetSpeakerExtractor, CHUNK_BYTES};
            let input = std::env::var("LISTENER_SELECTIVE_AUDIO_INPUT").unwrap();
            let output = std::env::var("LISTENER_SELECTIVE_AUDIO_OUTPUT").unwrap();
            let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
            let pcm = std::fs::read(fixture["pcm_path"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&pcm)),fixture["pcm_sha256"].as_str().unwrap());
            let phrase = crate::wake_phrase::normalize_configured_phrase(fixture["wake_phrase"].as_str().unwrap()).unwrap();
            let selective = fixture["selective_extraction"].as_bool().unwrap();
            let profile = enrolled_session_profile(&phrase).unwrap().unwrap();
            assert!(!profile.is_adaptive());
            let extractor = selective.then(|| {
                let reference = {
                    let mut state = STATE.lock();
                    load_template_locked(&mut state);
                    state.template.as_ref().filter(|t| template_matches_phrase(t,&phrase))
                        .and_then(|t|t.target_speaker_embedding.clone()).expect("protected separator reference")
                };
                TargetSpeakerExtractor::from_embedding(reference).unwrap()
            });
            let overlaps = fixture["overlap_ranges_ms"].as_array().unwrap();
            let directory = std::path::PathBuf::from(fixture["output_directory"].as_str().unwrap());
            std::fs::create_dir_all(&directory).unwrap();
            let mut quality = crate::speech_vad_windows::SourceSpeechQuality::new().unwrap();
            let mut hybrid = Vec::with_capacity(pcm.len());
            let mut chunks = Vec::new();
            let mut first_resolved = vec![serde_json::Value::Null; fixture["quality_windows"].as_array().unwrap().len()];
            for (index, chunk) in pcm.chunks(CHUNK_BYTES).enumerate() {
                let start = index as u64 * 3000;
                let end = start + chunk.len() as u64 / 32;
                let extract = selective && overlaps.iter().any(|r|r[0].as_u64().unwrap()<end && start<r[1].as_u64().unwrap());
                let started = std::time::Instant::now();
                let (chosen, metrics) = if extract {
                    let separated = extractor.as_ref().unwrap().extract_chunk_with_metrics(chunk).unwrap();
                    let observation = observe_session_speaker(&profile,&separated.pcm).unwrap();
                    let metrics = serde_json::json!({"classification":format!("{:?}",observation.classification),
                        "score":observation.classification.score(),"energy_speech_ms":observation.real_speech_ms,
                        "signal_quality_sufficient":observation.signal_quality_sufficient,
                        "transcript_evidence":observation.transcript_speaker_evidence.label(),
                        "separator_target_similarity":separated.target_similarity,
                        "target_to_mix_energy":separated.target_to_mix_energy});
                    (separated.pcm,metrics)
                } else { (chunk.to_vec(),serde_json::Value::Null) };
                assert_eq!(chosen.len(),chunk.len());
                quality.accept(&chosen).unwrap();
                for (i, window) in fixture["quality_windows"].as_array().unwrap().iter().enumerate() {
                    if first_resolved[i].is_null() {
                        if let Some(speech) = quality.resolved_speech_ms(window[0].as_u64().unwrap(),window[1].as_u64().unwrap()) {
                            first_resolved[i]=serde_json::json!({"observed_input_ms":end,"resolved_speech_ms":speech});
                        }
                    }
                }
                if extract { std::fs::write(directory.join(format!("{start:06}-{end:06}-extracted.pcm")),&chosen).unwrap(); }
                chunks.push(serde_json::json!({"start_ms":start,"end_ms":end,"extracted":extract,
                    "wall_ms":started.elapsed().as_millis(),"pcm_sha256":format!("{:x}",Sha256::digest(&chosen)),"metrics":metrics}));
                hybrid.extend_from_slice(&chosen);
            }
            quality.finish();
            assert!(quality.is_valid(),"source quality must not rewrite finalized silence");
            for row in &mut chunks {
                row["resolved_speech_ms"]=serde_json::json!(quality.resolved_speech_ms(row["start_ms"].as_u64().unwrap(),row["end_ms"].as_u64().unwrap()));
            }
            let resolved: Vec<_> = fixture["quality_windows"].as_array().unwrap().iter().map(|w|
                quality.resolved_speech_ms(w[0].as_u64().unwrap(),w[1].as_u64().unwrap())).collect();
            let hybrid_path = directory.join("routed.pcm");
            std::fs::write(&hybrid_path,&hybrid).unwrap();
            let report=serde_json::json!({"schema":"listener.selective-overlap-audio.v1",
                "source_pcm_sha256":fixture["pcm_sha256"],"selective_extraction":selective,
                "routed_pcm_path":hybrid_path,"routed_pcm_sha256":format!("{:x}",Sha256::digest(&hybrid)),
                "chunks":chunks,"quality_windows":fixture["quality_windows"],"final_resolved_speech_ms":resolved,
                "first_resolved":first_resolved,"completed_speech_ranges_ms":quality.completed_speech_ranges_ms(),
                "scope":"Frozen existing protected references; only fixed3s chunks intersecting native overlap are extracted. Final VAD flush is explicit. No new template, identity permission, provider or editor acceptance."});
            std::fs::write(output,serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }

        #[cfg(target_os = "windows")]
        #[test]
        #[ignore = "requires an explicit captured source window fixture and installed owner template"]
        fn recorded_pending_source_window_probe() {
            let input = std::env::var("LISTENER_SOURCE_WINDOW_INPUT").unwrap();
            let output = std::env::var("LISTENER_SOURCE_WINDOW_OUTPUT").unwrap();
            let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
            let pcm = std::fs::read(fixture["pcm_path"].as_str().unwrap()).unwrap();
            let identity_pcm = fixture.get("identity_pcm_path").and_then(serde_json::Value::as_str)
                .map(|path| std::fs::read(path).unwrap()).unwrap_or_else(|| pcm.clone());
            let profile = session_profile_snapshot_from_wake(&identity_pcm,
                fixture["wake_end_ms"].as_u64().unwrap() as f32 / 1000.0,
                fixture["wake_phrase"].as_str().unwrap(), true).unwrap();
            assert!(!profile.is_adaptive());
            let mut context = crate::speaker_verification::VerifiedOriginalSourceContext::new(profile.clone()).unwrap();
            #[cfg(feature = "target-speaker-extraction")]
            let separated_reference = target_speaker_embedding_for_phrase(fixture["wake_phrase"].as_str().unwrap()).unwrap();
            #[cfg(feature = "target-speaker-extraction")]
            let diagnostic_owner_reference_only =
                fixture["probe_diagnostic_owner_reference_only"].as_bool() == Some(true);
            #[cfg(feature = "target-speaker-extraction")]
            let live_reference = (!diagnostic_owner_reference_only).then(|| {
                crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(
                    &crate::asr::target_speaker_extraction::wake_phrase_enrollment_pcm(
                        &identity_pcm, fixture["wake_end_ms"].as_u64().unwrap() as f32 / 1000.0,
                    ),
                ).unwrap()
            });
            let rows: Vec<_> = pcm.chunks(3_000 * 32).enumerate().map(|(index, chunk)| {
                let observation = observe_session_speaker(&profile, chunk).unwrap();
                #[cfg(feature = "target-speaker-extraction")]
                let (source_encoder_similarity, source_encoder_error) = match separated_reference.as_ref() {
                    Some(reference) => match crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(chunk) {
                        Ok(embedding) => match super::super::normalized_cosine(reference, &embedding) {
                            Some(score) => (Some(score), None),
                            None => (None, Some("incompatible or invalid reference/observation vectors".to_owned())),
                        },
                        Err(error) => (None, Some(error)),
                    },
                    None => (None, Some("no enrolled target-speaker extraction reference".to_owned())),
                };
                #[cfg(not(feature = "target-speaker-extraction"))]
                let (source_encoder_similarity, source_encoder_error): (Option<f32>, Option<String>) =
                    (None, Some("target-speaker-extraction feature disabled".to_owned()));
                #[cfg(feature = "target-speaker-extraction")]
                let separation = if fixture["compare_separator_references"].as_bool() == Some(true) {
                    let mut observations = Vec::new();
                    let mut references = Vec::new();
                    if diagnostic_owner_reference_only {
                        let path = fixture["diagnostic_owner_reference_pcm_path"].as_str()
                            .expect("diagnostic owner reference path");
                        let reference_pcm = std::fs::read(path).unwrap();
                        references.push(("labelled_owner_body_diagnostic_only",
                            crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(&reference_pcm).unwrap()));
                    } else {
                        if let Some(reference) = separated_reference.as_ref() {
                            references.push(("persisted", reference.clone()));
                        }
                        if fixture["probe_persisted_separator_only"].as_bool() != Some(true) {
                            references.push(("accepted_wake", live_reference.clone().unwrap()));
                        }
                    }
                    // Diagnostic-only counterfactuals. Never save these references or
                    // admit their output to a recording/provider/editor session.
                    if !diagnostic_owner_reference_only
                        && fixture["probe_reference_contract"].as_bool() == Some(true) {
                        let wake = crate::asr::target_speaker_extraction::wake_phrase_enrollment_pcm(
                            &identity_pcm, fixture["wake_end_ms"].as_u64().unwrap() as f32 / 1000.0);
                        assert!(!wake.is_empty());
                        let repeated: Vec<u8> = wake.iter().copied().cycle().take(3_000 * 32).collect();
                        references.push(("accepted_wake_pcm_repeated",
                            crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(&repeated).unwrap()));
                        if let Some(path) = fixture["diagnostic_owner_reference_pcm_path"].as_str() {
                            let reference_pcm = std::fs::read(path).unwrap();
                            references.push(("labelled_owner_body_diagnostic_only",
                                crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(&reference_pcm).unwrap()));
                        }
                        let normalized: Vec<_> = references.iter().take(2).map(|(label, reference)| {
                            let norm = reference.iter().map(|v| v * v).sum::<f32>().sqrt();
                            assert!(norm.is_finite() && norm > 0.0);
                            (if *label == "persisted" { "persisted_unit_norm" } else { "accepted_wake_unit_norm" },
                                reference.iter().map(|v| v / norm).collect::<Vec<_>>())
                        }).collect();
                        references.extend(normalized);
                    }
                    for (label, reference) in references {
                        let reference_norm = reference.iter().map(|v| v * v).sum::<f32>().sqrt();
                        let extracted = crate::asr::target_speaker_extraction::TargetSpeakerExtractor::from_embedding(reference.clone())
                            .and_then(|extractor| extractor.extract_chunk_with_metrics(chunk));
                        observations.push(match extracted {
                            Ok(extracted) => {
                                let observed = observe_session_speaker(&profile, &extracted.pcm);
                                let extracted_pcm_sha256 =
                                    crate::firmware_ota::sha256_hex(&extracted.pcm);
                                let exported_pcm_path = std::env::var_os(
                                    "LISTENER_TEST_SEPARATOR_WINDOW_EXPORT_DIR",
                                )
                                .filter(|_| label == "persisted"
                                    || label == "labelled_owner_body_diagnostic_only")
                                .map(|directory| {
                                    let directory = std::path::PathBuf::from(directory);
                                    std::fs::create_dir_all(&directory).unwrap();
                                    let path = directory.join(format!(
                                        "source-{:06}-{:06}-{}-extracted.pcm",
                                        index * 3_000,
                                        index * 3_000 + chunk.len() / 32,
                                        label,
                                    ));
                                    std::fs::write(&path, &extracted.pcm).unwrap();
                                    path.to_string_lossy().into_owned()
                                });
                                serde_json::json!({"reference":label,
                                    "reference_l2_norm":reference_norm,
                                    "target_to_mix_energy":extracted.target_to_mix_energy,
                                    "separator_identity_similarity":extracted.target_similarity,
                                    "residual_ratio":extracted.residual_energy / extracted.mixture_energy.max(1e-12),
                                    "classification":observed.as_ref().ok().map(|o| format!("{:?}",o.classification)),
                                    "quality":observed.as_ref().ok().map(|o| o.signal_quality_sufficient),
                                    "speech_ms":observed.as_ref().ok().map(|o| o.real_speech_ms),
                                    "extracted_pcm_path":exported_pcm_path,
                                    "extracted_pcm_sha256":extracted_pcm_sha256,
                                    "error":observed.err()})
                            },
                            Err(error) => serde_json::json!({"reference":label,"error":error}),
                        });
                    }
                    observations
                } else { Vec::new() };
                #[cfg(not(feature = "target-speaker-extraction"))]
                let separation: Vec<serde_json::Value> = Vec::new();
                let context_scores: Vec<_> = context.body_embeddings.iter()
                    .filter_map(|reference| super::super::normalized_cosine(reference, &observation.embedding))
                    .collect();
                let profile_scores: Vec<_> = profile.embeddings.iter()
                    .filter_map(|reference| super::super::normalized_cosine(reference, &observation.embedding))
                    .collect();
                let owner = context.certifies_observation(&observation);
                let subwindows: Vec<_> = (0..chunk.len()).step_by(600 * 32).filter_map(|start| {
                    let end = (start + 1_200 * 32).min(chunk.len());
                    if end - start < 600 * 32 { return None; }
                    let observation = observe_session_speaker(&profile, &chunk[start..end]).unwrap();
                    Some(serde_json::json!({"start_ms":index*3000+start/32,
                        "end_ms":index*3000+end/32,"classification":format!("{:?}",observation.classification),
                        "speech_ms":observation.real_speech_ms,"quality":observation.signal_quality_sufficient}))
                }).collect();
                serde_json::json!({"start_ms":index*3000,"end_ms":index*3000+chunk.len()/32,
                    "classification":format!("{:?}",observation.classification),
                    "speech_ms":observation.real_speech_ms,"quality":observation.signal_quality_sufficient,
                    "profile_scores":profile_scores,"context_scores_before_admission":context_scores,
                    "independent_source_encoder_similarity":source_encoder_similarity,
                    "independent_source_encoder_error":source_encoder_error,
                    "separator_reference_comparison":separation,
                    "production_context_owner":owner,"subwindows":subwindows})
            }).collect();
            std::fs::write(output,serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        }

        /// 2026-09-23 71f64ac1 复现器（终端唤醒 + 干扰环境 → 全程
        /// ConfirmedOther → 冻结/吞尾）。用当晚真实 WAV 重放:唤醒窗档案
        /// (end=5.938s, enrolled) 对会话正文的 1.2s 滚动窗评分序列。
        /// 加 --all-sessions 参数不可行,所以第二个测试扫当天全部会话 WAV
        /// 出分带直方图(校准自信线用)。
        /// Set LISTENER_FORENSIC_WAKE_WAV and LISTENER_FORENSIC_BODY_WAV to
        /// local fixture paths, then run with --ignored --nocapture.
        #[cfg(target_os = "windows")]
        #[test]
        #[ignore = "forensics tool: needs the 2026-09-23 field WAVs on disk"]
        fn ef_session_71f64ac1_replay() {
            let wake_path = std::env::var("LISTENER_FORENSIC_WAKE_WAV")
                .expect("set LISTENER_FORENSIC_WAKE_WAV to the accepted wake WAV");
            let body_path = std::env::var("LISTENER_FORENSIC_BODY_WAV")
                .expect("set LISTENER_FORENSIC_BODY_WAV to the session WAV");
            let wake_pcm = read_pcm16_wav(&wake_path);
            let body_pcm = read_pcm16_wav(&body_path);
            let profile = session_profile_from_wake(&wake_pcm, 5.938, "开始录音", true)
                .expect("profile from wake window");
            const WINDOW_MS: usize = 1_200;
            const STEP_MS: usize = 400;
            let mut cursor_ms = 1_000usize;
            println!("body_pcm_ms={}", body_pcm.len() / 32);
            while cursor_ms <= body_pcm.len() / 32 {
                let start = cursor_ms.saturating_sub(WINDOW_MS) * 32;
                let slice = &body_pcm[start..(cursor_ms) * 32];
                let observation = observe_session_speaker(&profile, slice).expect("observe");
                println!(
                    "t={:>5}ms score={:.4} real_speech_ms={} quality={} classification={:?}",
                    cursor_ms,
                    observation.classification.score(),
                    observation.real_speech_ms,
                    observation.signal_quality_sufficient,
                    observation.classification
                );
                cursor_ms += STEP_MS;
            }
        }

        /// 阈值矩阵:指定唤醒档案 对 本地会话正文的分带统计。
        /// 只统计 quality=true(≥1s 真语音)窗,分带:<0.20 / 0.20-0.40 /
        /// 0.40-0.55 / ≥0.55。校准 SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE
        /// 的依据输出。
        #[cfg(target_os = "windows")]
        #[test]
        #[ignore = "forensics tool: needs the 2026-09-23 field WAVs on disk"]
        fn ef_session_score_bands_matrix() {
            let wake_path = std::env::var("LISTENER_FORENSIC_WAKE_WAV")
                .expect("set LISTENER_FORENSIC_WAKE_WAV to the accepted wake WAV");
            let wake_pcm = read_pcm16_wav(&wake_path);
            let profile = session_profile_from_wake(&wake_pcm, 5.938, "开始录音", true)
                .expect("profile from wake window");
            let dir_path = std::env::var("LISTENER_FORENSIC_RECORDINGS_DIR")
                .expect("set LISTENER_FORENSIC_RECORDINGS_DIR to the recordings directory");
            let dir = std::path::Path::new(&dir_path);
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .expect("recordings dir")
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
                .collect();
            names.sort_by_key(|path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok()));
            for path in names.iter().rev().take(12) {
                let body_pcm = read_pcm16_wav(path.to_string_lossy().as_ref());
                const WINDOW_MS: usize = 1_200;
                const STEP_MS: usize = 400;
                let mut cursor_ms = 1_000usize;
                let (mut low, mut mid, mut high, mut top) = (0u32, 0u32, 0u32, 0u32);
                let mut min_quality = f32::MAX;
                let mut max_quality = f32::MIN;
                let mut quality_windows = 0u32;
                while cursor_ms <= body_pcm.len() / 32 {
                    let start = cursor_ms.saturating_sub(WINDOW_MS) * 32;
                    let slice = &body_pcm[start..(cursor_ms) * 32];
                    if let Ok(observation) = observe_session_speaker(&profile, slice) {
                        let score = observation.classification.score();
                        if observation.signal_quality_sufficient {
                            quality_windows += 1;
                            min_quality = min_quality.min(score);
                            max_quality = max_quality.max(score);
                            if score < 0.20 {
                                low += 1;
                            } else if score < 0.40 {
                                mid += 1;
                            } else if score < 0.55 {
                                high += 1;
                            } else {
                                top += 1;
                            }
                        }
                    }
                    cursor_ms += STEP_MS;
                }
                println!(
                    "{:?} quality_windows={} range=[{:.3},{:.3}] bands(<.20/.20-.40/.40-.55/>=.55)={}/{}/{}/{}",
                    path.file_name().map(|n| n.to_string_lossy().to_string()),
                    quality_windows,
                    min_quality,
                    max_quality,
                    low,
                    mid,
                    high,
                    top
                );
            }
        }

        #[test]
        fn guided_enrollment_step_boundaries_match_the_three_visible_prompts() {
            assert_eq!(enrollment_step_for_elapsed_ms(0), 0);
            assert_eq!(enrollment_step_for_elapsed_ms(2_999), 0);
            assert_eq!(enrollment_step_for_elapsed_ms(3_000), 1);
            assert_eq!(enrollment_step_for_elapsed_ms(6_000), 2);
            assert_eq!(enrollment_step_for_elapsed_ms(9_000), 2);
            assert_eq!(enrollment_step_for_elapsed_ms(15_000), 2);
            assert_eq!(enrollment_step_target_ms(0), 600);
            assert_eq!(enrollment_step_target_ms(2), 600);
        }

        #[test]
        fn guided_enrollment_feedback_uses_real_signal_evidence() {
            let mut live = EnrollmentLiveState::default();
            assert_eq!(enrollment_capture_feedback(0, 0, &live), "waiting");
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "too_quiet");
            live.last_frame_active = true;
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "hearing");
            live.step_speech_ms[0] = enrollment_step_target_ms(0);
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "good");
            live.last_frame_clipped = true;
            assert_eq!(enrollment_capture_feedback(0, 1_200, &live), "too_loud");
        }

        #[test]
        fn template_binary_round_trip_is_normalized_and_compact() {
            let mut embedding = (1..=192).map(|value| value as f32).collect::<Vec<_>>();
            normalize(&mut embedding).unwrap();
            let encoded =
                encode_template(&embedding, "小爱同学", false, TemplatePurpose::WakePhrase)
                    .unwrap();
            assert!(encoded.len() < 1800);
            let decoded = decode_template(&encoded).unwrap();
            assert!((cosine(&embedding, &decoded.wake_embeddings[0]).unwrap() - 1.0).abs() < 1e-5);
            assert!(decoded.session_embeddings.is_empty());
            assert!(decoded.target_speaker_embedding.is_none());
            assert_eq!(decoded.phrase.as_deref(), Some("小爱同学"));
            assert!(!decoded.invalidated);
        }

        #[test]
        fn committed_profile_keeps_personal_activity_out_of_the_wake_and_body_banks() {
            let cam = vec![1.0, 2.0, 3.0];
            let personal = vec![0.25; 192];
            let wake = encode_template(&cam, "开始录音", false, TemplatePurpose::WakePhrase).unwrap();
            let body = encode_template(&cam, "开始录音", false, TemplatePurpose::FreeSpeech).unwrap();
            let activity = encode_template(&personal, "开始录音", false, TemplatePurpose::PersonalActivity).unwrap();
            let profile = decode_committed_profile(&[wake.clone(), body.clone(), activity.clone()]).unwrap();
            assert_eq!(profile.wake_embeddings.len(), 1);
            assert_eq!(profile.session_embeddings.len(), 1);
            assert_eq!(profile.personal_activity_reference.unwrap().values().len(), 192);
            assert!(decode_committed_profile(&[wake.clone(), body.clone(), activity.clone(), activity]).is_err());
            let other_phrase = encode_template(&personal, "start recording", false, TemplatePurpose::PersonalActivity).unwrap();
            assert!(decode_committed_profile(&[wake.clone(), body.clone(), other_phrase]).is_err());
            let legacy = decode_committed_profile(&[wake, body]).unwrap();
            assert!(legacy.personal_activity_reference.is_none(), "old banks stay usable without reinterpreting their vectors");
        }

        #[test]
        fn personal_activity_reference_rejects_same_size_other_models_and_missing_frontends() {
            let activity = encode_template(&vec![0.25; 192], "开始录音", false, TemplatePurpose::PersonalActivity).unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&activity).unwrap();
            value["model_sha256"] = serde_json::json!(TARGET_SPEAKER_MODEL_SHA256);
            assert!(decode_template(&value.to_string()).is_err());
            value["model_sha256"] = serde_json::json!(crate::personal_vad::ENCODER_SHA);
            value["frontend_id"] = serde_json::Value::Null;
            assert!(decode_template(&value.to_string()).is_err());
            value["frontend_id"] = serde_json::json!(crate::speaker_identity_eres::FRONTEND_ID);
            value["dimension"] = serde_json::json!(191);
            assert!(decode_template(&value.to_string()).is_err());
        }

        #[test]
        fn template_purpose_separates_new_banks_and_maps_legacy_to_both() {
            let mut embedding = vec![1.0, 2.0, 3.0];
            normalize(&mut embedding).unwrap();
            let free = decode_template(
                &encode_template(&embedding, "开始录音", false, TemplatePurpose::FreeSpeech)
                    .unwrap(),
            )
            .unwrap();
            assert!(free.wake_embeddings.is_empty());
            assert_eq!(free.session_embeddings.len(), 1);

            let target = (0..TARGET_SPEAKER_EMBEDDING_DIMENSION)
                .map(|value| value as f32 / 100.0 - 0.5)
                .collect::<Vec<_>>();
            let decoded_target = decode_template(
                &encode_template(&target, "开始录音", false, TemplatePurpose::TargetSpeaker)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                decoded_target.target_speaker_embedding.as_deref(),
                Some(target.as_slice())
            );
            assert!(decoded_target.wake_embeddings.is_empty());
            assert!(decoded_target.session_embeddings.is_empty());

            let mut bytes = Vec::new();
            for value in &embedding {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            let legacy = serde_json::to_string(&StoredTemplate {
                version: 2,
                model_sha256: MODEL_SHA256.to_string(),
                dimension: embedding.len(),
                embedding_base64: BASE64.encode(bytes),
                phrase: Some("开始录音".into()),
                invalidated: false,
                purpose: None,
                frontend_id: None,
            })
            .unwrap();
            let legacy = decode_template(&legacy).unwrap();
            assert_eq!(legacy.wake_embeddings.len(), 1);
            assert_eq!(legacy.session_embeddings.len(), 1);
            assert!(
                (cosine(&legacy.wake_embeddings[0], &legacy.session_embeddings[0]).unwrap() - 1.0)
                    .abs()
                    < 1e-5
            );
        }

        #[test]
        fn corrupt_or_wrong_model_template_is_rejected() {
            assert!(decode_template("{}").is_err());
            let wrong = serde_json::to_string(&StoredTemplate {
                version: 1,
                model_sha256: "wrong".into(),
                dimension: 1,
                embedding_base64: BASE64.encode(0.5f32.to_le_bytes()),
                phrase: Some("小爱同学".into()),
                invalidated: false,
                purpose: None,
                frontend_id: None,
            })
            .unwrap();
            assert!(decode_template(&wrong).is_err());
        }

        #[test]
        fn voiceprint_template_only_protects_its_enrolled_phrase() {
            let template = SpeakerTemplate {
                wake_embeddings: vec![vec![1.0, 0.0]],
                session_embeddings: vec![vec![1.0, 0.0]],
                target_speaker_embedding: None,
                personal_activity_reference: None,
                phrase: Some("小爱同学".into()),
                invalidated: false,
            };
            assert!(template_matches_phrase(&template, "小爱同学"));
            assert!(!template_matches_phrase(&template, "开始录音"));
            let invalidated = SpeakerTemplate {
                invalidated: true,
                ..template
            };
            assert!(!template_matches_phrase(&invalidated, "小爱同学"));
        }

        #[test]
        fn cosine_separates_aligned_and_opposed_vectors() {
            assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap(), 1.0);
            assert_eq!(cosine(&[1.0, 0.0], &[-1.0, 0.0]).unwrap(), -1.0);
        }

        #[test]
        fn enrollment_window_keeps_repeated_phrase_span_and_drops_outer_silence() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; SAMPLE_RATE as usize];
            samples.extend(vec![800i16; frame_samples * 32]);
            samples.extend(vec![0i16; frame_samples * 2]);
            samples.extend(vec![-900i16; frame_samples * 32]);
            samples.extend(vec![0i16; SAMPLE_RATE as usize]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let window = enrollment_speech_window(&pcm).expect("speech window");
            assert!(window.len() < pcm.len());
            assert!(window.len() >= SAMPLE_RATE as usize * 2 * ENROLLMENT_MIN_SECONDS);
        }

        #[test]
        fn enrollment_builds_bounded_long_and_short_owner_templates() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![800i16; frame_samples * 90];
            samples.extend(vec![-900i16; frame_samples * 60]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let windows = enrollment_template_windows(&pcm).expect("template windows");
            assert_eq!(windows.wake.len(), DUAL_TEMPLATE_WINDOWS_PER_BANK);
            assert_eq!(windows.session.len(), DUAL_TEMPLATE_WINDOWS_PER_BANK);
            assert!(windows
                .wake
                .iter()
                .all(|window| window.len() == ENROLLMENT_TEMPLATE_WINDOW_MS * 32));
            assert!(windows
                .session
                .iter()
                .all(|window| window.len() == ENROLLMENT_WAKE_STEP_SECONDS as usize * 32_000));
        }

        #[test]
        fn enrollment_accepts_three_minimum_length_phrases_with_volume_variation() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            // Real owner acceptance produced 600-800 ms phrases at visibly different
            // levels. Each 3-second slot must pass its own quality gate and then feed
            // both banks without a hidden second one-second-per-window requirement.
            let mut samples = vec![0i16; frame_samples * 5];
            samples.extend(vec![700i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 17]);
            samples.extend(vec![0i16; frame_samples * 10]);
            samples.extend(vec![4_000i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 12]);
            samples.extend(vec![0i16; frame_samples * 7]);
            samples.extend(vec![-700i16; frame_samples * 8]);
            samples.extend(vec![0i16; frame_samples * 15]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let windows = enrollment_template_windows(&pcm)
                .expect("three accepted phrase samples must build both template banks");
            assert_eq!(windows.wake.len(), 3);
            assert_eq!(windows.session.len(), 3);
            assert_ne!(
                windows.session, windows.wake,
                "session references retain surrounding context rather than repeated speech"
            );
            assert!(windows
                .wake
                .iter()
                .chain(&windows.session)
                .all(|window| window.len() >= VERIFICATION_MIN_SPEECH_MS * 32));
        }

        #[test]
        fn session_enrollment_retains_quiet_phonemes_and_late_speech_in_source_order() {
            let frame = |value: i16| vec![value; SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000];
            let mut sample = Vec::new();
            for value in [
                0, 0, 0, 0, 700, 800, 900, 700, 12, -12, 800, 900, 700, 800, 900, 700, 800, 900,
                700, 800, 900, 700, 1300, -1400, 0, 0, 0, 0, 0, 0,
            ] {
                sample.extend(frame(value));
            }
            let step = sample
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>();
            let pcm = step.repeat(ENROLLMENT_WAKE_STEPS as usize);
            let windows = enrollment_template_windows(&pcm).expect("three complete guided samples");
            // All original bytes from the first speech context through the last
            // speech context must reach every session reference, including the
            // quiet internal frames and late phonemes the wake exemplar omits.
            let expected = &step[300 * 32..2500 * 32];
            for span in &windows.session {
                assert_eq!(span, expected);
            }
            assert!(windows.wake.iter().all(|short| short.len() == 1000 * 32));
            assert!(windows
                .wake
                .iter()
                .all(|short| !short.chunks_exact(2).any(|b| b == (-1400i16).to_le_bytes())));
        }

        #[test]
        fn session_enrollment_uses_original_context_for_minimum_phrases_at_slot_edges() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut steps = Vec::new();
            for first in [0, 13, 24] {
                let mut samples = vec![0i16; frame_samples * 30];
                samples[first * frame_samples..(first + 6) * frame_samples].fill(800);
                steps.push(
                    samples
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect::<Vec<_>>(),
                );
            }
            let windows = enrollment_template_windows(&steps.concat())
                .expect("the existing 600 ms phrase gate must feed the 1 s encoder");
            for ((span, original), start_ms) in
                windows.session.iter().zip(&steps).zip([0, 1100, 2000])
            {
                assert_eq!(span, &original[start_ms * 32..(start_ms + 1000) * 32]);
                assert_eq!(
                    span.chunks_exact(2).filter(|b| *b != [0u8, 0]).count(),
                    600 * 16
                );
            }
        }

        #[test]
        fn enrollment_accepts_a_bounded_ble_transport_tail_shortfall() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = Vec::new();
            for amplitude in [800i16, 1_200, -900] {
                samples.extend(vec![0i16; frame_samples * 8]);
                samples.extend(vec![amplitude; frame_samples * 8]);
                samples.extend(vec![0i16; frame_samples * 14]);
            }
            // Reproduce the real 2026-08-23 capture: 287,360 bytes (8.98 s)
            // instead of the nominal 288,000 bytes, with zero missing packets.
            samples.truncate(samples.len() - SAMPLE_RATE as usize * 20 / 1_000);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            assert_eq!(pcm.len(), 287_360);
            let windows = enrollment_template_windows(&pcm)
                .expect("a packet-boundary tail shortfall must not discard three valid phrases");
            assert_eq!(windows.wake.len(), 3);
        }

        #[test]
        fn enrollment_rejects_more_than_the_bounded_transport_tail() {
            let required_samples = SAMPLE_RATE as usize * ENROLLMENT_WAKE_SECONDS as usize;
            let missing_samples =
                SAMPLE_RATE as usize * (ENROLLMENT_TRANSPORT_TAIL_TOLERANCE_MS + 50) / 1_000;
            let pcm = vec![800i16; required_samples - missing_samples]
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let error = enrollment_template_windows(&pcm)
                .expect_err("a genuinely early stop must remain rejected");
            assert!(error.contains("提前结束"), "unexpected error: {error}");
        }

        #[test]
        fn three_wake_steps_build_both_banks_without_a_fourth_prompt() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let samples = vec![800i16; frame_samples * 90];
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let windows = enrollment_template_windows(&pcm).expect("three-step enrollment");
            assert!(windows
                .wake
                .iter()
                .all(|window| i16::from_le_bytes([window[0], window[1]]) > 0));
            assert!(windows
                .session
                .iter()
                .all(|window| i16::from_le_bytes([window[0], window[1]]) > 0));
        }

        #[test]
        fn enrollment_rejects_a_missing_wake_phrase_step() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![800i16; frame_samples * 30];
            samples.extend(vec![0i16; frame_samples * 30]);
            samples.extend(vec![-900i16; frame_samples * 30]);
            samples.extend(vec![750i16; frame_samples * 60]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();

            let error = enrollment_template_windows(&pcm)
                .expect_err("every displayed wake step must provide its own sample");
            assert!(error.contains("第 2 次"), "unexpected error: {error}");
        }

        #[test]
        fn completed_enrollment_cancels_late_host_stop() {
            assert!(enrollment_capture_needs_host_stop(Some(
                CaptureState::Armed
            )));
            assert!(enrollment_capture_needs_host_stop(Some(
                CaptureState::Capturing
            )));
            assert!(!enrollment_capture_needs_host_stop(Some(
                CaptureState::Complete
            )));
            assert!(!enrollment_capture_needs_host_stop(Some(
                CaptureState::Error
            )));
            assert!(!enrollment_capture_needs_host_stop(None));
        }

        #[test]
        fn enrollment_rejects_a_short_sound_inside_a_long_capture() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 20];
            samples.extend(vec![900i16; frame_samples * 3]);
            samples.extend(vec![0i16; frame_samples * 20]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            assert!(enrollment_speech_window(&pcm).is_err());
        }

        #[test]
        fn verification_window_removes_outer_silence_and_keeps_model_floor() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 6];
            samples.extend(vec![900i16; frame_samples * 12]);
            samples.extend(vec![0i16; frame_samples * 6]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            let window = verification_speech_window(&pcm).expect("verification window");
            assert!(window.len() < pcm.len());
            assert!(window.len() >= VERIFICATION_MIN_SPEECH_MS * 32);
        }

        #[test]
        fn verification_window_keeps_model_floor_with_partial_live_tail_frame() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1000;
            let mut samples = vec![0i16; frame_samples * 13];
            samples.extend(vec![900i16; frame_samples * 8]);
            samples.extend(vec![900i16; frame_samples * 8 / 10]);
            let pcm = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>();
            assert_eq!(pcm.len() / 32, 2_180);
            let window = verification_speech_window(&pcm).expect("verification window");
            assert!(
                window.len() >= VERIFICATION_MIN_SPEECH_MS * 32,
                "partial live tail must not create a sub-model-floor owner window"
            );
        }

        #[test]
        fn session_speaker_padding_repeats_short_wake_audio_to_model_floor() {
            let short = vec![900i16; 760 * SAMPLE_RATE as usize / 1000]
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>();
            let speech = session_speaker_speech_window(&short)
                .expect("subsecond active wake must remain eligible for session tracking");
            assert_eq!(speech.len(), short.len());
            let padded = pad_session_speaker_pcm(speech);
            assert_eq!(padded.len(), VERIFICATION_MIN_SPEECH_MS * 32);
            assert_eq!(&padded[..short.len()], short.as_slice());
            assert_eq!(&padded[short.len()..], &short[..240 * 32]);
        }

        #[test]
        fn owner_verification_uses_real_pcm_without_silence_padding() {
            let pcm = vec![32u8; SAMPLE_RATE as usize * 2 * 1_100 / 1000];
            let samples = prepare_embedding_samples(&pcm).expect("1.1 second continuous speech");
            assert_eq!(samples.len(), SAMPLE_RATE as usize * 1_100 / 1000);
            // Product threshold may be looser than platform default for recall.
            assert!(VERIFICATION_THRESHOLD <= DEFAULT_SCORE_MILLI as f32 / 1000.0);
            assert!(VERIFICATION_THRESHOLD >= 0.35);
            let short_pcm = vec![32u8; SAMPLE_RATE as usize * 2 * 955 / 1000];
            assert!(prepare_embedding_samples(&short_pcm).is_err());
        }

        #[test]
        #[ignore = "requires downloaded upstream speaker fixtures"]
        fn runtime_separates_official_same_and_different_speakers() {
            let fixture = |name: &str| {
                let path = std::env::var(name).expect("fixture path env");
                let wav = fs::read(path).expect("read fixture");
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode 16 kHz mono fixture")
            };
            let runtime = match std::env::var("LISTENER_VOICEPRINT_EVAL_MODEL") {
                Ok(path) => {
                    let root = evaluation_runtime_root();
                    Arc::new(
                        SpeakerRuntime::load_model(&root, Path::new(&path))
                            .expect("load requested evaluation model"),
                    )
                }
                Err(_) => ensure_runtime().expect("load verified runtime"),
            };
            let enrolled = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER1_A"))
                .expect("speaker1 enrollment embedding");
            let same = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER1_B"))
                .expect("speaker1 query embedding");
            let different = runtime
                .embedding(&fixture("LISTENER_VOICEPRINT_SPEAKER2_A"))
                .expect("speaker2 query embedding");
            let same_score = cosine(&enrolled, &same).expect("same score");
            let different_score = cosine(&enrolled, &different).expect("different score");
            println!(
                "same_score={same_score:.6} different_score={different_score:.6} threshold={VERIFICATION_THRESHOLD:.3}"
            );
            assert!(same_score >= VERIFICATION_THRESHOLD);
            assert!(different_score < VERIFICATION_THRESHOLD);
            assert!(same_score > different_score);
        }

        #[test]
        #[ignore = "requires the enrolled owner credential and a consented local WAV"]
        fn runtime_reports_enrolled_owner_window_scores() {
            let path = std::env::var("LISTENER_VOICEPRINT_OWNER_WAV")
                .expect("LISTENER_VOICEPRINT_OWNER_WAV path");
            let wav = fs::read(path).expect("read consented owner fixture");
            let pcm =
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode 16 kHz mono fixture");
            let runtime = ensure_runtime().expect("load verified runtime");
            let template = {
                let mut state = STATE.lock();
                load_template_locked(&mut state);
                state.template.clone().expect("enrolled owner template")
            };
            let raw = runtime.embedding(&pcm).expect("raw embedding");
            let raw_score = template
                .session_embeddings
                .iter()
                .map(|enrolled| cosine(enrolled, &raw).expect("raw score"))
                .max_by(f32::total_cmp)
                .expect("raw score");
            let windows = verification_template_windows(&pcm).expect("verification windows");
            let mut processed_score = f32::NEG_INFINITY;
            for window in &windows {
                let candidate = runtime.embedding(window).expect("window embedding");
                for enrolled in &template.session_embeddings {
                    processed_score =
                        processed_score.max(cosine(enrolled, &candidate).expect("window score"));
                }
            }
            println!(
                "raw_score={raw_score:.6} processed_score={processed_score:.6} enrolled_templates={} candidate_windows={} threshold={VERIFICATION_THRESHOLD:.3}",
                template.session_embeddings.len(),
                windows.len()
            );
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationManifest {
            models: Vec<EvaluationModel>,
            enrollment_session_wavs: Vec<String>,
            samples: Vec<EvaluationSample>,
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationModel {
            name: String,
            path: String,
        }

        #[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationLabel {
            Owner,
            NonOwner,
        }

        #[derive(Debug, Deserialize)]
        struct EvaluationSample {
            id: String,
            path: String,
            label: EvaluationLabel,
            quality: EvaluationQuality,
        }

        #[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationQuality {
            Clean,
            Noisy,
            FarField,
        }

        impl EvaluationQuality {
            const ALL: [Self; 3] = [Self::Clean, Self::Noisy, Self::FarField];

            fn name(self) -> &'static str {
                match self {
                    Self::Clean => "clean",
                    Self::Noisy => "noisy",
                    Self::FarField => "far_field",
                }
            }
        }

        #[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum EvaluationDuration {
            Short,
            Medium,
            Long,
        }

        impl EvaluationDuration {
            const ALL: [Self; 3] = [Self::Short, Self::Medium, Self::Long];

            fn from_active_speech_ms(active_speech_ms: usize) -> Self {
                match active_speech_ms {
                    ..1_500 => Self::Short,
                    1_500..2_500 => Self::Medium,
                    _ => Self::Long,
                }
            }

            fn name(self) -> &'static str {
                match self {
                    Self::Short => "short_1000_1499_ms",
                    Self::Medium => "medium_1500_2499_ms",
                    Self::Long => "long_2500_plus_ms",
                }
            }
        }

        #[derive(Debug, Serialize)]
        struct EvaluationScore {
            id: String,
            label: EvaluationLabel,
            quality: EvaluationQuality,
            duration: EvaluationDuration,
            active_speech_ms: usize,
            speech_span_ms: usize,
            peak_rms: f32,
            reference_rms: f32,
            score: f32,
            consensus_score: f32,
            inference_ms: u128,
        }

        #[derive(Debug, Clone, Copy)]
        struct EvaluationMetrics {
            threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
        }

        impl EvaluationMetrics {
            fn passes(self) -> bool {
                self.owner_recall >= 0.95
                    && self.non_owner_suppression >= 0.90
                    && self.inference_p95_ms <= 300
            }
        }

        #[derive(Debug, Serialize)]
        struct EvaluationSliceReport {
            slice: String,
            applied_threshold: f32,
            diagnostic_threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
            pass: bool,
        }

        #[derive(Debug, Serialize)]
        struct ModelEvaluationReport {
            model: String,
            model_sha256: String,
            threshold: f32,
            owner_samples: usize,
            non_owner_samples: usize,
            owner_recall: f32,
            non_owner_suppression: f32,
            inference_p95_ms: u128,
            duration_slices: Vec<EvaluationSliceReport>,
            quality_slices: Vec<EvaluationSliceReport>,
            pass: bool,
            scores: Vec<EvaluationScore>,
        }

        fn resolve_evaluation_path(base: &Path, value: &str) -> std::path::PathBuf {
            let path = std::path::PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        }

        fn evaluation_runtime_root() -> std::path::PathBuf {
            match std::env::var("LISTENER_SPEAKER_EVAL_RUNTIME_ROOT") {
                Ok(path) => std::path::PathBuf::from(path),
                Err(_) => ensure_runtime_assets().expect("prepare speaker runtime assets"),
            }
        }

        fn evaluation_pcm(path: &Path) -> Vec<u8> {
            let wav = fs::read(path).unwrap_or_else(|err| {
                panic!(
                    "read consented evaluation fixture {}: {err}",
                    path.display()
                )
            });
            denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap_or_else(|err| {
                panic!(
                    "decode 16 kHz mono evaluation fixture {}: {err}",
                    path.display()
                )
            })
        }

        fn evaluation_embedding(runtime: &SpeakerRuntime, path: &Path) -> Vec<f32> {
            let pcm = evaluation_pcm(path);
            let speech = session_speaker_speech_window(&pcm).unwrap_or_else(|err| {
                panic!("quality-check evaluation fixture {}: {err}", path.display())
            });
            assert!(
                speech.len() >= VERIFICATION_MIN_SPEECH_MS * 32,
                "evaluation fixture must contain at least {} ms real speech: {}",
                VERIFICATION_MIN_SPEECH_MS,
                path.display()
            );
            runtime
                .embedding(speech)
                .unwrap_or_else(|err| panic!("embed evaluation fixture {}: {err}", path.display()))
        }

        fn evaluation_sample_score(
            runtime: &SpeakerRuntime,
            references: &[Vec<f32>],
            sample: &EvaluationSample,
            path: &Path,
        ) -> EvaluationScore {
            let pcm = evaluation_pcm(path);
            let speech = session_speaker_speech_window(&pcm).unwrap_or_else(|err| {
                panic!("quality-check evaluation fixture {}: {err}", path.display())
            });
            let (active_speech_ms, peak_rms, reference_rms) =
                session_speaker_signal_metrics(speech).unwrap_or_else(|err| {
                    panic!("measure evaluation fixture {}: {err}", path.display())
                });
            assert!(
                active_speech_ms >= VERIFICATION_MIN_SPEECH_MS,
                "evaluation fixture must contain at least {} ms real speech: {}",
                VERIFICATION_MIN_SPEECH_MS,
                path.display()
            );
            let model_pcm = pad_session_speaker_pcm(speech);
            let started = std::time::Instant::now();
            let embedding = runtime
                .embedding(&model_pcm)
                .unwrap_or_else(|err| panic!("embed evaluation fixture {}: {err}", path.display()));
            let inference_ms = started.elapsed().as_millis();
            let mut reference_scores = references
                .iter()
                .map(|reference| cosine(reference, &embedding).expect("matching dimensions"))
                .collect::<Vec<_>>();
            reference_scores.sort_by(|left, right| right.total_cmp(left));
            let score = *reference_scores.first().expect("enrollment references");
            let consensus_score = if reference_scores.len() >= 2 {
                (reference_scores[0] + reference_scores[1]) / 2.0
            } else {
                score
            };
            EvaluationScore {
                id: sample.id.clone(),
                label: sample.label,
                quality: sample.quality,
                duration: EvaluationDuration::from_active_speech_ms(active_speech_ms),
                active_speech_ms,
                speech_span_ms: speech.len() / 32,
                peak_rms,
                reference_rms,
                score,
                consensus_score,
                inference_ms,
            }
        }

        fn evaluation_metrics(scores: &[&EvaluationScore], threshold: f32) -> EvaluationMetrics {
            let owner_samples = scores
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::Owner)
                .count();
            let non_owner_samples = scores
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::NonOwner)
                .count();
            let accepted_owner = scores
                .iter()
                .filter(|sample| {
                    sample.label == EvaluationLabel::Owner && sample.score >= threshold
                })
                .count();
            let rejected_non_owner = scores
                .iter()
                .filter(|sample| {
                    sample.label == EvaluationLabel::NonOwner && sample.score < threshold
                })
                .count();
            let mut inference_ms = scores
                .iter()
                .map(|sample| sample.inference_ms)
                .collect::<Vec<_>>();
            inference_ms.sort_unstable();
            let inference_p95_ms = if inference_ms.is_empty() {
                0
            } else {
                let index = (inference_ms.len() * 95).div_ceil(100) - 1;
                inference_ms[index]
            };
            EvaluationMetrics {
                threshold,
                owner_samples,
                non_owner_samples,
                owner_recall: if owner_samples == 0 {
                    0.0
                } else {
                    accepted_owner as f32 / owner_samples as f32
                },
                non_owner_suppression: if non_owner_samples == 0 {
                    0.0
                } else {
                    rejected_non_owner as f32 / non_owner_samples as f32
                },
                inference_p95_ms,
            }
        }

        fn calibrated_evaluation_metrics(scores: &[&EvaluationScore]) -> EvaluationMetrics {
            (0..=1_000)
                .map(|step| evaluation_metrics(scores, step as f32 / 1_000.0))
                .max_by(|left, right| {
                    let left_pass = left.owner_recall >= 0.95 && left.non_owner_suppression >= 0.90;
                    let right_pass =
                        right.owner_recall >= 0.95 && right.non_owner_suppression >= 0.90;
                    left_pass
                        .cmp(&right_pass)
                        .then_with(|| {
                            (left.owner_recall + left.non_owner_suppression)
                                .total_cmp(&(right.owner_recall + right.non_owner_suppression))
                        })
                        .then_with(|| left.owner_recall.total_cmp(&right.owner_recall))
                        .then_with(|| right.threshold.total_cmp(&left.threshold))
                })
                .expect("threshold sweep")
        }

        fn evaluation_slice_report(
            name: String,
            scores: &[&EvaluationScore],
            applied_threshold: f32,
        ) -> EvaluationSliceReport {
            let applied = evaluation_metrics(scores, applied_threshold);
            let diagnostic = calibrated_evaluation_metrics(scores);
            let pass =
                applied.owner_samples >= 5 && applied.non_owner_samples >= 5 && applied.passes();
            EvaluationSliceReport {
                slice: name,
                applied_threshold,
                diagnostic_threshold: diagnostic.threshold,
                owner_samples: applied.owner_samples,
                non_owner_samples: applied.non_owner_samples,
                owner_recall: applied.owner_recall,
                non_owner_suppression: applied.non_owner_suppression,
                inference_p95_ms: applied.inference_p95_ms,
                pass,
            }
        }

        fn synthetic_evaluation_score(
            id: &str,
            label: EvaluationLabel,
            score: f32,
            inference_ms: u128,
        ) -> EvaluationScore {
            EvaluationScore {
                id: id.to_string(),
                label,
                quality: EvaluationQuality::Clean,
                duration: EvaluationDuration::Medium,
                active_speech_ms: 2_000,
                speech_span_ms: 2_000,
                peak_rms: 900.0,
                reference_rms: 600.0,
                score,
                consensus_score: score,
                inference_ms,
            }
        }

        #[test]
        fn evaluation_duration_buckets_keep_short_and_tail_risk_visible() {
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_000),
                EvaluationDuration::Short
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_499),
                EvaluationDuration::Short
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(1_500),
                EvaluationDuration::Medium
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(2_499),
                EvaluationDuration::Medium
            );
            assert_eq!(
                EvaluationDuration::from_active_speech_ms(2_500),
                EvaluationDuration::Long
            );
        }

        #[test]
        fn session_speaker_exclusion_uses_active_speech_not_pause_spanning_duration() {
            let frame_samples = SAMPLE_RATE as usize * ENROLLMENT_FRAME_MS / 1_000;
            let mut samples = vec![900i16; frame_samples * 3];
            samples.extend(vec![0i16; frame_samples * 6]);
            samples.extend(vec![-900i16; frame_samples * 3]);
            let pcm = samples
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>();
            let speech = session_speaker_speech_window(&pcm).expect("speech span");
            assert_eq!(speech.len() / 32, 1_200);
            let (real_speech_ms, _, _) =
                session_speaker_signal_metrics(speech).expect("active speech metrics");
            assert_eq!(real_speech_ms, 600);
            assert!(matches!(
                session_speaker_classification_for_evidence(0.20, real_speech_ms),
                SessionSpeakerClassification::Uncertain { .. }
            ));
        }

        #[test]
        fn r40_open_acceptance_wake_still_anchors_the_session_target() {
            // Drifted bank: every reference is far from the live wake voice.
            let bank = vec![vec![1.0, 0.0], vec![0.99, 0.02], vec![0.97, 0.04]];
            let (session, anchor) =
                session_target_with_wake_anchor(bank.clone(), vec![0.0, 1.0], false);
            assert_eq!(anchor, "open_acceptance");
            assert_eq!(session.len(), bank.len() + 1);
            assert_eq!(session.last(), Some(&vec![0.0, 1.0]));
            // The stored bank must stay untouched: purity is what keeps a
            // bystander wake from ever rewriting the enrolled template.
            assert_eq!(bank.len(), 3);

            // r40 shape: the wake speaker's body window max-cosines the union
            // at Target level through the fresh anchor, while the drifted bank
            // alone would have scored it strong NonTarget and cut the read.
            let body_window = vec![0.04, 0.999];
            let score = |profile: &[Vec<f32>]| {
                profile
                    .iter()
                    .map(|embedding| cosine(embedding, &body_window).unwrap())
                    .fold(f32::MIN, f32::max)
            };
            assert!(score(&bank) <= SESSION_SPEAKER_NON_TARGET_MAX_SCORE);
            assert!(score(&session) >= SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE);
        }

        #[test]
        fn union_lifts_owner_straddle_window_that_anchor_alone_scores_non_target() {
            // Measured 2026-09-21 (matrix, session a9a10419 t=5470): the
            // owner's mid-band body window scored 0.187 against the wake
            // anchor alone — under the 0.20 hard-NonTarget floor, which
            // latched the non-target veto, blocked the open-session body
            // recovery, and delivered "没有识别到语音" with 48 provider chars
            // heard. The same window scored 0.267 against the wake bank, so
            // the union kept it Uncertain. Real media tails measured
            // 0.005-0.14 against BOTH references — separation survives the
            // union; owner recall does not survive anchor-only.
            let anchor = vec![1.0, 0.0, 0.0];
            let wake_bank_slot = vec![0.906, 0.423, 0.0];
            // Owner body direction: 0.190 vs anchor (straddles below the
            // 0.20 floor), 0.270 vs the bank slot (lifts back above it) —
            // and still under 0.42, i.e. this is exactly a "bank failed the
            // wake" session.
            let owner_body = vec![0.19, 0.2315, 0.9538];
            // Real-media shape: far below the floor against every reference.
            let media_tail = vec![0.1, -0.9, 0.4243];
            let score = |profile: &[Vec<f32>], candidate: &[f32]| {
                profile
                    .iter()
                    .map(|embedding| cosine(embedding, candidate).unwrap())
                    .fold(f32::MIN, f32::max)
            };
            assert!(score(&[anchor.clone()], &owner_body) <= SESSION_SPEAKER_NON_TARGET_MAX_SCORE);
            let (union, label) =
                session_target_with_wake_anchor(vec![wake_bank_slot.clone()], anchor.clone(), false);
            assert_eq!(label, "open_acceptance");
            assert!(score(&union, &owner_body) > SESSION_SPEAKER_NON_TARGET_MAX_SCORE);
            assert!(score(&union, &owner_body) < VERIFICATION_THRESHOLD);
            assert!(score(&union, &media_tail) <= SESSION_SPEAKER_NON_TARGET_MAX_SCORE);
        }

        #[test]
        fn enrolled_match_and_open_acceptance_take_the_same_union_path() {
            let bank = vec![vec![1.0, 0.0], vec![0.99, 0.02], vec![0.97, 0.04]];
            let (matched, matched_anchor) =
                session_target_with_wake_anchor(bank.clone(), vec![0.0, 1.0], true);
            let (open, open_anchor) =
                session_target_with_wake_anchor(bank, vec![0.0, 1.0], false);
            assert_eq!(matched_anchor, "enrolled_match");
            assert_eq!(open_anchor, "open_acceptance");
            assert_eq!(matched.len(), open.len());
        }

        #[test]
        fn full_bank_cannot_take_a_wake_anchor() {
            let bank = vec![vec![1.0, 0.0]; SESSION_SPEAKER_MAX_EMBEDDINGS];
            let (session, anchor) =
                session_target_with_wake_anchor(bank.clone(), vec![0.0, 1.0], false);
            assert_eq!(anchor, "bank_only");
            assert_eq!(session.len(), SESSION_SPEAKER_MAX_EMBEDDINGS);
        }

        // 治本① 离线测量矩阵（按需运行，环境变量驱动，缺省跳过）：
        //   LISTENER_MATRIX_DIR=   目录含 bank.json（凭据库导出 [{name,purpose,b64}]）
        //                          与 manifest.json（[{wav,wake_end_ms,label}]）
        //   LISTENER_MODEL_ROOT=   含 3dspeaker onnx + onnxruntime.dll + sherpa-onnx-c-api.dll
        // 对每场会话：按真实 wake_end 复算锚嵌入；正文 1200ms 滚动窗（350ms 步进）过
        // 生产同款 speech-window/pad/embedding 链路；对四类参照打分：
        // anchor（tig 会话参照）/ wake_bank（注册唤醒银行）/ free_bank（注册自由语音银行）/
        // union（anchor+wake_bank，r40 银行认出时的并集）。用 --nocapture 看矩阵。
        #[test]
        fn offline_anchor_bank_matrix_from_env() {
            let Ok(dir) = std::env::var("LISTENER_MATRIX_DIR") else {
                return;
            };
            if dir.trim().is_empty() {
                return;
            }
            let model_root =
                std::env::var("LISTENER_MODEL_ROOT").expect("LISTENER_MODEL_ROOT required");
            let runtime = SpeakerRuntime::load(std::path::Path::new(&model_root))
                .expect("load speaker runtime from LISTENER_MODEL_ROOT");
            let decode = |b64: &str| -> Vec<f32> {
                BASE64
                    .decode(b64.trim())
                    .expect("base64 bank blob")
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect()
            };
            let bank: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(format!("{dir}\\bank.json")).expect("bank.json"),
            )
            .expect("bank.json json");
            let mut wake_bank: Vec<Vec<f32>> = Vec::new();
            let mut free_bank: Vec<Vec<f32>> = Vec::new();
            for slot in bank.as_array().expect("bank array") {
                let embedding = decode(slot["b64"].as_str().expect("b64"));
                match slot["purpose"].as_str().unwrap_or_default() {
                    "wake_phrase" => wake_bank.push(embedding),
                    "free_speech" => free_bank.push(embedding),
                    _ => {}
                }
            }
            println!(
                "bank: wake_slots={} free_slots={}",
                wake_bank.len(),
                free_bank.len()
            );
            let manifest: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(format!("{dir}\\manifest.json")).expect("manifest.json"),
            )
            .expect("manifest.json json");
            let max_cos = |refs: &[Vec<f32>], candidate: &[f32]| -> f32 {
                refs.iter()
                    .filter_map(|reference| cosine(reference, candidate).ok())
                    .fold(f32::MIN, f32::max)
            };
            for session in manifest.as_array().expect("manifest array") {
                let wav = session["wav"].as_str().expect("wav path");
                let wake_end_ms = session["wake_end_ms"].as_u64().expect("wake_end_ms") as usize;
                let label = session["label"].as_str().unwrap_or("?");
                let bytes = std::fs::read(wav).expect("read wav");
                let pcm = pcm_after_data_chunk(&bytes);
                let (anchor, ..) =
                    verified_wake_session_embedding(pcm, wake_end_ms as f32 / 1000.0)
                        .expect("anchor embedding");
                println!("== {label} wake_end_ms={wake_end_ms} anchor_dim={}", anchor.len());
                let body_start = ((wake_end_ms + 250) * 32).min(pcm.len()) & !1usize;
                let window_bytes = 1200 * 32;
                let step_bytes = 350 * 32;
                let mut index = body_start;
                let mut skipped = 0usize;
                while index + window_bytes <= pcm.len() {
                    let t_ms = index / 32;
                    let window = &pcm[index..index + window_bytes];
                    let Ok(speech) = session_speaker_speech_window(window) else {
                        skipped += 1;
                        index += step_bytes;
                        continue;
                    };
                    let model_pcm = pad_session_speaker_pcm(speech);
                    if let Ok(embedding) = runtime.embedding(&model_pcm) {
                        let a = cosine(&anchor, &embedding).unwrap_or(f32::MIN);
                        let w = max_cos(&wake_bank, &embedding);
                        let f = max_cos(&free_bank, &embedding);
                        let mut union: Vec<Vec<f32>> = wake_bank.clone();
                        union.push(anchor.clone());
                        let u = max_cos(&union, &embedding);
                        println!("  t={t_ms:>5}ms anchor={a:.3} wake_bank={w:.3} free_bank={f:.3} union={u:.3}");
                    }
                    index += step_bytes;
                }
                if skipped > 0 {
                    println!("  ({skipped} windows skipped: no speech window)");
                }
            }
        }

        fn pcm_after_data_chunk(bytes: &[u8]) -> &[u8] {
            if bytes.len() > 12 && &bytes[0..4] == b"RIFF" {
                let mut pos = 12;
                while pos + 8 <= bytes.len() {
                    if &bytes[pos..pos + 4] == b"data" {
                        let size = u32::from_le_bytes([
                            bytes[pos + 4],
                            bytes[pos + 5],
                            bytes[pos + 6],
                            bytes[pos + 7],
                        ]) as usize;
                        let start = pos + 8;
                        let end = (start + size).min(bytes.len());
                        return &bytes[start..end];
                    }
                    pos += 8;
                }
            }
            bytes
        }

        #[test]
        fn evaluation_calibration_prefers_a_threshold_that_meets_both_hard_gates() {
            let mut scores = Vec::new();
            for index in 0..20 {
                scores.push(synthetic_evaluation_score(
                    &format!("owner-{index}"),
                    EvaluationLabel::Owner,
                    if index == 0 { 0.48 } else { 0.70 },
                    (index + 1) as u128,
                ));
                scores.push(synthetic_evaluation_score(
                    &format!("non-owner-{index}"),
                    EvaluationLabel::NonOwner,
                    if index < 2 { 0.52 } else { 0.30 },
                    (index + 1) as u128,
                ));
            }
            let refs = scores.iter().collect::<Vec<_>>();
            let metrics = calibrated_evaluation_metrics(&refs);
            assert!(metrics.threshold > 0.52 && metrics.threshold <= 0.70);
            assert_eq!(metrics.owner_recall, 0.95);
            assert_eq!(metrics.non_owner_suppression, 1.0);
            assert_eq!(metrics.inference_p95_ms, 19);
            assert!(metrics.passes());
        }

        #[test]
        fn evaluation_slice_requires_five_samples_per_label() {
            let scores = vec![
                synthetic_evaluation_score("owner", EvaluationLabel::Owner, 0.8, 10),
                synthetic_evaluation_score("non-owner", EvaluationLabel::NonOwner, 0.2, 10),
            ];
            let refs = scores.iter().collect::<Vec<_>>();
            let report = evaluation_slice_report("quality:clean".to_string(), &refs, 0.5);
            assert_eq!(report.owner_recall, 1.0);
            assert_eq!(report.non_owner_suppression, 1.0);
            assert!(!report.pass);
        }

        #[test]
        #[ignore = "requires a consented, owner-labeled Listener speaker corpus"]
        fn runtime_evaluates_listener_labeled_speaker_corpus() {
            let manifest_path = std::path::PathBuf::from(
                std::env::var("LISTENER_SPEAKER_EVAL_MANIFEST")
                    .expect("LISTENER_SPEAKER_EVAL_MANIFEST path"),
            );
            let output_path = std::path::PathBuf::from(
                std::env::var("LISTENER_SPEAKER_EVAL_OUTPUT")
                    .expect("LISTENER_SPEAKER_EVAL_OUTPUT path"),
            );
            let manifest: EvaluationManifest = serde_json::from_slice(
                &fs::read(&manifest_path).expect("read speaker evaluation manifest"),
            )
            .expect("parse speaker evaluation manifest");
            let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            assert!(
                manifest.enrollment_session_wavs.len() >= DUAL_TEMPLATE_WINDOWS_PER_BANK,
                "evaluation requires at least three consented free-speech enrollment references"
            );
            let owner_samples = manifest
                .samples
                .iter()
                .filter(|sample| sample.label == EvaluationLabel::Owner)
                .count();
            let non_owner_samples = manifest.samples.len().saturating_sub(owner_samples);
            assert!(
                owner_samples >= 20,
                "evaluation requires at least 20 owner samples"
            );
            assert!(
                non_owner_samples >= 20,
                "evaluation requires at least 20 non-owner samples"
            );

            let runtime_root = evaluation_runtime_root();
            let mut reports = Vec::new();
            for model in &manifest.models {
                let model_path = resolve_evaluation_path(base, &model.path);
                let runtime = SpeakerRuntime::load_model(&runtime_root, &model_path)
                    .unwrap_or_else(|err| panic!("load evaluation model {}: {err}", model.name));
                let references = manifest
                    .enrollment_session_wavs
                    .iter()
                    .map(|path| {
                        evaluation_embedding(&runtime, &resolve_evaluation_path(base, path))
                    })
                    .collect::<Vec<_>>();
                let scores = manifest
                    .samples
                    .iter()
                    .map(|sample| {
                        evaluation_sample_score(
                            &runtime,
                            &references,
                            sample,
                            &resolve_evaluation_path(base, &sample.path),
                        )
                    })
                    .collect::<Vec<_>>();
                let all_scores = scores.iter().collect::<Vec<_>>();
                let overall = calibrated_evaluation_metrics(&all_scores);
                let duration_slices = EvaluationDuration::ALL
                    .into_iter()
                    .map(|duration| {
                        let slice = scores
                            .iter()
                            .filter(|sample| sample.duration == duration)
                            .collect::<Vec<_>>();
                        evaluation_slice_report(
                            format!("duration:{}", duration.name()),
                            &slice,
                            overall.threshold,
                        )
                    })
                    .collect::<Vec<_>>();
                let quality_slices = EvaluationQuality::ALL
                    .into_iter()
                    .map(|quality| {
                        let slice = scores
                            .iter()
                            .filter(|sample| sample.quality == quality)
                            .collect::<Vec<_>>();
                        evaluation_slice_report(
                            format!("quality:{}", quality.name()),
                            &slice,
                            overall.threshold,
                        )
                    })
                    .collect::<Vec<_>>();
                let pass = overall.owner_samples == owner_samples
                    && overall.non_owner_samples == non_owner_samples
                    && overall.passes()
                    && duration_slices.iter().all(|slice| slice.pass)
                    && quality_slices.iter().all(|slice| slice.pass);
                reports.push(ModelEvaluationReport {
                    model: model.name.clone(),
                    model_sha256: sha256(&model_path).expect("hash evaluation model"),
                    threshold: overall.threshold,
                    owner_samples: overall.owner_samples,
                    non_owner_samples: overall.non_owner_samples,
                    owner_recall: overall.owner_recall,
                    non_owner_suppression: overall.non_owner_suppression,
                    inference_p95_ms: overall.inference_p95_ms,
                    duration_slices,
                    quality_slices,
                    pass,
                    scores,
                });
            }
            assert!(
                !reports.is_empty(),
                "evaluation manifest contains no models"
            );
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).expect("create speaker evaluation output directory");
            }
            fs::write(
                &output_path,
                serde_json::to_vec_pretty(&reports).expect("encode speaker evaluation report"),
            )
            .expect("write speaker evaluation report");
            assert!(
                reports.iter().any(|report| report.pass),
                "no speaker model meets owner recall, non-owner suppression, and latency gates"
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::prepare_runtime_assets;
#[cfg(target_os = "windows")]
pub(crate) use platform::personal_activity_reference_for_phrase;
#[cfg(target_os = "windows")]
pub(crate) use platform::endpoint_identity_verifier_for_phrase;
#[cfg(target_os = "windows")]
pub(crate) use platform::endpoint_presence_references_for_phrase;
#[cfg(target_os = "windows")]
pub(crate) use platform::{native_word_profile_for_phrase, native_word_profile_is_current, native_word_binding_is_current, acquire_native_word_reference};
#[cfg(all(test, target_os = "windows"))]
pub(crate) use platform::session_profile_snapshot_from_wake;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) use platform::target_speaker_embedding_for_phrase;
#[cfg(target_os = "windows")]
pub use platform::{
    adapt_owner_session_bank, begin_enrollment_processing, cancel_enrollment, delete_template,
    enrollment_should_process, fail_enrollment, finish_enrollment, invalidate_for_phrase_change,
    is_enrolled_for_phrase, observe_enrollment_capture, observe_session_speaker,
    prepare_for_phrase, retry_enrollment_save, session_profile_from_wake, start_enrollment, status_for_phrase,
    take_enrollment_arm, verify,
};

#[cfg(all(test, target_os = "windows"))]
pub(crate) use platform::has_bound_template_without_mutation;

#[cfg(not(target_os = "windows"))]
pub fn status() -> VoiceprintStatus {
    VoiceprintStatus {
        available: false,
        runtime_ready: false,
        model_ready: false,
        enrolled: false,
        enrolled_phrase: None,
        requires_reenrollment: false,
        profile_state: VoiceProfileState::Unavailable,
        state: "unavailable".into(),
        progress: 0,
        capture_seconds_remaining: None,
        capture_step: None,
        capture_step_count: 3,
        capture_elapsed_ms: 0,
        step_speech_ms: vec![0; 3],
        signal_level: 0,
        capture_feedback: None,
        score: None,
        threshold: 0.5,
        error: None,
        failure_stage: None,
        can_retry_save: false,
        model_name: "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
        runtime_version: "1.13.1",
        local_only: true,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_enrolled_for_phrase(_wake_phrase: &str) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn take_enrollment_arm() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn begin_enrollment_processing() -> Option<u64> { None }

#[cfg(not(target_os = "windows"))]
pub fn observe_enrollment_capture(_pcm: &[u8]) {}

#[cfg(not(target_os = "windows"))]
pub fn enrollment_should_process() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn start_enrollment(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn cancel_enrollment(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Ok(status())
}

#[cfg(not(target_os = "windows"))]
pub fn delete_template() -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn finish_enrollment(_pcm: &[u8], _wake_phrase: &str, _generation: u64) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn retry_enrollment_save(_wake_phrase: &str) -> Result<VoiceprintStatus, String> {
    Err("voiceprint enrollment is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn fail_enrollment(_error: &str) {}

#[cfg(not(target_os = "windows"))]
pub fn verify(_pcm: &[u8], _wake_phrase: &str) -> Result<VerificationResult, String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn adapt_owner_wake_bank(
    _wake_phrase: &str,
    _verified_wake_embedding: &[f32],
) -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
pub fn adapt_owner_session_bank(
    _wake_phrase: &str,
    _body_embedding: &[f32],
) -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
pub fn session_profile_from_wake(
    _pcm: &[u8],
    _wake_end_seconds: f32,
    _wake_phrase: &str,
    _live_wake_matches_enrolled_owner: bool,
) -> Result<SessionSpeakerProfile, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn classify_session_speaker(
    _profile: &SessionSpeakerProfile,
    _pcm: &[u8],
) -> Result<SessionSpeakerClassification, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn observe_session_speaker(
    _profile: &SessionSpeakerProfile,
    _pcm: &[u8],
) -> Result<SessionSpeakerObservation, String> {
    Err("session speaker tracking is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_for_phrase(_wake_phrase: &str) -> Result<(), String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(not(target_os = "windows"))]
pub fn status_for_phrase(_wake_phrase: &str) -> VoiceprintStatus {
    status()
}

#[cfg(not(target_os = "windows"))]
pub fn invalidate_for_phrase_change(
    _previous_phrase: &str,
    _next_phrase: &str,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn prepare_runtime_assets() -> Result<(), String> {
    Err("voiceprint verification is currently available on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_identity_cannot_upgrade_an_inconclusive_mismatch() {
        use crate::personal_vad::PresenceState;
        use crate::speech_decision_kernel::TranscriptSpeakerEvidence;
        let mut sample = observation(SessionSpeakerClassification::NonTarget { score: 0.17 }, 2_000, [1.0, 0.0]);
        for evidence in [TranscriptSpeakerEvidence::Inconclusive, TranscriptSpeakerEvidence::ForeignTailHint] {
            sample.transcript_speaker_evidence = evidence;
            assert_eq!(sample.endpoint_presence_identity(), PresenceState::Unknown);
        }
        sample.transcript_speaker_evidence = TranscriptSpeakerEvidence::HardNonTarget;
        assert_eq!(sample.endpoint_presence_identity(), PresenceState::ConfirmedOther);
        sample.signal_quality_sufficient = false;
        assert_eq!(sample.endpoint_presence_identity(), PresenceState::Unknown);
        sample.classification = SessionSpeakerClassification::Target { score: 0.6 };
        assert_eq!(sample.endpoint_presence_identity(), PresenceState::Owner);
    }

    fn observation(
        classification: SessionSpeakerClassification,
        real_speech_ms: usize,
        embedding: [f32; 2],
    ) -> SessionSpeakerObservation {
        SessionSpeakerObservation {
            classification,
            real_speech_ms,
            signal_quality_sufficient: real_speech_ms >= SESSION_SPEAKER_MIN_NON_TARGET_MS,
            transcript_speaker_evidence:
                crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
            embedding: embedding.to_vec(),
        }
    }

    #[test]
    fn interference_quality_uses_existing_identity_signal_requirements() {
        assert!(!session_speaker_signal_quality_sufficient(1200, 194.0));
        assert!(!session_speaker_signal_quality_sufficient(700, 4000.0));
        assert!(session_speaker_signal_quality_sufficient(1000, 512.0));
        assert!(session_speaker_signal_quality_sufficient(1200, 3000.0));
        // A quiet real owner's audio is still processed and remains uncertain;
        // this quality check supplies no authority to delete or stop speech.
        assert!(matches!(
            session_speaker_classification_for_signal(0.65, 1200, 194.0),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    #[test]
    fn public_status_has_explicit_local_privacy_contract() {
        assert!(status_for_phrase("开始录音").local_only);
        assert!(status_for_phrase("开始录音").threshold > 0.0);
    }

    #[test]
    fn session_speaker_score_has_a_mixed_speech_uncertainty_band() {
        assert!(matches!(
            session_speaker_classification_for_score(0.55),
            SessionSpeakerClassification::Target { .. }
        ));
        assert!(matches!(
            session_speaker_classification_for_score(0.20),
            SessionSpeakerClassification::NonTarget { .. }
        ));
        for score in [0.294403, 0.307526, 0.331096, 0.336520] {
            assert!(matches!(
                session_speaker_classification_for_score(score),
                SessionSpeakerClassification::Uncertain { .. }
            ));
        }
        assert!(matches!(
            session_speaker_classification_for_score(0.38),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    /// 2026-09-23 重校（71f64ac1 矩阵）：本人的连续说话声对档案
    /// （TTS 银行 + 当会话唤醒锚点）实测 0.42–0.59。旧线 0.55 让本人
    /// 全程 Uncertain → owner_isolation 冻结在旁人插话后永不解除。
    /// 新线 0.40 下本人带必须是 Target；旁人带（当晚实测 <=0.28）
    /// 仍不得越线。
    #[test]
    fn session_speaker_owner_body_band_is_target_after_recalibration() {
        // 71f64ac1 真实重放序列（quality=true 窗）。
        for score in [0.434_754_2, 0.456_008_52, 0.507_647_45, 0.511_233_87, 0.541_903_14] {
            assert!(matches!(
                session_speaker_classification_for_signal(score, 1_200, 2_000.0),
                SessionSpeakerClassification::Target { .. }
            ));
        }
        // 当晚真实旁人序列上限。
        for score in [0.283_097_42, 0.199_755_15] {
            assert!(!matches!(
                session_speaker_classification_for_signal(score, 1_200, 2_000.0),
                SessionSpeakerClassification::Target { .. }
            ));
        }
        // 线边界：0.40 恰好确信，0.399 仍 Uncertain。
        assert!(matches!(
            session_speaker_classification_for_score(0.40),
            SessionSpeakerClassification::Target { .. }
        ));
        assert!(matches!(
            session_speaker_classification_for_score(0.399),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    #[test]
    fn adaptive_bank_refresh_keeps_anchor_and_skips_represented_exemplars() {
        let anchor = vec![1.0, 0.0, 0.0, 0.0];
        let churn = vec![0.0, 1.0, 0.0, 0.0];
        let third = vec![0.0, 0.0, 1.0, 0.0];
        let bank = vec![anchor.clone(), churn.clone(), third.clone()];

        // Already represented — identical slot, or the same direction at a
        // different recording level — must not rewrite the credential bank.
        assert!(!adaptive_bank_should_refresh(&bank, &churn));
        assert!(
            !adaptive_bank_should_refresh(&bank, &[0.0, 3.5, 0.0, 0.0]),
            "normalized similarity must ignore vector scale"
        );
        // Close-but-not-identical (cosine ~0.96) is still representation.
        assert!(!adaptive_bank_should_refresh(&bank, &[0.0, 2.0, 0.6, 0.0]));
        // A genuinely drifted voice (cosine 0.36-0.54 against every slot, the
        // installed 0.58→0.20 overnight shape) refreshes the churn slot.
        assert!(adaptive_bank_should_refresh(
            &bank,
            &[1.0, 1.5, 1.5, 1.5]
        ));
        // Guard rails: empty candidate, bank without a churn slot,
        // dimension mismatch.
        assert!(!adaptive_bank_should_refresh(&bank, &[]));
        assert!(!adaptive_bank_should_refresh(&[anchor], &[1.0, 1.5, 1.5, 1.5]));
        assert!(!adaptive_bank_should_refresh(
            &bank,
            &[0.0, 0.0, 0.0, 1.0, 0.0]
        ));
    }

    #[test]
    fn observation_embedding_accessor_exposes_bank_admission_payload() {
        let observation = observation(SessionSpeakerClassification::Target { score: 0.62 }, 1_200, [0.5, 0.25]);
        assert_eq!(observation.embedding(), &[0.5, 0.25]);
        // The confident-body admission floor must sit above the Target
        // classification threshold: a media-mixed 0.26-0.44 window can never
        // qualify even when classified Target on a clean day.
        assert!(ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE > SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE);
    }

    #[test]
    fn weak_target_band_is_uncertain_not_target() {
        // 2026-09-23 重校（71f64ac1 矩阵 + ef_session_score_bands_matrix）：
        // [0.42, 0.55) 现在是本人的正文带（对"TTS 银行+当会话唤醒锚点"
        // 档案实测 0.42–0.59，旧线 0.55 让本人全程 Uncertain、冻结永不
        // 解除）→ 必须判 Target。F3 fixture（2026-08-09 第二个人 0.426–0.58）
        // 是 pre-2026-09-18 TTS 银行时代的测量——anchor-era 旁人 <=0.11，
        // 2026-09-23 真实旁人 <=0.28。Uncertain 带相应收窄到 (0.20, 0.40)。
        for score in [0.42, 0.426, 0.47, 0.50, 0.54] {
            assert!(
                matches!(
                    session_speaker_classification_for_score(score),
                    SessionSpeakerClassification::Target { .. }
                ),
                "score {score} must be Target (owner body band, 2026-09-23 recalibration)"
            );
        }
        for score in [0.55, 0.58, 0.72] {
            assert!(
                matches!(
                    session_speaker_classification_for_score(score),
                    SessionSpeakerClassification::Target { .. }
                ),
                "score {score} must be confident Target"
            );
        }
        assert!(matches!(
            session_speaker_classification_for_score(0.399),
            SessionSpeakerClassification::Uncertain { .. }
        ));
        assert!(matches!(
            session_speaker_classification_for_score(0.34),
            SessionSpeakerClassification::Uncertain { .. }
        ));
        // 旁人带上限（当晚实测 0.283）仍必须 Uncertain。
        assert!(matches!(
            session_speaker_classification_for_score(0.283),
            SessionSpeakerClassification::Uncertain { .. }
        ));
    }

    #[test]
    fn short_body_speech_cannot_be_confidently_excluded_after_repeat_padding() {
        assert!(matches!(
            session_speaker_classification_for_evidence(0.20, 900),
            SessionSpeakerClassification::Uncertain { score } if score == 0.20
        ));
        assert!(matches!(
            session_speaker_classification_for_evidence(0.20, 1_000),
            SessionSpeakerClassification::NonTarget { score } if score == 0.20
        ));
        assert!(matches!(
            session_speaker_classification_for_evidence(0.55, 700),
            SessionSpeakerClassification::Target { score } if score == 0.55
        ));
    }

    #[test]
    fn flat_idle_noise_cannot_become_session_identity_evidence() {
        // Installed session b6c7bd31 ended with two flat idle windows at
        // peak_rms=70.7. The relative VAD counted all 1,200 ms as active and
        // their low cosine scores became NonTarget, cutting the transcript.
        for score in [0.176_024_97, 0.179_590_54, 0.80] {
            assert!(matches!(
                session_speaker_classification_for_signal(score, 1_200, 70.7),
                SessionSpeakerClassification::Uncertain { score: actual }
                    if actual == score
            ));
        }
        assert!(matches!(
            session_speaker_classification_for_signal(0.18, 1_200, 1_500.0),
            SessionSpeakerClassification::NonTarget { score } if score == 0.18
        ));
        assert!(matches!(
            session_speaker_classification_for_signal(0.60, 1_200, 1_500.0),
            SessionSpeakerClassification::Target { score } if score == 0.60
        ));
    }

    #[test]
    fn target_speaker_endpoint_short_high_energy_mismatch_is_typed_transcript_evidence() {
        use crate::speech_decision_kernel::{
            classify_transcript_speaker_evidence, TranscriptSpeakerEvidence,
        };

        assert_eq!(
            classify_transcript_speaker_evidence(0.077, 600, 771.7),
            TranscriptSpeakerEvidence::HardNonTarget
        );
        assert_eq!(
            classify_transcript_speaker_evidence(0.077, 599, 771.7),
            TranscriptSpeakerEvidence::ForeignTailHint
        );
        assert_eq!(
            classify_transcript_speaker_evidence(0.077, 300, 771.7),
            TranscriptSpeakerEvidence::ForeignTailHint
        );
        assert_eq!(
            classify_transcript_speaker_evidence(0.077, 800, 70.7),
            TranscriptSpeakerEvidence::Inconclusive
        );
        assert_eq!(
            classify_transcript_speaker_evidence(0.172, 800, 771.7),
            TranscriptSpeakerEvidence::Inconclusive
        );
    }

    #[test]
    fn enrolled_session_adds_one_verified_wake_exemplar_without_becoming_adaptive() {
        let mut embeddings = vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.8, 0.2]];
        assert!(append_verified_wake_session_exemplar(
            &mut embeddings,
            vec![0.7, 0.3],
        ));
        assert_eq!(embeddings.len(), SESSION_SPEAKER_MAX_EMBEDDINGS);
        assert_eq!(embeddings.last(), Some(&vec![0.7, 0.3]));

        assert!(
            !append_verified_wake_session_exemplar(&mut embeddings, vec![0.6, 0.4]),
            "the session bank must stay bounded"
        );
        let profile = SessionSpeakerProfile {
            embeddings: Arc::new(embeddings),
            adaptive: false,
        };
        assert!(!profile.is_adaptive());
    }

    #[test]
    fn verified_wake_exemplar_rejects_empty_or_wrong_dimension_banks() {
        let mut empty = Vec::new();
        assert!(!append_verified_wake_session_exemplar(
            &mut empty,
            vec![1.0, 0.0],
        ));

        let mut enrolled = vec![vec![1.0, 0.0]];
        assert!(!append_verified_wake_session_exemplar(
            &mut enrolled,
            vec![1.0],
        ));
        assert_eq!(enrolled.len(), 1);
    }

    #[test]
    fn session_profile_adapts_only_after_consecutive_high_confidence_and_stable_coverage() {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(
            2_600,
            observation(
                SessionSpeakerClassification::Target { score: 0.52 },
                1_000,
                [0.9, 0.1],
            ),
        );
        assert_eq!(gate.promote_covered(&mut profile, Some(3_000)), 0);
        assert_eq!(profile.embeddings.len(), 1);

        gate.note(
            3_000,
            observation(
                SessionSpeakerClassification::Target { score: 0.51 },
                1_200,
                [0.8, 0.2],
            ),
        );
        assert_eq!(gate.promote_covered(&mut profile, Some(2_800)), 1);
        assert_eq!(profile.embeddings.len(), 2);
        assert_eq!(gate.promote_covered(&mut profile, Some(3_000)), 1);
        assert_eq!(profile.embeddings.len(), 3);
    }

    #[test]
    fn unenrolled_session_bootstraps_from_three_consecutive_full_target_windows_without_cloud_coverage(
    ) {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        for (audio_end_ms, score, embedding) in [
            (3_000, 0.483, [0.90, 0.10]),
            (3_400, 0.458, [0.85, 0.15]),
            (3_800, 0.435, [0.80, 0.20]),
        ] {
            gate.note(
                audio_end_ms,
                observation(
                    SessionSpeakerClassification::Target { score },
                    1_200,
                    embedding,
                ),
            );
        }

        assert_eq!(gate.promote_covered(&mut profile, None), 3);
        assert_eq!(profile.embeddings.len(), 4);
        assert_eq!(gate.promote_covered(&mut profile, None), 0);
    }

    #[test]
    fn uncertain_or_confirmed_non_target_prevents_local_body_bootstrap() {
        let target = |score, embedding| {
            observation(
                SessionSpeakerClassification::Target { score },
                1_200,
                embedding,
            )
        };
        let mut interrupted_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut interrupted = SessionSpeakerAdaptationGate::default();
        interrupted.note(3_000, target(0.48, [0.90, 0.10]));
        interrupted.note(
            3_400,
            observation(
                SessionSpeakerClassification::Uncertain { score: 0.38 },
                1_200,
                [0.75, 0.25],
            ),
        );
        interrupted.note(3_800, target(0.47, [0.88, 0.12]));
        interrupted.note(4_200, target(0.46, [0.86, 0.14]));
        assert_eq!(
            interrupted.promote_covered(&mut interrupted_profile, None),
            0
        );

        let mut switched_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut switched = SessionSpeakerAdaptationGate::default();
        for audio_end_ms in [3_000, 3_400] {
            switched.note(
                audio_end_ms,
                observation(
                    SessionSpeakerClassification::NonTarget { score: 0.20 },
                    1_200,
                    [0.0, 1.0],
                ),
            );
        }
        for (index, audio_end_ms) in [3_800, 4_200, 4_600].into_iter().enumerate() {
            switched.note(audio_end_ms, target(0.48 - index as f32 * 0.01, [0.9, 0.1]));
        }
        assert_eq!(switched.promote_covered(&mut switched_profile, None), 0);

        let mut enrolled_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: false,
        };
        let mut enrolled = SessionSpeakerAdaptationGate::default();
        for audio_end_ms in [3_000, 3_400, 3_800] {
            enrolled.note(audio_end_ms, target(0.60, [0.9, 0.1]));
        }
        assert_eq!(enrolled.promote_covered(&mut enrolled_profile, None), 0);
        assert_eq!(enrolled_profile.embeddings.len(), 1);
    }

    #[test]
    fn short_target_window_does_not_break_body_bootstrap_streak() {
        // 2026-08-07 07:23 「你看一」事故：800/600ms 的 Target 短窗把 bootstrap
        // 连击清零，body exemplar 从未建立，后段声纹漂移冻结在 8 字。
        // 短 Target 窗不算一格，但不得清空连击（LST-REC-025 只让
        // Uncertain/NonTarget 打断）。
        let target_full = |score, embedding| {
            observation(
                SessionSpeakerClassification::Target { score },
                1_200,
                embedding,
            )
        };
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(3_000, target_full(0.48, [0.90, 0.10]));
        // 短 Target 窗（不足 1000ms）插在全窗之间：连击必须保留。
        gate.note(
            3_200,
            observation(
                SessionSpeakerClassification::Target { score: 0.47 },
                600,
                [0.88, 0.12],
            ),
        );
        gate.note(3_400, target_full(0.46, [0.86, 0.14]));
        gate.note(3_800, target_full(0.45, [0.84, 0.16]));
        assert_eq!(gate.promote_covered(&mut profile, None), 3);
        assert_eq!(profile.embeddings.len(), 4);
    }

    #[test]
    fn uncertain_low_short_and_enrolled_observations_cannot_pollute_profiles() {
        let mut adaptive_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let mut gate = SessionSpeakerAdaptationGate::default();
        gate.note(
            1_000,
            observation(
                SessionSpeakerClassification::Target { score: 0.60 },
                900,
                [0.9, 0.1],
            ),
        );
        gate.note(
            1_400,
            observation(
                SessionSpeakerClassification::Uncertain { score: 0.38 },
                1_200,
                [0.8, 0.2],
            ),
        );
        gate.note(
            1_800,
            observation(
                SessionSpeakerClassification::Target { score: 0.49 },
                1_200,
                [0.7, 0.3],
            ),
        );
        gate.note(
            2_200,
            observation(
                SessionSpeakerClassification::NonTarget { score: 0.20 },
                1_200,
                [0.0, 1.0],
            ),
        );
        assert_eq!(gate.promote_covered(&mut adaptive_profile, Some(3_000)), 0);
        assert_eq!(adaptive_profile.embeddings.len(), 1);

        let mut enrolled_profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: false,
        };
        let candidates = [observation(
            SessionSpeakerClassification::Target { score: 0.90 },
            1_200,
            [0.9, 0.1],
        )];
        assert_eq!(
            adapt_session_speaker_profile(&mut enrolled_profile, &candidates),
            0
        );
        assert_eq!(enrolled_profile.embeddings.len(), 1);
    }

    #[test]
    fn session_profile_adaptation_is_bounded_to_three_body_exemplars() {
        let mut profile = SessionSpeakerProfile {
            embeddings: Arc::new(vec![vec![1.0, 0.0]]),
            adaptive: true,
        };
        let candidates = (0..8)
            .map(|index| {
                observation(
                    SessionSpeakerClassification::Target { score: 0.75 },
                    1_200,
                    [0.9 - index as f32 * 0.01, 0.1 + index as f32 * 0.01],
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(adapt_session_speaker_profile(&mut profile, &candidates), 3);
        assert_eq!(profile.embeddings.len(), 4);
        assert_eq!(adapt_session_speaker_profile(&mut profile, &candidates), 0);
    }
}

#[cfg(all(test,target_os="windows"))]
mod existing_background_projection_probe {
    use super::*;
    use serde_json::{Value,json};
    use sha2::{Digest,Sha256};
    #[test]
    #[ignore="Explicit private exact-source embedding diagnostic; no new model, cloud or editor"]
    fn existing_source_embedding_background_projection() {
        let path=std::path::PathBuf::from(std::env::var("LISTENER_EMBEDDING_PROJECTION_FIXTURE").unwrap());
        let fixture:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let phrase=fixture["phrase"].as_str().unwrap();assert!(has_bound_template_without_mutation(phrase));
        let profile=native_word_profile_for_phrase(phrase).unwrap().unwrap();assert!(!profile.has_qualified_reference);
        let mut observations=Vec::new();let mut measurements=Vec::new();
        for row in fixture["inputs"].as_array().unwrap() {
            let bytes=std::fs::read(row["pcm"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&bytes)),row["pcmSha256"].as_str().unwrap());
            let start=row["startMs"].as_u64().unwrap() as usize*32;let end=row["endMs"].as_u64().unwrap() as usize*32;
            let began=std::time::Instant::now();let observation=profile.observe(&bytes[start..end]).unwrap();
            measurements.push(json!({"id":row["id"],"sourceRangeMs":[start/32,end/32],"assessmentOnly":row["assessmentOnly"],
                "baseClassification":format!("{:?}",observation.classification),"realSpeechMs":observation.real_speech_ms,
                "quality":observation.signal_quality_sufficient,"computeMs":began.elapsed().as_secs_f64()*1000.0}));
            observations.push(observation);
        }
        let cosine=|a:&[f32],b:&[f32]|normalized_cosine(a,b).map(f64::from);
        let unit=|v:&[f32]| {let n=v.iter().map(|x|(*x as f64).powi(2)).sum::<f64>().sqrt();assert!(n>0.0);v.iter().map(|x|(*x as f64/n) as f32).collect::<Vec<_>>()};
        let phone=fixture["inputs"].as_array().unwrap().iter().position(|r|r["role"]=="backgroundReference").unwrap();
        let background=unit(&observations[phone].embedding);
        let residual=|v:&[f32]| {let v=unit(v);let dot=v.iter().zip(&background).map(|(a,b)|a*b).sum::<f32>();v.iter().zip(&background).map(|(a,b)|a-dot*b).collect::<Vec<_>>()};
        let ids:Vec<_>=fixture["inputs"].as_array().unwrap().iter().map(|r|r["id"].clone()).collect();
        let mut relations=Vec::new();let mut residuals=Vec::new();
        for (i,a) in observations.iter().enumerate() {
            let ra=residual(&a.embedding);let norm=ra.iter().map(|x|(*x as f64).powi(2)).sum::<f64>().sqrt();
            let bank_raw:Vec<_>=profile.base.embeddings.iter().map(|v|cosine(&a.embedding,v)).collect();
            let bank_residual:Vec<_>=profile.base.embeddings.iter().map(|v|cosine(&ra,&residual(v))).collect();
            residuals.push(json!({"id":ids[i],"residualNorm":norm,"phoneCosine":cosine(&a.embedding,&background),
                "originalBankCosines":bank_raw,"phoneOrthogonalBankCosines":bank_residual}));
            for (j,b) in observations.iter().enumerate().take(i+1) {
                relations.push(json!({"a":ids[i],"b":ids[j],"cosine":cosine(&a.embedding,&b.embedding),"phoneOrthogonalCosine":cosine(&ra,&residual(&b.embedding))}));
            }
        }
        std::fs::write(path.parent().unwrap().join("result.json"),serde_json::to_vec_pretty(&json!({
            "scope":fixture["scope"],"measurements":measurements,"relations":relations,"residuals":residuals,
            "sourceEncodingCount":observations.len(),"newWeights":false,"newModel":false,"vectorsExported":false,
            "profileWrites":0,"cloudCalls":0,"actualEditor":false,"actualStop":false,
        })).unwrap()).unwrap();
    }
}
