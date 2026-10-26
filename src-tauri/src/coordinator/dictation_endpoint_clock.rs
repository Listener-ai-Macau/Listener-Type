/// Wall-clock companion to the provider audio clock.
///
/// The provider can publish a complete, speaker-attributed preview and then
/// stop advancing its covered-audio timestamp while local low-level noise
/// continues to trip the energy detector. In that state the ordinary endpoint
/// waits for the firmware fallback even though the user-visible text has been
/// settled for several seconds. Arm this clock only for a visible body with a
/// stable cloud target. Provisional speech cancels it, and a newer target
/// boundary rearms it, so it cannot race an actively growing owner utterance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndpointStopPhase {
    Proposed,
    Sending,
    Sent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EndpointStopTicket {
    session_id: SessionId,
    proposal_id: u64,
    endpoint_generation: u64,
    local_vad_revision: u64,
    activity_epoch: u64,
    manual_vad_guard: bool,
}

#[derive(Debug, Default)]
struct EndpointStopDispatchState {
    next_proposal_id: u64,
    ticket: Option<(EndpointStopPhase, EndpointStopTicket)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BodySubmissionSequence(u64);

#[derive(Debug)]
struct PendingBodyDispatch {
    sequence: BodySubmissionSequence,
    dispatched_at: Instant,
    audio_boundary_ms: Option<u64>,
    owner_boundary_ms: Option<u64>,
    personal_continuation_boundary_ms: Option<u64>,
}

#[derive(Debug)]
struct PendingBodyReceipt {
    sequence: BodySubmissionSequence,
    ticket: crate::insertion::ClipboardTicket,
    receipt: crate::insertion::PasteReceiptHandle,
    personal_continuation_boundary_ms: Option<u64>,
}

#[derive(Debug)]
struct PendingOwnerDeliveryHold {
    key: String,
    recognition_pending: bool,
}

impl EndpointStopDispatchState {
    fn propose(
        &mut self,
        session_id: SessionId,
        endpoint_generation: u64,
        local_vad_revision: u64,
        activity_epoch: u64,
        manual_vad_guard: bool,
    ) -> Option<EndpointStopTicket> {
        if self.ticket.is_some() {
            return None;
        }
        self.next_proposal_id = self.next_proposal_id.wrapping_add(1).max(1);
        let ticket = EndpointStopTicket {
            session_id,
            proposal_id: self.next_proposal_id,
            endpoint_generation,
            local_vad_revision,
            activity_epoch,
            manual_vad_guard,
        };
        self.ticket = Some((EndpointStopPhase::Proposed, ticket));
        Some(ticket)
    }

    fn is_current(&self, ticket: EndpointStopTicket) -> bool {
        self.ticket.is_some_and(|(_, current)| current == ticket)
    }

    fn begin_sending(&mut self, ticket: EndpointStopTicket) -> bool {
        if self
            .ticket
            .is_some_and(|(phase, current)| phase == EndpointStopPhase::Proposed && current == ticket)
        {
            self.ticket = Some((EndpointStopPhase::Sending, ticket));
            true
        } else {
            false
        }
    }

    fn cancel_if_current(&mut self, ticket: EndpointStopTicket) -> bool {
        if self.ticket.is_some_and(|(phase, current)| {
            phase != EndpointStopPhase::Sent && current == ticket
        }) {
            self.ticket = None;
            true
        } else {
            false
        }
    }

    fn mark_sent_if_current(&mut self, ticket: EndpointStopTicket) -> bool {
        if self
            .ticket
            .is_some_and(|(phase, current)| phase == EndpointStopPhase::Sending && current == ticket)
        {
            self.ticket = Some((EndpointStopPhase::Sent, ticket));
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
struct SettledTargetEndpointClock {
    stop_proposed: bool,
    generation: u64,
    armed_target_end_ms: Option<u64>,
    armed_at: Option<Instant>,
    armed_from_visible_body_fallback: bool,
    /// The last body chunk submitted to the focused input. The user's
    /// continuation window starts here, rather than at the preceding VAD
    /// silence edge or at a later provider preview revision.
    last_body_delivery_at: Option<Instant>,
    /// Each receipt stays paired with the body dispatch that created it. A
    /// newer dispatch cannot consume an older completion, and punctuation
    /// cannot create a sequence or rearm the continuation clock.
    next_body_submission_sequence: u64,
    last_body_submission_sequence: Option<BodySubmissionSequence>,
    pending_body_dispatch: Option<PendingBodyDispatch>,
    pending_body_receipts: Vec<PendingBodyReceipt>,
    /// A permitted body candidate can reach the endpoint before delivery. In
    /// personal mode its settling allowance belongs to observed owner audio,
    /// not to every changing raw provider key (which may be background speech).
    pending_owner_delivery: Option<PendingOwnerDeliveryHold>,
    last_pending_personal_continuation_end_ms: Option<u64>,
    last_body_delivery_audio_ms: Option<u64>,
    /// A local Target decision for post-delivery audio whose signal quality
    /// has not yet qualified it as owner speech. It can briefly veto STOP
    /// while the next classifier window settles, but cannot rearm the clock.
    tentative_owner_continuation_at: Option<Instant>,
    tentative_owner_continuation_audio_ms: Option<u64>,
    /// VAD turn containing fresh qualified owner speech, or beginning inside
    /// the last delivered body's continuation window. Carry this across the
    /// wake/body handoff even before the first paste. Unresolved continuation
    /// only vetoes STOP; it never becomes a transcript owner watermark.
    tentative_owner_vad_epoch: Option<u64>,
    tentative_owner_quiet_audio_ms: Option<u64>,
    /// The accepted wake has not produced any body text.  This is not a
    /// second endpoint: it is an explicit mode of the same controller, armed
    /// from the automatic-wake guard's original clock.  Wake-phrase audio and
    /// provider bookkeeping cannot rearm or block this bounded abandonment.
    automatic_no_body_armed: bool,
    automatic_no_body_initial_audio_ms: u64,
    automatic_no_body_last_speech_ms: Option<u64>,
    /// Sticky marker for the whole session: this clock was armed by an
    /// accepted automatic wake.  `automatic_no_body_armed` above clears when
    /// the first body text appears; this marker survives that transition so
    /// body-stage policy (the bounded open-clause continuation) can tell
    /// automatic wake sessions apart from manual hotkey sessions.
    automatic_wake_session: bool,
    /// Wall-clock moment of the last POSITIVE owner-dictation evidence: a
    /// local qualified/Target watermark advance, visible preview growth, or
    /// the session's first arm.  See EMBEDDED_OWNER_POSITIVE_EVIDENCE_BUDGET_MS.
    last_positive_owner_evidence_at: Option<Instant>,
    /// A provisional cloud tail normally cancels the owner endpoint. Preserve
    /// its original deadline out of band so a later sustained-local-other
    /// decision can restore that deadline instead of starting another wait.
    paused_armed_target_end_ms: Option<u64>,
    paused_armed_at: Option<Instant>,
    latest_update: Option<crate::asr::volcengine::TargetSpeakerUpdate>,
    /// Monotonic arrival time of the evidence above. Endpoint decisions must
    /// not keep treating a frozen provider snapshot as live owner speech.
    /// Once this expires, the session reducer converts the snapshot into a
    /// bounded provider-stall observation and evaluates it exactly once from
    /// the watchdog.
    latest_update_at: Option<Instant>,
    /// PCM onset of the current VAD epoch, retained after PendingSpeech
    /// becomes Speech. A late owner observation for the previous epoch must
    /// not adopt a new room-speech turn.
    latest_local_vad_epoch_start_ms: Option<u64>,
    /// Whether the latest visible preview ends at a sentence boundary. A
    /// provider row can stop advancing for several seconds in the middle of a
    /// long clause; the settled-text wall clock must not cut that open body
    /// while local speech is still reaching the microphone.
    latest_visible_body_ends_terminal: Option<bool>,
    /// Peak visible length across the current run of open previews. Speaker
    /// attribution can replace a long open provider preview with a much
    /// shorter terminal supplement; the pre-replacement length is the signal
    /// that distinguishes that risky shrink from an ordinary short command.
    open_body_peak_visible_chars: usize,
    /// A short terminal supplement can close an earlier open preview while
    /// the provider has skipped words between them. In manual/no-voiceprint
    /// recording, bridge that suspicious transition for a bounded interval;
    /// ordinary terminal-first short commands keep the normal 900 ms clock.
    manual_terminal_bridge_until: Option<Instant>,
    manual_terminal_bridge_rearm_pending: bool,
    /// Manual body endpointing has a bounded second stage for an open clause.
    /// The first one-second silence window is only a qualification point; an
    /// unfinished visible body may keep capture alive until this deadline.
    continuation_pending_until: Option<Instant>,
    continuation_pending_anchor_speech_end_ms: Option<u64>,
    continuation_pending_anchor_canonical_speech_serial: Option<u64>,
    continuation_cutoff_reached: bool,
    pending_was_seen: bool,
    /// The sole visible-recording stop authority. All callback evidence is
    /// reduced into this controller; no provider or firmware callback owns a
    /// second endpoint timer.
    product_endpoint: crate::speech_decision_kernel::OwnerEndpointController,
    /// Revision of the sidecar VAD snapshot consumed by the reducer. A new
    /// VAD state at the same PCM edge is still a new observation.
    latest_local_vad_revision: Option<u64>,
    /// `Some` is supplied only for the manual-body local-VAD endpoint path.
    /// It keeps the bounded continuation policy away from automatic wake and
    /// enrolled-speaker sessions until those paths have their own evidence.
    manual_vad_guard: bool,
    latest_local_vad_evidence: Option<crate::asr::volcengine::LocalSpeechEvidence>,
    /// Fresh, monotonic silence proof for the personal-worker failure path.
    /// Re-reading the same snapshot cannot refresh its observation timestamp.
    local_vad_stop_observation: Option<LocalVadStopObservation>,
    canonical_speech_serial: u64,
    last_visible_body_signature: Option<(bool, usize)>,
    /// Last local speech edge used to renew the firmware endpoint.  Provider
    /// callbacks can repeat the same snapshot; dedupe it so a stale snapshot
    /// cannot keep recording alive indefinitely.
    last_firmware_lease_owner_speech_ms: Option<u64>,
    /// Last host-driven VREC:SPEECH sent to keep firmware's 1s silence
    /// fallback from cutting a session the product hang clock still owns.
    last_host_hang_lease_at: Option<Instant>,
    /// A bounded diagnostic latch: when the endpoint deadline is reached but
    /// evidence keeps it in Hold/CatchingUp, report the first reason for this
    /// generation only.  This avoids per-50ms log spam while making a stuck
    /// endpoint distinguishable from an actively growing owner utterance.
    pending_due_hold_diagnostic: Option<(u64, &'static str)>,
    last_reported_due_hold_diagnostic: Option<(u64, &'static str)>,
    /// Independent presence controls stop only; it cannot authenticate text.
    personal_presence_endpoint: Option<crate::personal_vad::PresenceEndpoint>,
}

/// A new VAD epoch is unclassified until personal analysis has consumed its
/// source. Pending onset uses an exclusive boundary; a snapshot that missed
/// the onset uses the completed-through boundary of its first observation.
#[derive(Clone, Copy, Debug)]
enum LocalVadEpochSourceFence {
    Onset(u64),
    ObservedThrough(u64),
}

impl LocalVadEpochSourceFence {
    fn for_epoch(evidence: crate::asr::volcengine::LocalSpeechEvidence) -> Option<Self> {
        (evidence.activity_epoch > 0).then(|| {
            evidence.pending_speech_start_ms.map_or(
                Self::ObservedThrough(evidence.analyzed_through_ms), Self::Onset,
            )
        })
    }

    fn is_unobserved(self, analyzed_end_ms: u64) -> bool {
        match self {
            Self::Onset(start_ms) => analyzed_end_ms <= start_ms,
            Self::ObservedThrough(end_ms) => analyzed_end_ms < end_ms,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LocalVadStopObservation {
    evidence: crate::asr::volcengine::LocalSpeechEvidence,
    observed_at: Instant,
    silence_since_ms: u64,
    epoch_source_fence: Option<LocalVadEpochSourceFence>,
    valid: bool,
}

impl LocalVadStopObservation {
    fn new(evidence: crate::asr::volcengine::LocalSpeechEvidence, observed_at: Instant) -> Self {
        use crate::asr::volcengine::LocalSpeechActivityState;
        Self {
            evidence,
            observed_at,
            silence_since_ms: if evidence.state == LocalSpeechActivityState::NonSpeech
                && evidence.activity_epoch == 0
            {
                evidence.last_detected_speech_end_ms.unwrap_or_default()
            } else {
                evidence.analyzed_through_ms
            },
            epoch_source_fence: LocalVadEpochSourceFence::for_epoch(evidence),
            valid: evidence.revision > 0 && evidence.state != LocalSpeechActivityState::Unknown,
        }
    }

    fn observe(&mut self, evidence: crate::asr::volcengine::LocalSpeechEvidence, observed_at: Instant) {
        use crate::asr::volcengine::LocalSpeechActivityState;
        if evidence == self.evidence {
            return;
        }
        if evidence.revision <= self.evidence.revision
            || evidence.analyzed_through_ms < self.evidence.analyzed_through_ms
            || evidence.activity_epoch < self.evidence.activity_epoch
        {
            // An out-of-order callback cannot revive old quiet evidence after
            // a speech onset. Retain the high-water revision until fresh input.
            self.valid = false;
            return;
        }
        if evidence.activity_epoch > self.evidence.activity_epoch {
            self.epoch_source_fence = LocalVadEpochSourceFence::for_epoch(evidence);
        }
        // Same-epoch background speech cannot continually renew this fence.
        self.silence_since_ms = self.silence_since_ms.max(
            if evidence.state == LocalSpeechActivityState::NonSpeech {
                if evidence.activity_epoch > self.evidence.activity_epoch
                    || matches!(self.evidence.state, LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Unknown)
                {
                    // Snapshot consumers can miss an entire short onset. Its
                    // epoch still advances even if canonical Speech never did.
                    // Start proven quiet at the first completed negative; do
                    // not backdate it to the previous long utterance's end.
                    evidence.analyzed_through_ms
                } else {
                    evidence.last_detected_speech_end_ms.unwrap_or_default()
                }
            } else {
                // PendingSpeech and Unknown also break a proven quiet span.
                // A short onset need not meet canonical VAD's duration gate.
                evidence.analyzed_through_ms
            },
        );
        self.evidence = evidence;
        self.observed_at = observed_at;
        self.valid = evidence.state != LocalSpeechActivityState::Unknown;
    }

    fn confirmed_silence(&self) -> Option<crate::personal_vad::ConfirmedSilenceObservation> {
        (self.valid
            && self.evidence.state == crate::asr::volcengine::LocalSpeechActivityState::NonSpeech
            && self.evidence.pending_speech_start_ms.is_none())
            .then_some(crate::personal_vad::ConfirmedSilenceObservation {
                analyzed_end_ms: self.evidence.analyzed_through_ms,
                silence_since_ms: self.silence_since_ms,
                observed_at: self.observed_at,
            })
    }
}

const RECENT_STRONG_NON_TARGET_WINDOW_MS: u64 = 900;
// A new local VAD onset can arrive just before the owner inactivity deadline.
// The classifier's coverage may briefly trail capture; that gap is pending
// analysis, not evidence of silence. It is never an owner watermark.
const AUTOMATIC_STOP_VAD_MAX_LAG_MS: u64 = 250;
const AUTOMATIC_STOP_VAD_STALLED_ANALYSIS_ESCAPE_MS: u64 = 3_000;
const TENTATIVE_OWNER_CONTINUATION_MAX_WAIT_MS: u64 = 1_500;
const MANUAL_TERMINAL_BRIDGE_MAX_MS: u64 = 3_000;
const MANUAL_TERMINAL_BRIDGE_MIN_VISIBLE_CHARS: usize = 20;
/// Automatic-wake sessions earn the bounded open-clause continuation only
/// with an established body.  A short finished command (below this many
/// visible chars) keeps the fast one-second contract; a flowing dictation's
/// deliberate mid-sentence pause (defect A, 2026-09-19: "…然后现在体验好像没
/// 有一开" cut mid-word in a quiet room; and the 12:36Z session "你看一下怎
/// 么弄哦？我快点把…" cut after a rhetorical question) gets the same one
/// natural pause manual sessions already have.
const AUTOMATIC_WAKE_CONTINUATION_MIN_VISIBLE_CHARS: usize = 10;
const ENDPOINT_PROVIDER_CATCH_UP_GRACE_MS: u64 = 300;
// A 1.2 s speaker window can take longer than the public one-second endpoint
// when the owner-only separator is using the same inference pool.  Wait for
// the already-started identity job, not for arbitrary sound or provider text.
// One uninterrupted chain has a hard ceiling so a wedged model cannot disable
// auto-end.
const OWNER_ANALYSIS_IN_FLIGHT_MAX_WAIT_MS: u64 = 3_000;

fn endpoint_hold_reason(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    latest_visible_body_ends_terminal: Option<bool>,
) -> &'static str {
    if update.pending_unattributed_speech
        && !update_has_recent_strong_non_target(update)
        && has_unresolved_recent_owner_speech(
            update,
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
    {
        return "pending_provider_text";
    }
    if update.local_speaker_tracking_enabled
        && has_unresolved_recent_owner_speech(update, EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS)
    {
        return "fresh_owner_tail";
    }
    if update.local_speaker_tracking_enabled
        && has_established_owner_uncertain_continuation(update)
    {
        return "owner_uncertain_continuation";
    }
    if latest_visible_body_ends_terminal == Some(false) {
        return "open_clause_tail";
    }
    if !(update.speaker_info_present && update.speaker_id.is_some())
        && update.qualified_owner_speech_end_ms.is_none()
    {
        return "owner_identity_not_settled";
    }
    "endpoint_guard"
}

fn product_endpoint_evidence(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    authoritative_owner_watermark_ms: Option<u64>,
    positive_owner_evidence_live: bool,
) -> crate::speech_decision_kernel::EndpointEvidence {
    let latest_speech_confirmed_non_target = update
        .local_speech_end_ms
        .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
    crate::speech_decision_kernel::EndpointEvidence {
        // The settled clock has already fused provider and local identity.
        // Reusing the raw provider boundary here would let a provider row that
        // collapsed a nearby second speaker masquerade as fresh owner growth.
        owner_watermark_ms: authoritative_owner_watermark_ms,
        provider_coverage_ms: update.provider_audio_duration_ms,
        // An expired positive-evidence budget downgrades unclassified/cloud
        // continuation to silence for the stop controller as well: leaving
        // these bits set let the kernel Hold forever on room-noise edges the
        // policy gate had already stopped vouching for.
        pending_provider_text: positive_owner_evidence_live
            && update.pending_unattributed_speech,
        latest_speech_confirmed_non_target,
        // Same fusion as `update_allows_endpoint`: an established owner whose
        // live speech windows stay Uncertain (interference-degraded identity,
        // installed r10) is still an unresolved owner tail for the stop
        // controller, otherwise decide_stop fires through a hold the policy
        // gate just granted.
        unresolved_owner_tail: positive_owner_evidence_live
            && (has_unresolved_recent_owner_speech(
                update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            ) || has_established_owner_uncertain_continuation(update)),
    }
}

fn update_has_recent_strong_non_target(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    let Some(non_target_end_ms) = update.local_non_target_speech_end_ms else {
        return false;
    };
    if update
        .qualified_owner_speech_end_ms
        .is_some_and(|target_end_ms| target_end_ms > non_target_end_ms)
    {
        return false;
    }
    let latest_audio_ms = update
        .audio_duration_ms
        .into_iter()
        .chain(update.provider_audio_duration_ms)
        .max()
        .unwrap_or(non_target_end_ms);
    latest_audio_ms.saturating_sub(non_target_end_ms) <= RECENT_STRONG_NON_TARGET_WINDOW_MS
}

fn automatic_vad_candidate_holds_stop(
    evidence: crate::asr::volcengine::LocalSpeechEvidence,
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    recent_owner_evidence: bool,
) -> bool {
    use crate::asr::volcengine::LocalSpeechActivityState;
    let speech_candidate = matches!(
        evidence.state,
        LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech
    );
    if !speech_candidate || !recent_owner_evidence || update_has_recent_strong_non_target(update) {
        return false;
    }
    let captured_ms = update.audio_duration_ms.unwrap_or_default();
    evidence.revision > 0
        && captured_ms.saturating_sub(evidence.analyzed_through_ms)
            < AUTOMATIC_STOP_VAD_STALLED_ANALYSIS_ESCAPE_MS
}

/// Once the local owner has been established, a fresh still-speakerless or
/// provider-pending speech edge whose identity is uncertain is a bounded stop
/// barrier, not a new owner watermark. The endpoint watermark remains the last
/// qualified local Target observation. Cloud-attributed growth is excluded: it
/// is the shape that can merge a room speaker into the owner's old provider
/// id. Silence is an unchanged speech edge and therefore does not renew this
/// boundary.
fn owner_endpoint_boundary_with_uncertain_tail(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    cloud_owner_boundary_ms: Option<u64>,
) -> Option<u64> {
    authoritative_owner_endpoint_boundary(update, cloud_owner_boundary_ms)
}

impl SettledTargetEndpointClock {
    fn note_personal_presence(
        &mut self,
        observation: Option<crate::personal_vad::PresenceObservation>,
        now: Instant,
    ) -> bool {
        let endpoint = self.personal_presence_endpoint.get_or_insert_with(Default::default);
        let owner_advanced = match observation {
            Some(frame) => endpoint.observe(frame),
            None => { endpoint.invalidate(); false }
        };
        if owner_advanced {
            self.generation = self.generation.wrapping_add(1);
            self.stop_proposed = false;
            self.armed_at = Some(now);
            self.clear_paused_arm();
        }
        owner_advanced
    }

    fn personal_presence_stop_due(&self, captured_ms: u64, now: Instant, timeout_ms: u64) -> bool {
        // STOP admission calls this again after a proposal was scheduled. A
        // newly dispatched paste must hold it until its actual receipt arrives.
        if self.pending_body_dispatch.is_some() || !self.pending_body_receipts.is_empty()
            || self.pending_owner_delivery_active(now)
        {
            return false;
        }
        let Some(endpoint) = self.personal_presence_endpoint.as_ref() else { return false; };
        // A still-fresh negative can end before newly captured audio was
        // analyzed. Revoke that authority for a new VAD source epoch, both
        // when proposing STOP and when admitting the queued physical STOP.
        let new_epoch_unobserved = self.local_vad_stop_observation.as_ref()
            .and_then(|observation| observation.epoch_source_fence)
            .zip(endpoint.analyzed_source_end_ms())
            .is_some_and(|(fence, end_ms)| fence.is_unobserved(end_ms));
        if !new_epoch_unobserved && endpoint.due(captured_ms, now, timeout_ms) {
            return true;
        }
        // Missing/stalled personal analysis is not silence. Only a separate
        // completed VAD quiet span may release it, and actual editor delivery
        // still retains the full continuation window in this fallback.
        if self.last_body_delivery_at.is_some_and(|at| {
            now.saturating_duration_since(at) < Duration::from_millis(timeout_ms)
        }) {
            return false;
        }
        self.local_vad_stop_observation.as_ref()
            .and_then(LocalVadStopObservation::confirmed_silence)
            .is_some_and(|silence| endpoint.due_on_confirmed_silence(silence, captured_ms, now, timeout_ms))
    }

    fn begin_text_submission(
        &mut self,
        now: Instant,
        text: &str,
    ) -> Option<BodySubmissionSequence> {
        if embedded_audio_partial_preview_stability_key(text).is_empty()
            || !self.automatic_wake_session
            || self.automatic_no_body_armed
        {
            return None;
        }
        self.poll_body_receipts();
        self.next_body_submission_sequence =
            self.next_body_submission_sequence.wrapping_add(1).max(1);
        let sequence = BodySubmissionSequence(self.next_body_submission_sequence);
        let audio_boundary_ms = self
            .latest_update
            .as_ref()
            .and_then(|update| update.audio_duration_ms);
        let owner_boundary_ms = self
            .latest_update
            .as_ref()
            .and_then(endpoint_owner_watermark);
        let personal_continuation_boundary_ms = self.personal_presence_endpoint.as_ref()
            .and_then(crate::personal_vad::PresenceEndpoint::continuation_activity_end_ms);
        self.pending_body_dispatch = Some(PendingBodyDispatch {
            sequence,
            dispatched_at: now,
            audio_boundary_ms,
            owner_boundary_ms,
            personal_continuation_boundary_ms,
        });
        Some(sequence)
    }

    fn settle_text_submission(
        &mut self,
        sequence: Option<BodySubmissionSequence>,
        submitted: bool,
        receipt: Option<crate::insertion::PasteReceiptHandle>,
    ) {
        let Some(sequence) = sequence else { return; };
        if !self
            .pending_body_dispatch
            .as_ref()
            .is_some_and(|dispatch| dispatch.sequence == sequence)
        {
            return;
        }
        let dispatch = self.pending_body_dispatch.take().expect("matching dispatch");
        if !submitted {
            return;
        }
        let owner_boundary_now = self
            .latest_update
            .as_ref()
            .and_then(endpoint_owner_watermark);
        let owner_advanced_after_dispatch = owner_boundary_now.is_some_and(|owner_ms| {
            dispatch.audio_boundary_ms.map_or_else(
                || dispatch.owner_boundary_ms.is_none_or(|prior| owner_ms > prior),
                |audio_ms| owner_ms > audio_ms,
            )
        }) || self
            .tentative_owner_continuation_at
            .is_some_and(|at| at > dispatch.dispatched_at)
            || self.tentative_owner_continuation_audio_ms.is_some_and(|audio_ms| {
                dispatch
                    .audio_boundary_ms
                    .is_some_and(|boundary| audio_ms > boundary)
            });
        self.note_body_delivery_at(
            dispatch.dispatched_at,
            dispatch.audio_boundary_ms,
            owner_advanced_after_dispatch,
            dispatch.personal_continuation_boundary_ms,
            receipt.is_some(),
        );
        self.last_body_submission_sequence = Some(sequence);
        // A successful newer body makes every older receipt irrelevant to the
        // current continuation clock. If the newer dispatch fails, this clear
        // is never reached and the prior receipt keeps its ownership.
        self.pending_body_receipts.clear();
        if let Some(receipt) = receipt {
            let ticket = receipt.ticket();
            self.pending_body_receipts.push(PendingBodyReceipt {
                sequence,
                ticket,
                receipt,
                personal_continuation_boundary_ms: dispatch.personal_continuation_boundary_ms,
            });
        }
    }

    fn poll_body_receipts(&mut self) {
        use crate::insertion::{PasteReceiptPoll, PasteReceiptOutcome};
        let mut index = 0;
        while index < self.pending_body_receipts.len() {
            let poll = self.pending_body_receipts[index].receipt.try_take();
            match poll {
                PasteReceiptPoll::Pending => index += 1,
                PasteReceiptPoll::Ready(completion) => {
                    let pending = self.pending_body_receipts.remove(index);
                    if completion.ticket != pending.ticket {
                        if self.last_body_submission_sequence == Some(pending.sequence) {
                            if let Some(endpoint) = self.personal_presence_endpoint.as_mut() {
                                endpoint.settle_unconfirmed_delivery(pending.personal_continuation_boundary_ms);
                            }
                        }
                        continue;
                    }
                    // Only a positive editor observation moves the public
                    // continuation clock. A delayed receipt cannot erase PCM
                    // or VAD evidence recorded after dispatch.
                    if self.last_body_submission_sequence == Some(pending.sequence) {
                        if matches!(completion.outcome, PasteReceiptOutcome::Confirmed(_)) {
                            if let Some(dispatched_at) = self.last_body_delivery_at {
                                self.last_body_delivery_at =
                                    Some(dispatched_at.max(completion.observed_at));
                            }
                            if let Some(endpoint) = self.personal_presence_endpoint.as_mut() {
                                endpoint.confirm_delivery(
                                    completion.observed_at,
                                    pending.personal_continuation_boundary_ms,
                                );
                            }
                        } else if let Some(endpoint) = self.personal_presence_endpoint.as_mut() {
                            endpoint.settle_unconfirmed_delivery(pending.personal_continuation_boundary_ms);
                        }
                    }
                    log::info!(
                        "[asr] endpoint body receipt sequence={} ticket={:?} confirmed={} outcome={:?} continuation_ms={}",
                        pending.sequence.0,
                        pending.ticket,
                        matches!(completion.outcome, PasteReceiptOutcome::Confirmed(_)),
                        completion.outcome,
                        EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
                    );
                }
                PasteReceiptPoll::Consumed => {
                    let pending = self.pending_body_receipts.remove(index);
                    if self.last_body_submission_sequence == Some(pending.sequence) {
                        if let Some(endpoint) = self.personal_presence_endpoint.as_mut() {
                            endpoint.settle_unconfirmed_delivery(pending.personal_continuation_boundary_ms);
                        }
                    }
                }
            }
        }
    }

    fn refresh_pending_body_delivery(
        &mut self,
        candidate: Option<crate::asr::volcengine::PendingBodyDelivery>,
        _now: Instant,
    ) {
        if self.automatic_no_body_armed {
            // A pre-wake/command-only preview has not entered the product's
            // body lifecycle and cannot create a delivery obligation.
            self.pending_owner_delivery = None;
            return;
        }
        let Some(crate::asr::volcengine::PendingBodyDelivery { key, recognition_pending, .. }) = candidate else {
            self.pending_owner_delivery = None;
            return;
        };
        if embedded_audio_partial_preview_stability_key(&key).is_empty() {
            return;
        }
        if let Some(endpoint) = self.personal_presence_endpoint.as_ref() {
            self.last_pending_personal_continuation_end_ms = self.last_pending_personal_continuation_end_ms
                .max(endpoint.continuation_activity_end_ms());
        }
        self.pending_owner_delivery = Some(PendingOwnerDeliveryHold { key, recognition_pending });
    }

    fn pending_owner_delivery_active(&self, _now: Instant) -> bool {
        // Recognition, lexical maturity and editor submission are separate
        // responsibilities. A deadline cannot resolve any approved body.
        // Consumption/retraction clears this obligation; dispatch and matching
        // editor receipts then own the existing three-second continuation.
        self.pending_owner_delivery.is_some()
    }

    #[cfg(test)]
    fn refresh_pending_owner_delivery(&mut self, candidate: Option<(String, Instant)>, now: Instant) {
        self.refresh_pending_body_delivery(candidate.map(|(key, updated_at)|
            crate::asr::volcengine::PendingBodyDelivery { key, updated_at, recognition_pending: false }), now);
    }

    fn note_text_delivery(&mut self, now: Instant, text: &str) {
        // Punctuation-only revisions do not represent the owner speaking or
        // new body delivery. They cannot postpone the three-second deadline.
        if !embedded_audio_partial_preview_stability_key(text).is_empty() {
            self.note_body_delivery(now);
        }
    }

    fn note_body_delivery(&mut self, now: Instant) {
        // A synchronous/TSF commit is a new body generation without a paste
        // receipt. Older clipboard tickets no longer own its continuation
        // clock. Punctuation never reaches this method through text delivery.
        self.pending_body_receipts.clear();
        self.last_body_submission_sequence = None;
        let audio_boundary_ms = self
            .latest_update
            .as_ref()
            .and_then(|update| update.audio_duration_ms);
        let personal_continuation_boundary_ms = self.personal_presence_endpoint.as_ref()
            .and_then(crate::personal_vad::PresenceEndpoint::continuation_activity_end_ms);
        self.note_body_delivery_at(now, audio_boundary_ms, false, personal_continuation_boundary_ms, false);
    }

    fn note_body_delivery_at(
        &mut self,
        now: Instant,
        audio_boundary_ms: Option<u64>,
        preserve_post_dispatch_owner: bool,
        personal_continuation_boundary_ms: Option<u64>,
        personal_receipt_pending: bool,
    ) {
        if !self.automatic_wake_session || self.automatic_no_body_armed {
            return;
        }
        if let Some(endpoint) = self.personal_presence_endpoint.as_mut() {
            endpoint.note_submission(now, personal_continuation_boundary_ms, personal_receipt_pending);
            // A delivered possible-owner range cannot buy another hold for
            // a later background key. Newer captured activity stays separate.
            self.last_pending_personal_continuation_end_ms = self.last_pending_personal_continuation_end_ms
                .max(personal_continuation_boundary_ms);
        }
        self.pending_owner_delivery = None;
        self.last_body_delivery_at = Some(now);
        self.last_body_delivery_audio_ms = audio_boundary_ms;
        if !preserve_post_dispatch_owner {
            self.tentative_owner_continuation_at = None;
            self.tentative_owner_continuation_audio_ms = None;
            self.tentative_owner_vad_epoch = None;
            self.tentative_owner_quiet_audio_ms = None;
            if self.latest_local_vad_evidence.is_some_and(|evidence| {
                matches!(
                    evidence.state,
                    crate::asr::volcengine::LocalSpeechActivityState::PendingSpeech
                        | crate::asr::volcengine::LocalSpeechActivityState::Speech
                )
            }) && self.latest_update.as_ref().is_some_and(|update| {
                !update_has_recent_strong_non_target(update)
            }) {
                self.tentative_owner_vad_epoch =
                    self.latest_local_vad_evidence.map(|evidence| evidence.activity_epoch);
            }
        }
        self.manual_terminal_bridge_until = None;
        self.manual_terminal_bridge_rearm_pending = false;
        // A committed chunk is the start of the public continuation window.
        // A later provider revision of that same audio is not a new utterance.
        if !preserve_post_dispatch_owner {
            self.reset_continuation_tracking();
        }
        log::info!(
            "[asr] endpoint body_delivery window_ms={} audio_ms={:?} generation={}",
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            self.last_body_delivery_audio_ms,
            self.generation,
        );
    }

    fn owner_spoke_after_body_delivery(
        &self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
    ) -> bool {
        let Some(delivery_audio_ms) = self.last_body_delivery_audio_ms else {
            return false;
        };
        let owner_end_ms = if update.local_speaker_tracking_enabled {
            // local_target_speech_end_ms can be advanced by a delayed cloud
            // preview of audio captured before delivery (session b72c67ea).
            // Only a fresh quality-qualified local voiceprint observation
            // proves the enrolled owner spoke after the chunk reached input.
            update.qualified_owner_speech_end_ms
        } else {
            update.local_speech_end_ms
        };
        owner_end_ms.is_some_and(|end_ms| end_ms > delivery_audio_ms)
    }

    pub(crate) fn lifecycle(&self) -> crate::speech_decision_kernel::OwnerEndpointState {
        self.product_endpoint.state()
    }

    /// Seed the single endpoint controller when a provider emits visible text
    /// before its first diarization/target row. Without this hand-off a
    /// healthy ASR session had no clock at all, so the disabled raw-energy
    /// fallback left the capsule recording indefinitely. The snapshot is
    /// only an initial observation; subsequent renewals still require positive
    /// owner evidence from the normal target-update path.
    pub(crate) fn seed_from_snapshot_if_missing(
        &mut self,
        update: crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
    ) {
        if self.latest_update.is_none() {
            self.observe(&update, body_started, now);
        }
    }

    fn arm_automatic_no_body_if_needed(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        started_at: Instant,
        observed_at: Instant,
    ) {
        if self.automatic_no_body_armed {
            self.observe_automatic_no_body_speech(update, observed_at);
            self.latest_update = Some(update.clone());
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = Some(started_at);
        self.armed_from_visible_body_fallback = true;
        self.automatic_no_body_armed = true;
        self.automatic_wake_session = true;
        if self.last_positive_owner_evidence_at.is_none() {
            self.note_positive_owner_evidence(started_at);
        }
        self.automatic_no_body_initial_audio_ms = update.audio_duration_ms.unwrap_or(0);
        self.automatic_no_body_last_speech_ms = update.local_speech_end_ms;
        self.latest_update = Some(update.clone());
        self.latest_update_at = Some(started_at);
        self.clear_paused_arm();
        self.pending_was_seen = false;
        self.product_endpoint
            .arm(crate::speech_decision_kernel::EndpointEvidence::default());
        self.stop_proposed = false;
        log::info!(
            "[asr] endpoint transition action=arm source=automatic_no_body reason=accepted_wake_without_body generation={} initial_audio_ms={} local_speech_end_ms={:?} audio_ms={:?} provider_ms={:?}",
            self.generation,
            self.automatic_no_body_initial_audio_ms,
            self.automatic_no_body_last_speech_ms,
            update.audio_duration_ms,
            update.provider_audio_duration_ms,
        );
    }

    fn observe_automatic_no_body_speech(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        now: Instant,
    ) {
        let Some(speech_ms) = update.local_speech_end_ms else { return; };
        // A cloud first result can take longer than the no-body window. New
        // microphone speech after the wake boundary must preserve that body
        // while it is awaiting text. Repeated wake evidence cannot renew it,
        // and a positively identified other speaker cannot take ownership.
        if speech_ms <= self.automatic_no_body_initial_audio_ms.saturating_add(600)
            || self.automatic_no_body_last_speech_ms.is_some_and(|last| speech_ms <= last)
            || local_speech_confidently_non_target(update, speech_ms)
        {
            return;
        }
        let armed_at_before = self.armed_at;
        self.automatic_no_body_last_speech_ms = Some(speech_ms);
        self.latest_update_at = Some(now);
        self.armed_at = Some(now);
        log::info!(
            "[asr] endpoint transition action=rearm source=automatic_no_body reason=new_post_wake_local_speech generation={} previous_armed_age_ms={:?} audio_ms={:?} provider_ms={:?} local_speech_end_ms={:?} speech_ms={} classification={:?} quality={:?}",
            self.generation,
            Self::elapsed_ms(armed_at_before, now),
            update.audio_duration_ms,
            update.provider_audio_duration_ms,
            update.local_speech_end_ms,
            speech_ms,
            update.local_speaker_classification_kind,
            update.local_speaker_signal_quality_sufficient,
        );
    }

    fn leave_automatic_no_body_mode(&mut self) {
        if !self.automatic_no_body_armed {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = None;
        self.armed_from_visible_body_fallback = false;
        self.automatic_no_body_armed = false;
        self.clear_paused_arm();
        self.pending_was_seen = false;
        self.product_endpoint.reset();
        self.stop_proposed = false;
    }

    fn should_renew_firmware_endpoint_lease(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
    ) -> bool {
        if self.personal_presence_endpoint.is_some() {
            // Personal-VAD positives renew this lease at the observation site.
            // Generic speech and provider words carry no owner activity here.
            return false;
        }
        let unresolved_owner_tail = has_unresolved_recent_owner_speech(
            update, EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        );
        let owner_speech_ms = endpoint_owner_watermark(update);
        if update.local_speaker_tracking_enabled
            && update.qualified_owner_speech_end_ms.is_some()
            && !update.qualified_owner_activity_advanced
        {
            return false;
        }
        if owner_speech_ms.is_some()
            && owner_speech_ms == self.last_firmware_lease_owner_speech_ms
        {
            return false;
        }
        let latest_speech_confirmed_non_target = update
            .local_speech_end_ms
            .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
        let owner_established = endpoint_owner_watermark(update).is_some();
        let evidence = crate::speech_decision_kernel::FirmwareEndpointLeaseEvidence {
            visible_body: body_started,
            owner_established,
            owner_speech_watermark_ms: owner_speech_ms,
            provider_coverage_ms: update.provider_audio_duration_ms,
            unresolved_owner_tail,
            latest_speech_confirmed_non_target,
        };
        if crate::speech_decision_kernel::decide_firmware_endpoint_lease(evidence)
            != crate::speech_decision_kernel::FirmwareEndpointLeaseDecision::Renew
        {
            return false;
        }
        self.last_firmware_lease_owner_speech_ms = owner_speech_ms;
        true
    }

    fn should_keep_firmware_alive_for_host_hang(
        &mut self,
        hang_ms: u64,
        last_visible_growth_at: Option<Instant>,
        now: Instant,
    ) -> bool {
        if self.personal_presence_endpoint.is_some() {
            return false;
        }
        if self.continuation_cutoff_reached {
            return false;
        }
        let Some(grown_at) = last_visible_growth_at else {
            return false;
        };
        if now.saturating_duration_since(grown_at) >= Duration::from_millis(hang_ms) {
            return false;
        }
        if self.last_host_hang_lease_at.is_some_and(|last| {
            now.saturating_duration_since(last)
                < Duration::from_millis(EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS)
        }) {
            return false;
        }
        self.last_host_hang_lease_at = Some(now);
        true
    }

    fn should_keep_firmware_alive_for_continuation(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.continuation_pending_until else {
            return false;
        };
        if now >= deadline {
            return false;
        }
        if self.last_host_hang_lease_at.is_some_and(|last| {
            now.saturating_duration_since(last)
                < Duration::from_millis(EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS)
        }) {
            return false;
        }
        self.last_host_hang_lease_at = Some(now);
        true
    }

    #[cfg(test)]
    fn note_local_vad_evidence(
        &mut self,
        evidence: crate::asr::volcengine::LocalSpeechEvidence,
    ) {
        self.note_local_vad_evidence_at(evidence, Instant::now());
    }

    fn note_local_vad_evidence_at(
        &mut self,
        evidence: crate::asr::volcengine::LocalSpeechEvidence,
        observed_at: Instant,
    ) {
        match self.local_vad_stop_observation.as_mut() {
            Some(observation) => observation.observe(evidence, observed_at),
            None => self.local_vad_stop_observation = Some(LocalVadStopObservation::new(evidence, observed_at)),
        }
        use crate::asr::volcengine::LocalSpeechActivityState;
        if matches!(evidence.state, LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech)
            && self.latest_local_vad_evidence.is_none_or(|previous| {
                previous.activity_epoch != evidence.activity_epoch
            })
        {
            self.latest_local_vad_epoch_start_ms = Some(
                evidence.pending_speech_start_ms.unwrap_or_else(|| {
                    self.latest_local_vad_evidence.map_or(
                        evidence.last_detected_speech_end_ms.unwrap_or_default(),
                        |previous| previous.analyzed_through_ms,
                    )
                }),
            );
        }
        // The last delivered chunk opens a three-second opportunity to
        // continue. A new local speech turn within it remains unresolved until
        // speaker evidence arrives; it cannot be silently discarded merely
        // because every classifier window is Uncertain.
        if self.automatic_wake_session
            && !self.automatic_no_body_armed
            && self.tentative_owner_vad_epoch.is_none()
            && matches!(evidence.state, LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech)
            && self.latest_update.as_ref().is_some_and(|update| {
                !update_has_recent_strong_non_target(update)
            })
        {
            if let (Some(delivery_ms), Some(previous)) =
                (self.last_body_delivery_audio_ms, self.latest_local_vad_evidence)
            {
                let onset_ms = evidence
                    .pending_speech_start_ms
                    .unwrap_or(evidence.analyzed_through_ms);
                if evidence.activity_epoch > previous.activity_epoch
                    && onset_ms.saturating_add(AUTOMATIC_STOP_VAD_MAX_LAG_MS) >= delivery_ms
                    && self.last_body_delivery_at.is_some_and(|delivered_at| {
                        // Microphone PCM and wall time can drift during BLE
                        // catch-up. The public window starts at submission;
                        // a fresh VAD onset inside it must remain pending.
                        observed_at.saturating_duration_since(delivered_at)
                            < Duration::from_millis(EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS)
                    })
                {
                    self.tentative_owner_vad_epoch = Some(evidence.activity_epoch);
                    self.tentative_owner_quiet_audio_ms = None;
                }
            }
        }
        // A new speech epoch inside the quiet window may be the same owner
        // continuing after a pause. Keep it pending until speaker evidence
        // settles; a confirmed Other observation clears the turn in observe.
        if let (Some(tracked_epoch), Some(previous)) =
            (self.tentative_owner_vad_epoch, self.latest_local_vad_evidence)
        {
            if evidence.activity_epoch == tracked_epoch
                && evidence.state == LocalSpeechActivityState::NonSpeech
                && self.tentative_owner_quiet_audio_ms.is_none()
            {
                let quiet_start_ms = evidence
                    .last_detected_speech_end_ms
                    .filter(|speech_end_ms| {
                        self.tentative_owner_continuation_audio_ms
                            .or(self.last_body_delivery_audio_ms)
                            .is_some_and(|candidate_ms| *speech_end_ms >= candidate_ms)
                    })
                    .unwrap_or(evidence.analyzed_through_ms);
                self.tentative_owner_quiet_audio_ms = Some(quiet_start_ms);
            }
            if previous.activity_epoch == tracked_epoch
                && previous.state == LocalSpeechActivityState::NonSpeech
                && evidence.activity_epoch > tracked_epoch
                && matches!(evidence.state, LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech)
                && self.tentative_owner_quiet_audio_ms.is_some_and(|speech_end_ms| {
                    evidence
                        .pending_speech_start_ms
                        .unwrap_or(evidence.analyzed_through_ms)
                        .saturating_sub(speech_end_ms)
                        < EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
                })
            {
                self.tentative_owner_vad_epoch = Some(evidence.activity_epoch);
                self.tentative_owner_quiet_audio_ms = None;
            }
        }
        let previous_state = self.latest_local_vad_evidence.map(|previous| previous.state);
        if evidence.state == crate::asr::volcengine::LocalSpeechActivityState::Speech
            && previous_state
                != Some(crate::asr::volcengine::LocalSpeechActivityState::Speech)
        {
            self.canonical_speech_serial = self.canonical_speech_serial.wrapping_add(1);
        }
        self.latest_local_vad_evidence = Some(evidence);
    }

    fn note_qualified_owner_vad_turn(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
    ) {
        if !self.automatic_wake_session || !update.local_speaker_tracking_enabled {
            return;
        }
        if update_has_recent_strong_non_target(update) {
            self.tentative_owner_vad_epoch = None;
            self.tentative_owner_quiet_audio_ms = None;
            return;
        }
        // Delivery already seeds its own continuation window. Before it,
        // preserve the actual owner-qualified turn from the no-body stage;
        // waiting for provider attribution cannot discard that turn's VAD.
        if self.last_body_delivery_at.is_some() {
            return;
        }
        let Some(owner_end_ms) = update.qualified_owner_speech_end_ms.filter(|edge| {
            *edge > 0 && self.latest_update.as_ref()
                .and_then(|previous| previous.qualified_owner_speech_end_ms)
                .is_none_or(|previous| *edge > previous)
        }) else { return; };
        let Some(evidence) = self.latest_local_vad_evidence else { return; };
        use crate::asr::volcengine::LocalSpeechActivityState;
        if matches!(evidence.state, LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech)
            && evidence.activity_epoch > 0
            && evidence.revision > 0
            && self.latest_local_vad_epoch_start_ms.is_some_and(|onset| owner_end_ms >= onset)
            && owner_end_ms <= evidence.analyzed_through_ms.saturating_add(AUTOMATIC_STOP_VAD_MAX_LAG_MS)
            && update.audio_duration_ms.is_some_and(|captured| {
                captured.abs_diff(evidence.analyzed_through_ms) <= AUTOMATIC_STOP_VAD_MAX_LAG_MS
            })
        {
            self.tentative_owner_vad_epoch = Some(evidence.activity_epoch);
            self.tentative_owner_continuation_audio_ms = Some(owner_end_ms);
            self.tentative_owner_quiet_audio_ms = None;
            log::info!(
                "[asr] endpoint owner_turn source=qualified_local_pcm activity_epoch={} owner_end_ms={} onset_ms={:?} before_first_delivery=true",
                evidence.activity_epoch, owner_end_ms, self.latest_local_vad_epoch_start_ms,
            );
        }
    }

    fn clear_continuation_pending(&mut self) {
        self.continuation_pending_until = None;
        self.continuation_pending_anchor_speech_end_ms = None;
    }

    fn positive_owner_evidence_live(&self, now: Instant) -> bool {
        self.last_positive_owner_evidence_at.is_some_and(|at| {
            now.saturating_duration_since(at)
                < Duration::from_millis(EMBEDDED_OWNER_POSITIVE_EVIDENCE_BUDGET_MS)
        })
    }

    fn note_positive_owner_evidence(&mut self, now: Instant) {
        self.last_positive_owner_evidence_at = Some(now);
    }

    fn reset_continuation_tracking(&mut self) {
        self.clear_continuation_pending();
        self.continuation_pending_anchor_canonical_speech_serial = None;
        self.continuation_cutoff_reached = false;
    }

    #[cfg(test)]
    fn continuation_pending_active(&self, now: Instant) -> bool {
        self.continuation_pending_until
            .is_some_and(|deadline| now < deadline)
    }

    fn maybe_enter_manual_continuation_pending(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        armed_at: Instant,
        now: Instant,
    ) -> bool {
        // Defect A (2026-09-19): an automatic wake's deliberate mid-sentence
        // thinking pause in a quiet room leaves no acoustic evidence at all —
        // no Uncertain window, no fresh unclassified speech — so the public
        // one-second clock fired and cut a flowing dictation mid-word.  Manual
        // sessions already keep this bounded continuation stage for exactly
        // that body shape; extend it to automatic wake sessions once the body
        // is established (>= 10 visible chars keeps short finished commands
        // on the fast one-second contract).  A terminal mark does NOT end an
        // automatic session's eligibility (12:36Z session: the rhetorical
        // "你看一下怎么弄哦？" was punctuated terminal, the 1 s clock cut the
        // resumed "我快点把…" tail — a conversational ？ invites continuation,
        // it does not close the dictation).  Manual hotkey sessions keep the
        // stricter non-terminal-only contract.  Confirmed other-speaker
        // evidence and the continuous-interference Uncertain holds keep their
        // own contracts and are not weakened here.  The automatic eligibility
        // also requires a live positive-evidence budget: a settled preview
        // whose room noise keeps advancing local speech edges would otherwise
        // cancel and re-enter this window with a fresh anchor forever (the
        // "不能自动结束" loop).  A real pause fits — preview growth refreshed
        // the budget at the last spoken word, and the window is at most 3 s.
        let continuation_eligible = self.manual_vad_guard
            || (self.automatic_wake_session
                && self.positive_owner_evidence_live(now)
                && self.open_body_peak_visible_chars
                    >= AUTOMATIC_WAKE_CONTINUATION_MIN_VISIBLE_CHARS);
        let body_shape_allows_continuation = if self.manual_vad_guard {
            self.latest_visible_body_ends_terminal == Some(false)
        } else {
            true
        };
        if !continuation_eligible || !body_shape_allows_continuation {
            if self
                .continuation_pending_anchor_canonical_speech_serial
                .is_some()
            {
                self.continuation_cutoff_reached = true;
            }
            self.clear_continuation_pending();
            return false;
        }
        if self.continuation_cutoff_reached {
            return false;
        }
        if self.latest_local_vad_evidence.is_some_and(|evidence| {
            matches!(
                evidence.state,
                crate::asr::volcengine::LocalSpeechActivityState::Speech
                    | crate::asr::volcengine::LocalSpeechActivityState::Unknown
            )
        }) {
            // The ordinary endpoint guard below still handles live speech and
            // unknown/lagging analysis. Neither state is a confirmed silence
            // interval from which a continuation deadline may be created.
            return false;
        }
        if let Some(deadline) = self.continuation_pending_until {
            if now < deadline {
                return true;
            }
            self.clear_continuation_pending();
            self.continuation_cutoff_reached = true;
            return false;
        }
        // When the independent VAD is present, its last confirmed speech edge
        // is the only legal anchor. In particular, PendingSpeech/Unknown may
        // expose the current capture edge through TargetSpeakerUpdate, but
        // that edge is not a confirmed new speech interval and must not buy a
        // fresh 3 s continuation window.
        let confirmed_speech_end_ms = if self.automatic_wake_session
            && update.local_speaker_tracking_enabled
        {
            // A bystander's raw VAD edge is not the user's continuation.
            // The owner-specific edge is the only legal deadline anchor when
            // the automatic session has local speaker tracking.
            update
                .local_target_speech_end_ms
                .or(update.qualified_owner_speech_end_ms)
        } else {
            match self.latest_local_vad_evidence {
                Some(evidence) => evidence.last_detected_speech_end_ms,
                None => {
                    // 2026-09-19 13:42Z (session f564b094): a missing or
                    // long-stale edge used to latch the cutoff and swallow
                    // the owner's resumed tail. For manual/no-tracker turns,
                    // use the live audio edge inside this bounded window.
                    const STALE_LOCAL_EDGE_MAX_LAG_MS: u64 = 2_000;
                    let local_edge_usable = update.local_speech_end_ms.is_some_and(|edge_ms| {
                        update.audio_duration_ms.is_none_or(|audio_ms| {
                            audio_ms.saturating_sub(edge_ms) <= STALE_LOCAL_EDGE_MAX_LAG_MS
                        })
                    });
                    if local_edge_usable {
                        update.local_speech_end_ms
                    } else {
                        update.audio_duration_ms
                    }
                }
            }
        };
        let current_audio_ms = update
            .audio_duration_ms
            .or(confirmed_speech_end_ms)
            .unwrap_or_default();
        let Some(confirmed_speech_end_ms) = confirmed_speech_end_ms else {
            self.continuation_cutoff_reached = true;
            return false;
        };
        let continuation_end_audio_ms = confirmed_speech_end_ms
            .saturating_add(EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS);
        let mut remaining_ms = continuation_end_audio_ms.saturating_sub(current_audio_ms);
        if self.automatic_wake_session {
            // The public three seconds start at the last owner arm, not when a
            // delayed provider/VAD callback happens to enter this stage.
            remaining_ms = remaining_ms.min(
                EMBEDDED_DANGLING_CONTINUATION_END_TIMEOUT_MS.saturating_sub(
                    now.saturating_duration_since(armed_at)
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                ),
            );
        }
        if remaining_ms == 0 {
            self.continuation_cutoff_reached = true;
            return false;
        }
        let deadline = now + Duration::from_millis(remaining_ms);
        self.continuation_pending_until = Some(deadline);
        self.continuation_pending_anchor_speech_end_ms = Some(confirmed_speech_end_ms);
        self.continuation_pending_anchor_canonical_speech_serial =
            Some(self.canonical_speech_serial);
        self.continuation_cutoff_reached = false;
        log::info!(
            "[asr] endpoint transition action=continuation_pending generation={} deadline_in_ms={} armed_age_ms={} confirmed_speech_end_ms={:?} current_audio_ms={} cutoff_audio_ms={} canonical_speech_serial={} visible_body_ends_terminal={:?}",
            self.generation,
            deadline.saturating_duration_since(now).as_millis(),
            now.saturating_duration_since(armed_at).as_millis(),
            confirmed_speech_end_ms,
            current_audio_ms,
            continuation_end_audio_ms,
            self.canonical_speech_serial,
            self.latest_visible_body_ends_terminal,
        );
        true
    }

    fn pause_arm_for_provisional_tail(&mut self) {
        if let Some(armed_at) = self.armed_at {
            self.paused_armed_target_end_ms = self.armed_target_end_ms;
            self.paused_armed_at = Some(armed_at);
        }
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = None;
        self.armed_from_visible_body_fallback = false;
    }

    fn clear_paused_arm(&mut self) {
        self.paused_armed_target_end_ms = None;
        self.paused_armed_at = None;
    }

    fn elapsed_ms(started_at: Option<Instant>, now: Instant) -> Option<u64> {
        started_at.map(|started_at| {
            now.saturating_duration_since(started_at)
                .as_millis()
                .min(u64::MAX as u128) as u64
        })
    }

    fn log_pending_owner_tail_transition(
        &self,
        source: &'static str,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
        generation_before: u64,
        armed_at_before: Option<Instant>,
        armed_target_before: Option<u64>,
        recent_strong_non_target: bool,
    ) {
        log::info!(
            "[asr] endpoint transition action={} source={} generation_before={} generation_after={} body_started={} armed_before_age_ms={:?} armed_after_age_ms={:?} paused_deadline_age_ms={:?} armed_target_before_ms={:?} paused_target_after_ms={:?} armed_target_after_ms={:?} pending_owner_tail=true recent_strong_non_target={} unresolved_owner_tail=true pending_provider_text={} audio_ms={:?} provider_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} qualified_owner_advanced={} target_activity_advanced={} pending_activity_advanced={} classification={:?} quality={:?} observation_end_ms={:?} speaker_info_present={}",
            if armed_at_before.is_some() {
                "pause"
            } else {
                "hold_unarmed"
            },
            source,
            generation_before,
            self.generation,
            body_started,
            Self::elapsed_ms(armed_at_before, now),
            Self::elapsed_ms(self.armed_at, now),
            Self::elapsed_ms(self.paused_armed_at, now),
            armed_target_before,
            self.paused_armed_target_end_ms,
            self.armed_target_end_ms,
            recent_strong_non_target,
            update.pending_unattributed_speech,
            update.audio_duration_ms,
            update.provider_audio_duration_ms,
            update.local_speech_end_ms,
            update.qualified_owner_speech_end_ms,
            update.qualified_owner_activity_advanced,
            update.target_activity_advanced,
            update.pending_activity_advanced,
            update.local_speaker_classification_kind,
            update.local_speaker_signal_quality_sufficient,
            update.local_speaker_observation_end_ms,
            update.speaker_info_present,
        );
    }

    fn log_endpoint_arm_transition(
        &self,
        source: &'static str,
        reason: &'static str,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
        generation_before: u64,
        armed_at_before: Option<Instant>,
        armed_target_before: Option<u64>,
        paused_armed_at_before: Option<Instant>,
    ) {
        log::info!(
            "[asr] endpoint transition action={} source={} reason={} generation_before={} generation_after={} body_started={} armed_before_age_ms={:?} armed_after_age_ms={:?} paused_before_age_ms={:?} armed_target_before_ms={:?} armed_target_after_ms={:?} pending_was_seen={} pending_provider_text={} audio_ms={:?} provider_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} qualified_owner_advanced={} target_activity_advanced={} pending_activity_advanced={} classification={:?} quality={:?} observation_end_ms={:?} speaker_info_present={}",
            if reason == "restore_paused_deadline" {
                "restore"
            } else if armed_at_before.is_some() {
                "rearm"
            } else {
                "arm"
            },
            source,
            reason,
            generation_before,
            self.generation,
            body_started,
            Self::elapsed_ms(armed_at_before, now),
            Self::elapsed_ms(self.armed_at, now),
            Self::elapsed_ms(paused_armed_at_before, now),
            armed_target_before,
            self.armed_target_end_ms,
            self.pending_was_seen,
            update.pending_unattributed_speech,
            update.audio_duration_ms,
            update.provider_audio_duration_ms,
            update.local_speech_end_ms,
            update.qualified_owner_speech_end_ms,
            update.qualified_owner_activity_advanced,
            update.target_activity_advanced,
            update.pending_activity_advanced,
            update.local_speaker_classification_kind,
            update.local_speaker_signal_quality_sufficient,
            update.local_speaker_observation_end_ms,
            update.speaker_info_present,
        );
    }

    /// Returns a generation token when a new one-second timer must be started.
    fn observe(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
    ) -> Option<u64> {
        self.observe_with_local_vad_revision(update, body_started, now, None)
    }

    fn observe_with_local_vad_revision(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
        local_vad_revision: Option<u64>,
    ) -> Option<u64> {
        if self.personal_presence_endpoint.is_some() {
            self.latest_update = Some(update.clone());
            self.latest_update_at = Some(now);
            if body_started && self.automatic_no_body_armed { self.leave_automatic_no_body_mode(); }
            return None;
        }
        if local_vad_revision.is_some() {
            self.manual_vad_guard = true;
        }
        self.note_qualified_owner_vad_turn(update);
        if self.automatic_no_body_armed {
            if !body_started {
                self.observe_automatic_no_body_speech(update, now);
                self.latest_update = Some(update.clone());
                return None;
            }
            // The first accepted body is a real owner-session transition. It
            // replaces the wake-only deadline with the ordinary owner clock.
            self.leave_automatic_no_body_mode();
        }
        // Keep the previous fused owner boundary before replacing the cached
        // update.  A newer boundary is not automatically new speech: cloud
        // diarization can publish a late/stable row for audio that was already
        // consumed.  Only a boundary accompanied by a fresh activity edge may
        // restart the wall-clock endpoint.
        let previous_local_owner_end_ms = self
            .latest_update
            .as_ref()
            .and_then(|previous| previous.qualified_owner_speech_end_ms);
        let previous_local_target_end_ms = self
            .latest_update
            .as_ref()
            .and_then(|previous| previous.local_target_speech_end_ms);
        let continuation_activity_rearm = (self.manual_vad_guard
            && self
                .continuation_pending_anchor_canonical_speech_serial
                .is_some_and(|anchor_serial| self.canonical_speech_serial > anchor_serial))
            // Automatic wake sessions observe VAD only as a STOP veto. With
            // local tracking, only a newer owner-specific edge can restart
            // the deadline; another person's raw speech must not do so.
            || (self.automatic_wake_session
                && self
                    .continuation_pending_anchor_speech_end_ms
                    .is_some_and(|anchor_ms| {
                        let current_speech_end_ms = if update.local_speaker_tracking_enabled {
                            update.local_target_speech_end_ms
                        } else {
                            update.local_speech_end_ms
                        };
                        current_speech_end_ms.is_some_and(|speech_ms| speech_ms > anchor_ms)
                    }));
        if continuation_activity_rearm {
            log::info!(
                "[asr] endpoint transition action=cancel_continuation source=canonical_local_vad_speech generation={} anchor_canonical_speech_serial={:?} current_canonical_speech_serial={} current_speech_end_ms={:?}",
                self.generation,
                self.continuation_pending_anchor_canonical_speech_serial,
                self.canonical_speech_serial,
                update.local_speech_end_ms,
            );
            self.reset_continuation_tracking();
        }
        // A late provider row can repeat the same local audio snapshot. Its
        // arrival does not make that old microphone evidence fresh again.
        let local_vad_revision_changed = local_vad_revision.is_some_and(|revision| {
            self.latest_local_vad_revision
                .is_none_or(|previous| revision > previous)
        });
        if local_vad_revision_changed {
            self.latest_local_vad_revision = local_vad_revision;
        }
        let local_timeline_changed = local_vad_revision_changed
            || self.latest_update.as_ref().is_none_or(|previous| {
                previous.audio_duration_ms != update.audio_duration_ms
                    || previous.local_speech_end_ms != update.local_speech_end_ms
            });
        self.latest_update = Some(update.clone());
        if local_timeline_changed {
            self.latest_update_at = Some(now);
        }
        // Positive-evidence bookkeeping comes before every gate below: a local
        // qualified/Target watermark advance is the voiceprint saying the owner
        // really spoke (see EMBEDDED_OWNER_POSITIVE_EVIDENCE_BUDGET_MS).
        let local_owner_activity_advanced = update
            .qualified_owner_speech_end_ms
            .zip(previous_local_owner_end_ms)
            .is_some_and(|(new_end_ms, previous_end_ms)| new_end_ms > previous_end_ms);
        let local_target_activity_advanced = update
            .local_target_speech_end_ms
            .zip(previous_local_target_end_ms)
            .is_some_and(|(new_end_ms, previous_end_ms)| new_end_ms > previous_end_ms);
        if local_owner_activity_advanced
            || local_target_activity_advanced
            || update.qualified_owner_activity_advanced
        {
            self.note_positive_owner_evidence(now);
        }
        let positive_evidence_live = self.positive_owner_evidence_live(now);
        let recent_strong_non_target = update_has_recent_strong_non_target(update);
        if self.automatic_wake_session && update.local_speaker_tracking_enabled {
            if self.owner_spoke_after_body_delivery(update) || recent_strong_non_target {
                self.tentative_owner_continuation_at = None;
                self.tentative_owner_continuation_audio_ms = None;
                self.tentative_owner_vad_epoch = None;
                self.tentative_owner_quiet_audio_ms = None;
            } else if let Some(candidate_audio_ms) = update.local_tentative_owner_speech_end_ms {
                if self
                    .last_body_delivery_audio_ms
                    .is_some_and(|delivery_audio_ms| candidate_audio_ms > delivery_audio_ms)
                    && self
                        .tentative_owner_continuation_audio_ms
                        .is_none_or(|previous_ms| candidate_audio_ms > previous_ms)
                {
                    self.tentative_owner_continuation_at.get_or_insert(now);
                    self.tentative_owner_continuation_audio_ms = Some(candidate_audio_ms);
                    if let Some(evidence) = self.latest_local_vad_evidence {
                        if matches!(
                            evidence.state,
                            crate::asr::volcengine::LocalSpeechActivityState::PendingSpeech
                                | crate::asr::volcengine::LocalSpeechActivityState::Speech
                        ) && evidence.analyzed_through_ms.saturating_add(AUTOMATIC_STOP_VAD_MAX_LAG_MS)
                            >= candidate_audio_ms
                        {
                            self.tentative_owner_vad_epoch = Some(evidence.activity_epoch);
                            self.tentative_owner_quiet_audio_ms = None;
                        }
                    }
                }
            }
        }
        let pending_was_seen_before = self.pending_was_seen;
        let mut pending_snapshot = update.clone();
        pending_snapshot.target_speech_end_ms = pending_snapshot.target_speech_end_ms
            .or(self.armed_target_end_ms).or(self.paused_armed_target_end_ms);
        let pending_owner_tail = positive_evidence_live
            && update.pending_unattributed_speech
            && !recent_strong_non_target
            && has_unresolved_recent_owner_speech(
                &pending_snapshot,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            );
        if pending_owner_tail {
            let generation_before = self.generation;
            let armed_at_before = self.armed_at;
            let armed_target_before = self.armed_target_end_ms;
            let should_log = !pending_was_seen_before || armed_at_before.is_some();
            self.pending_was_seen = true;
            if armed_at_before.is_some() {
                self.pause_arm_for_provisional_tail();
            }
            if should_log {
                self.log_pending_owner_tail_transition(
                    "observe",
                    update,
                    body_started,
                    now,
                    generation_before,
                    armed_at_before,
                    armed_target_before,
                    recent_strong_non_target,
                );
            }
            // Waiting for provider text must not erase a newer, positively
            // verified owner turn. af4abec6 advanced 5400 -> 13400 ms here,
            // then restored the original 8.9-second-old paused deadline when
            // that tail settled. Replace its origin now; replayed flags,
            // cloud-only revisions and another speaker cannot renew it.
            if local_owner_activity_advanced && self.paused_armed_at.is_some() {
                self.paused_armed_at = Some(now);
                self.paused_armed_target_end_ms = update.qualified_owner_speech_end_ms;
                self.generation = self.generation.wrapping_add(1);
                self.stop_proposed = false;
                self.product_endpoint.arm(product_endpoint_evidence(
                    update,
                    self.paused_armed_target_end_ms,
                    true,
                ));
                log::info!(
                    "[asr] endpoint paused deadline renewed by verified owner generation={} owner_end_ms={:?}",
                    self.generation,
                    self.paused_armed_target_end_ms,
                );
            }
            return None;
        }

        let stable_cloud_target_end_ms =
            (update.speaker_info_present && update.speaker_id.is_some())
            .then_some(update.target_speech_end_ms)
            .flatten();
        // The local verifier is an equally authoritative owner clock once it
        // reports Target. Installed multi-interference session c96a7159 had a
        // fresh local owner edge at 4500 ms while the cloud boundary remained
        // at 3182 ms. The old clock compared only the cloud value, so the
        // already-due 900 ms timer stopped at 4600 ms -- just 100 ms after the
        // owner had spoken. Rearm on the fused confirmed-owner boundary; raw
        // VAD and NonTarget observations still cannot move this value.
        let stable_owner_end_ms = owner_endpoint_boundary_with_uncertain_tail(
            update,
            stable_cloud_target_end_ms,
        );
        let qualified_owner_activity_advanced = update.qualified_owner_activity_advanced
            || local_owner_activity_advanced;
        let provider_activity_allowed = !update.local_speaker_tracking_enabled
            || update.qualified_owner_speech_end_ms.is_none();
        // Cloud-only activity (target/pending advance with no local
        // qualified edge) may only re-arm while positive evidence is live:
        // after the budget expires it is exactly the noise-fed loop that
        // made auto-end hang (rearm reason=provider_activity_advanced).
        let fresh_owner_activity = qualified_owner_activity_advanced
            || local_target_activity_advanced
            || (positive_evidence_live
                && provider_activity_allowed
                && (update.target_activity_advanced || update.pending_activity_advanced));
        // Installed session be0c3e6e: the verifier established the owner, then
        // later windows went Uncertain while local speech and the unattributed
        // preview kept growing with no cloud speaker row and no other-speaker
        // evidence. That speakerless continuing body must rearm on the new
        // speech edge instead of letting the stale watermark stop mid-body.
        // Bounded by the positive-evidence budget like every other
        // non-attributed continuation signal.
        let speakerless_continuing_owner_speech = positive_evidence_live
            && update.local_speaker_tracking_enabled
            && !update.speaker_info_present
            && stable_cloud_target_end_ms.is_none()
            && update.local_speech_end_ms.is_some_and(|speech_ms| {
                self.armed_target_end_ms.is_none_or(|armed_end_ms| speech_ms > armed_end_ms)
            });
        // Restore uses the watermark comparison, not the provider-sourced
        // `qualified_owner_activity_advanced` flag: a merged cloud row can
        // re-emit that flag against an unchanged watermark (installed 1284),
        // and a flag-only update must still restore the paused deadline. A
        // paused tail followed by genuinely fresh owner speech (settled
        // re-publication) arms from its own publication time instead.
        // An expired positive-evidence budget forces the restore: leaving the
        // clock paused forever on a provisional tail that can no longer be
        // owner speech is the other half of the "不能自动结束" hang.
        let restore_paused_owner_deadline = self.paused_armed_at.is_some()
            && ((self.pending_was_seen
                && !local_owner_activity_advanced
                && !local_target_activity_advanced)
                || !positive_evidence_live);
        // A qualified-flag rearm replaces an existing armed boundary only when
        // the new fused boundary IS the local qualified authority and differs
        // from the armed one (installed 1284: a cloud row merged room speech
        // into the owner id and re-emitted the flag against an identical
        // watermark; re-arming the same value would just restart the wall
        // clock and hang auto-end). A pull-back to the qualified watermark —
        // for example replacing a stale bridged cloud boundary — is a genuine
        // authority replacement; flicker in the fresh-target edge alignment
        // (installed 531) is not.
        let qualified_flag_replaces_boundary = qualified_owner_activity_advanced
            && stable_owner_end_ms == qualified_owner_speech_end_ms(update)
            && self
                .armed_target_end_ms
                .is_some_and(|armed_end_ms| stable_owner_end_ms != Some(armed_end_ms));
        let stable_target_should_rearm = stable_owner_end_ms.is_some()
            && ((self.armed_at.is_none() && self.paused_armed_at.is_none())
                || local_owner_activity_advanced
                || restore_paused_owner_deadline
                || local_target_activity_advanced
                || speakerless_continuing_owner_speech
                || qualified_flag_replaces_boundary
                || self
                    .armed_target_end_ms
                    .is_none_or(|armed_end_ms| {
                        stable_owner_end_ms > Some(armed_end_ms) && fresh_owner_activity
                    }));
        // Some valid Volcengine previews arrive before diarization publishes a
        // speaker id. Once visible body text exists, arm a wall-clock fallback
        // instead of leaving the session entirely dependent on noisy firmware
        // VAD. A provisional tail still cancels the clock above.
        let unattributed_visible_body_should_arm =
            stable_owner_end_ms.is_none() && self.armed_at.is_none();
        // Two consecutive very-low voiceprint windows identify current room
        // speech as a likely second speaker. Keep the already-running owner
        // timer in that state: provider diarization can temporarily fold both
        // people into one speaker id, and allowing either provider growth or
        // preview growth to re-arm here makes auto-end wait forever. This is
        // endpoint-only; it does not discard or rewrite recognized text.
        let delivery_without_new_owner = self.last_body_delivery_at.is_some()
            && !self.owner_spoke_after_body_delivery(update);
        let should_rearm = body_started
            && (stable_target_should_rearm
                || unattributed_visible_body_should_arm
                || continuation_activity_rearm)
            && (!recent_strong_non_target || self.armed_at.is_none())
            && !(delivery_without_new_owner && self.armed_at.is_some());
        self.pending_was_seen = false;
        if !should_rearm {
            if pending_was_seen_before {
                log::info!(
                    "[asr] endpoint transition action=noop source=observe reason=pending_tail_cleared_without_rearm generation={} body_started={} recent_strong_non_target={} stable_owner_end_ms={:?} qualified_owner_advanced={} owner_boundary_advanced={} fresh_owner_activity={} armed_present={} paused_present={} audio_ms={:?} provider_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} classification={:?}",
                    self.generation,
                    body_started,
                    recent_strong_non_target,
                    stable_owner_end_ms,
                    qualified_owner_activity_advanced,
                    stable_owner_end_ms.is_some_and(|new_end_ms| {
                        self.armed_target_end_ms
                            .is_none_or(|armed_end_ms| new_end_ms > armed_end_ms)
                    }),
                    fresh_owner_activity,
                    self.armed_at.is_some(),
                    self.paused_armed_at.is_some(),
                    update.audio_duration_ms,
                    update.provider_audio_duration_ms,
                    update.local_speech_end_ms,
                    update.qualified_owner_speech_end_ms,
                    update.local_speaker_classification_kind,
                );
            }
            return None;
        }

        let generation_before = self.generation;
        let armed_at_before = self.armed_at;
        let armed_target_before = self.armed_target_end_ms;
        let paused_armed_at_before = self.paused_armed_at;
        let reason = if restore_paused_owner_deadline {
            "restore_paused_deadline"
        } else if local_owner_activity_advanced {
            "qualified_owner_advanced"
        } else if local_target_activity_advanced {
            "local_target_edge_advanced"
        } else if speakerless_continuing_owner_speech {
            "speakerless_continuing_speech"
        } else if qualified_flag_replaces_boundary {
            "qualified_owner_replaced_boundary"
        } else if continuation_activity_rearm {
            "continuation_activity"
        } else if unattributed_visible_body_should_arm {
            "visible_body_fallback"
        } else {
            "provider_activity_advanced"
        };
        self.generation = self.generation.wrapping_add(1);
        if restore_paused_owner_deadline {
            self.armed_target_end_ms = self.paused_armed_target_end_ms;
            self.armed_at = self.paused_armed_at;
            // The paused boundary already came from visible owner text, and
            // the current frame now has sustained local other-speaker proof.
            // Do not require that provisional frame to repeat the cloud id.
            self.armed_from_visible_body_fallback = true;
        } else {
            // The speakerless continuing body (be0c3e6e) has no fused identity
            // boundary for its new speech; the live speech edge is the only
            // defensible stop origin.
            self.armed_target_end_ms = if speakerless_continuing_owner_speech {
                update.local_speech_end_ms.or(stable_owner_end_ms)
            } else {
                stable_owner_end_ms
            };
            self.armed_at = Some(now);
            // The session's first arm seeds the positive-evidence budget;
            // only genuinely positive signals refresh it afterwards.
            if self.last_positive_owner_evidence_at.is_none() {
                self.note_positive_owner_evidence(now);
            }
            // Local Target can rearm the clock before cloud diarization catches
            // up, but local-only visible text still uses the existing guarded
            // visible-body fallback authority.
            self.armed_from_visible_body_fallback = stable_cloud_target_end_ms.is_none();
        }
        self.clear_paused_arm();
        self.stop_proposed = false;
        // Arm/reopen only consume the watermark; the evidence flags are
        // irrelevant here, so pass an unbounded budget bit.
        self.product_endpoint.arm(product_endpoint_evidence(
            update,
            self.armed_target_end_ms,
            true,
        ));
        self.log_endpoint_arm_transition(
            "observe",
            reason,
            update,
            body_started,
            now,
            generation_before,
            armed_at_before,
            armed_target_before,
            paused_armed_at_before,
        );
        Some(self.generation)
    }

    #[cfg(test)]
    fn due_update(
        &mut self,
        generation: u64,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        if self.generation != generation {
            return None;
        }
        self.latest_due_update(now, timeout_ms)
    }

    fn take_due_hold_diagnostic(&mut self) -> Option<(u64, &'static str)> {
        self.pending_due_hold_diagnostic.take()
    }

    fn note_session_policy_hold(&mut self, reason: &'static str) {
        self.note_due_hold_diagnostic(self.generation, reason);
    }

    fn note_due_hold_diagnostic(&mut self, generation: u64, reason: &'static str) {
        let diagnostic = (generation, reason);
        if self.last_reported_due_hold_diagnostic != Some(diagnostic) {
            self.last_reported_due_hold_diagnostic = Some(diagnostic);
            self.pending_due_hold_diagnostic = Some(diagnostic);
        }
    }

    fn arm_latest_for_visible_body(&mut self, now: Instant, body_started: bool) -> Option<u64> {
        // A provider callback can carry only the wake phrase (or an
        // incomplete wake prefix). It must not release the longer automatic
        // no-body window and re-enter the ordinary body endpoint path.
        if !body_started {
            return None;
        }
        if self.personal_presence_endpoint.is_some() {
            // A preview may contain only background words. The independent
            // presence clock already owns endpointing, including first body.
            return None;
        }
        self.leave_automatic_no_body_mode();
        let update = self.latest_update.clone()?;
        // The delivered body is the user's clock. A delayed preview revision
        // of already captured audio cannot create a second three-second wait.
        if self.last_body_delivery_at.is_some()
            && !self.owner_spoke_after_body_delivery(&update)
            && self.armed_at.is_some()
        {
            self.manual_terminal_bridge_rearm_pending = false;
            self.manual_terminal_bridge_until = None;
            return None;
        }
        let recent_strong_non_target = update_has_recent_strong_non_target(&update);
        let pending_was_seen_before = self.pending_was_seen;
        // Preview growth has already refreshed the budget before this call
        // (note_visible_body_boundary runs first); a settled preview with an
        // expired budget must not re-enter the provisional-tail pause loop.
        let pending_owner_tail = self.positive_owner_evidence_live(now)
            && update.pending_unattributed_speech
            && !recent_strong_non_target
            && has_unresolved_recent_owner_speech(
                &update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            );
        if pending_owner_tail {
            let generation_before = self.generation;
            let armed_at_before = self.armed_at;
            let armed_target_before = self.armed_target_end_ms;
            let should_log = !pending_was_seen_before || armed_at_before.is_some();
            self.pending_was_seen = true;
            if armed_at_before.is_some() {
                self.pause_arm_for_provisional_tail();
            }
            if should_log {
                self.log_pending_owner_tail_transition(
                    "visible_preview",
                    &update,
                    true,
                    now,
                    generation_before,
                    armed_at_before,
                    armed_target_before,
                    recent_strong_non_target,
                );
            }
            return None;
        }
        if self.armed_at.is_some() && recent_strong_non_target {
            return None;
        }
        let stable_cloud_target_end_ms =
            (update.speaker_info_present && update.speaker_id.is_some())
            .then_some(update.target_speech_end_ms)
            .flatten();
        let stable_owner_end_ms = owner_endpoint_boundary_with_uncertain_tail(
            &update,
            stable_cloud_target_end_ms,
        );
        let restore_paused_owner_deadline = self.pending_was_seen
            && self.paused_armed_at.is_some()
            && !update.qualified_owner_activity_advanced;
        let owner_boundary_advanced = stable_owner_end_ms.is_some_and(|new_end_ms| {
            self.armed_target_end_ms
                .is_none_or(|armed_end_ms| new_end_ms > armed_end_ms)
        });
        let provider_activity_allowed = !update.local_speaker_tracking_enabled
            || update.qualified_owner_speech_end_ms.is_none();
        let fresh_owner_activity = update.qualified_owner_activity_advanced
            || (provider_activity_allowed
                && (update.target_activity_advanced || update.pending_activity_advanced));
        // Preview growth is display evidence only. Once the manual endpoint
        // has entered its bounded continuation stage, delayed provider text
        // must not create another silence deadline.
        if self.continuation_pending_until.is_some() || self.continuation_cutoff_reached {
            self.manual_terminal_bridge_until = None;
            self.manual_terminal_bridge_rearm_pending = false;
            return None;
        }
        // This method is called from every visible preview callback.  A
        // callback is not itself fresh owner speech: restarting the wall clock
        // here makes a long but already-settled preview wait forever.  Re-arm
        // only for a strictly newer fused owner boundary or restoration of a
        // paused deadline after explicit other-speaker evidence.
        if self.armed_at.is_some()
            && !owner_boundary_advanced
            && !restore_paused_owner_deadline
            && !self.manual_terminal_bridge_rearm_pending
        {
            return None;
        }
        let generation_before = self.generation;
        let armed_at_before = self.armed_at;
        let armed_target_before = self.armed_target_end_ms;
        let paused_armed_at_before = self.paused_armed_at;
        let reason = if restore_paused_owner_deadline {
            "restore_paused_deadline"
        } else if update.qualified_owner_activity_advanced || fresh_owner_activity {
            "owner_activity_advanced"
        } else if self.manual_terminal_bridge_rearm_pending {
            "manual_terminal_bridge"
        } else {
            "visible_body_fallback"
        };
        self.generation = self.generation.wrapping_add(1);
        if restore_paused_owner_deadline {
            self.armed_target_end_ms = self.paused_armed_target_end_ms;
            self.armed_at = self.paused_armed_at;
            self.armed_from_visible_body_fallback = true;
        } else if self.armed_at.is_some() && owner_boundary_advanced && !fresh_owner_activity {
            // A late provider boundary is informational only.  Keep the
            // already-running deadline instead of restarting it from callback
            // arrival time; otherwise delayed diarization can make endpointing
            // wait forever while the microphone is quiet.
            return None;
        } else {
            self.armed_target_end_ms = stable_owner_end_ms;
            self.armed_at = Some(now);
            self.armed_from_visible_body_fallback = stable_cloud_target_end_ms.is_none();
        }
        self.clear_paused_arm();
        self.manual_terminal_bridge_rearm_pending = false;
        self.stop_proposed = false;
        // See the observe arm site: only the watermark is consumed here.
        self.product_endpoint.arm(product_endpoint_evidence(
            &update,
            self.armed_target_end_ms,
            true,
        ));
        self.log_endpoint_arm_transition(
            "visible_preview",
            reason,
            &update,
            true,
            now,
            generation_before,
            armed_at_before,
            armed_target_before,
            paused_armed_at_before,
        );
        Some(self.generation)
    }

    fn is_due(&mut self, now: Instant, timeout_ms: u64) -> bool {
        self.poll_body_receipts();
        if self.pending_body_dispatch.is_some() || !self.pending_body_receipts.is_empty() {
            self.note_due_hold_diagnostic(self.generation, "body_receipt_pending");
            return false;
        }
        if self.pending_owner_delivery_active(now) {
            let reason = if self.pending_owner_delivery.as_ref().is_some_and(|pending| pending.recognition_pending) {
                "body_recognition_pending"
            } else { "body_delivery_pending" };
            self.note_due_hold_diagnostic(self.generation, reason);
            return false;
        }
        if self.stop_proposed {
            return false;
        }
        if self.personal_presence_endpoint.is_some() {
            let captured_ms = self.latest_update.as_ref()
                .and_then(|update| update.audio_duration_ms).unwrap_or_default();
            let due = self.personal_presence_stop_due(captured_ms, now, timeout_ms);
            if !due { self.note_due_hold_diagnostic(self.generation, "personal_owner_activity_pending"); }
            return due;
        }
        let Some(armed_at) = self.armed_at else {
            if self.paused_armed_at.is_some() {
                self.note_due_hold_diagnostic(self.generation, "paused_provisional_tail");
            }
            return false;
        };
        if now.saturating_duration_since(armed_at) < Duration::from_millis(timeout_ms) {
            return false;
        }
        let Some(update) = self.latest_for_decision(now, timeout_ms) else {
            self.note_due_hold_diagnostic(self.generation, "no_latest_update");
            return false;
        };
        if self.automatic_no_body_armed {
            // No new post-wake speech for the whole window. Cloud pending
            // state and repeated wake evidence cannot keep this alive.
            let decision = self.product_endpoint.decide_stop(
                crate::speech_decision_kernel::EndpointEvidence::default(),
                now,
                Duration::from_millis(ENDPOINT_PROVIDER_CATCH_UP_GRACE_MS),
            );
            return matches!(decision, crate::speech_decision_kernel::EndpointDecision::Stop);
        }
        if self.last_body_delivery_at.is_some_and(|delivered_at| {
            now.saturating_duration_since(delivered_at)
                < Duration::from_millis(EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS)
        }) {
            return false;
        }
        if self.automatic_wake_session
            && !update_has_recent_strong_non_target(&update)
            && self.tentative_owner_vad_epoch.is_some()
            && self.latest_local_vad_evidence.is_some_and(|evidence| {
                use crate::asr::volcengine::LocalSpeechActivityState;
                if Some(evidence.activity_epoch) != self.tentative_owner_vad_epoch {
                    return false;
                }
                let captured_ms = update.audio_duration_ms.unwrap_or_default();
                match evidence.state {
                    LocalSpeechActivityState::PendingSpeech | LocalSpeechActivityState::Speech => {
                        captured_ms.saturating_sub(evidence.analyzed_through_ms)
                            < AUTOMATIC_STOP_VAD_STALLED_ANALYSIS_ESCAPE_MS
                    }
                    LocalSpeechActivityState::NonSpeech => self
                        .tentative_owner_quiet_audio_ms
                        .is_some_and(|quiet_start_ms| {
                            captured_ms.saturating_sub(evidence.analyzed_through_ms)
                                < AUTOMATIC_STOP_VAD_STALLED_ANALYSIS_ESCAPE_MS
                                && captured_ms.saturating_sub(quiet_start_ms)
                                < EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
                        }),
                    LocalSpeechActivityState::Unknown => false,
                }
            })
        {
            self.note_due_hold_diagnostic(self.generation, "tentative_owner_turn");
            return false;
        }
        if self.automatic_wake_session
            && self.tentative_owner_continuation_at.is_some_and(|candidate_at| {
                now.saturating_duration_since(candidate_at)
                    < Duration::from_millis(TENTATIVE_OWNER_CONTINUATION_MAX_WAIT_MS)
            })
            && !update_has_recent_strong_non_target(&update)
        {
            self.note_due_hold_diagnostic(self.generation, "tentative_owner_continuation");
            return false;
        }
        if self.automatic_wake_session
            && !self.manual_vad_guard
            && self
                .latest_local_vad_evidence
                .is_some_and(|evidence| {
                    automatic_vad_candidate_holds_stop(
                        evidence,
                        &update,
                        self.positive_owner_evidence_live(now),
                    )
                })
        {
            self.note_due_hold_diagnostic(self.generation, "automatic_vad_speech_pending");
            return false;
        }
        if self.maybe_enter_manual_continuation_pending(&update, armed_at, now) {
            self.note_due_hold_diagnostic(self.generation, "continuation_pending");
            return false;
        }
        let positive_evidence_live = self.positive_owner_evidence_live(now);
        if !Self::update_allows_endpoint(
            &update,
            self.armed_from_visible_body_fallback,
            self.latest_visible_body_ends_terminal,
            self.manual_terminal_bridge_until,
            armed_at,
            now,
            positive_evidence_live,
        ) {
            self.note_due_hold_diagnostic(
                self.generation,
                endpoint_hold_reason(&update, self.latest_visible_body_ends_terminal),
            );
            return false;
        }
        let decision = self.product_endpoint.decide_stop(
            product_endpoint_evidence(
                &update,
                self.armed_target_end_ms,
                positive_evidence_live,
            ),
            now,
            Duration::from_millis(ENDPOINT_PROVIDER_CATCH_UP_GRACE_MS),
        );
        match decision {
            crate::speech_decision_kernel::EndpointDecision::Stop => true,
            crate::speech_decision_kernel::EndpointDecision::AwaitingOwnerAnalysis => {
                self.note_due_hold_diagnostic(self.generation, "owner_analysis_in_flight");
                false
            }
            crate::speech_decision_kernel::EndpointDecision::CatchingUp => {
                self.note_due_hold_diagnostic(self.generation, "provider_catch_up");
                false
            }
            crate::speech_decision_kernel::EndpointDecision::Hold => {
                self.note_due_hold_diagnostic(self.generation, "arbiter_hold");
                false
            }
        }
    }

    fn latest_due_update(
        &mut self,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        if !self.is_due(now, timeout_ms) {
            return None;
        }
        // The STOP consumer must see the same aged evidence as the reducer.
        // Returning the raw snapshot resurrected the wake phrase as fresh
        // speech after a terminal BLE segment and vetoed every due stop.
        let update = self.latest_for_decision(now, timeout_ms);
        // Consume the proposal once. A failed BLE STOP explicitly reopens it.
        self.stop_proposed = true;
        update
    }

    fn latest_due_update_after_owner_analysis(
        &mut self,
        now: Instant,
        timeout_ms: u64,
        owner_analysis_pending: bool,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        self.product_endpoint.note_owner_analysis_pending(
            owner_analysis_pending,
            now,
            Duration::from_millis(OWNER_ANALYSIS_IN_FLIGHT_MAX_WAIT_MS),
        );
        self.latest_due_update(now, timeout_ms)
    }

    /// The only product-level endpoint decision entry used by both provider
    /// callbacks and the watchdog. Evidence producers may update this clock,
    /// but they cannot duplicate session-grace or timeout policy around it.
    fn reduce_session_policy(
        &mut self,
        now: Instant,
        policy: TargetSpeakerEndpointPolicy,
        decision_snapshot: &crate::asr::volcengine::TargetSpeakerUpdate,
        owner_analysis_pending: bool,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        self.reduce_session_policy_with_local_vad_revision(
            now,
            policy,
            decision_snapshot,
            owner_analysis_pending,
            None,
        )
    }

    fn reduce_session_policy_with_local_vad_revision(
        &mut self,
        now: Instant,
        policy: TargetSpeakerEndpointPolicy,
        decision_snapshot: &crate::asr::volcengine::TargetSpeakerUpdate,
        owner_analysis_pending: bool,
        local_vad_revision: Option<u64>,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        self.manual_vad_guard = local_vad_revision.is_some();
        // Manual streams may have no diarization id, so local PCM advances
        // without a target-speaker callback. Always consume a newer capture
        // snapshot before aging provider evidence. Otherwise a cloud pause
        // turns still-live microphone speech into artificial silence.
        let local_capture_advanced = self.latest_update.as_ref().is_some_and(|previous| {
            decision_snapshot.audio_duration_ms > previous.audio_duration_ms
                || decision_snapshot.local_speech_end_ms > previous.local_speech_end_ms
        });
        let local_vad_revision_changed = local_vad_revision.is_some_and(|revision| {
            self.latest_local_vad_revision
                .is_none_or(|previous| revision > previous)
        });
        if local_capture_advanced || local_vad_revision_changed {
            self.observe_with_local_vad_revision(
                decision_snapshot,
                policy.body_started,
                now,
                local_vad_revision,
            );
        }
        if let Some(started_at) = policy.automatic_no_body_started_at {
            self.arm_automatic_no_body_if_needed(decision_snapshot, started_at, now);
        }
        if policy.initial_body_wait_active {
            self.note_session_policy_hold("automatic_body_initial_wait");
            return None;
        }
        // No body exists for a classifier to protect. An in-flight analysis
        // belongs to the already accepted wake phrase and must not create a
        // second post-window wait.
        let owner_analysis_pending = owner_analysis_pending
            && policy.automatic_no_body_started_at.is_none()
            // A sustained, current Other decision resolves this turn. A
            // background source chunk may still be running for diagnostics,
            // but it cannot keep a confirmed foreign tail alive.
            && !update_has_recent_strong_non_target(decision_snapshot);
        self.latest_due_update_after_owner_analysis(
            now,
            policy.wall_clock_timeout_ms,
            owner_analysis_pending,
        )
    }

    /// Return one immutable decision snapshot for the session reducer.
    ///
    /// Provider/local callbacks are not a clock: under a provider stall the
    /// last callback can remain unchanged while the microphone session keeps
    /// running. Keeping its `pending` and local-tail bits forever made every
    /// later fix depend on another escape hatch. After one endpoint interval
    /// without a fresh observation, the snapshot is explicitly classified as
    /// stale provider evidence. The watchdog remains the only caller that can
    /// turn that classification into a stop.
    fn latest_for_decision(
        &self,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        let mut update = self.latest_update.clone()?;
        let stale = self.latest_update_at.is_some_and(|observed_at| {
            now.saturating_duration_since(observed_at) >= Duration::from_millis(
                timeout_ms.max(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
            )
        });
        // Silence ages from the last observation even before the provider is
        // stale; a frozen speech timestamp must not remain fresh forever.
        if let Some(audio_ms) = update.audio_duration_ms.max(update.provider_audio_duration_ms) {
                let elapsed_ms = self.latest_update_at.map_or(0, |observed_at| {
                    now.saturating_duration_since(observed_at).as_millis().min(u64::MAX as u128) as u64
                });
                update.audio_duration_ms = Some(audio_ms.saturating_add(elapsed_ms));
        }
        if stale {
            update.pending_unattributed_speech = false;
            update.pending_activity_advanced = false;
            update.target_activity_advanced = false;
        }
        Some(update)
    }

    fn reopen_after_failed_stop(&mut self) {
        self.stop_proposed = false;
        if let Some(update) = self.latest_update.as_ref() {
            let evidence = if self.automatic_no_body_armed {
                crate::speech_decision_kernel::EndpointEvidence::default()
            } else {
                // See the observe arm site: only the watermark is consumed.
                product_endpoint_evidence(update, self.armed_target_end_ms, true)
            };
            self.product_endpoint.reopen_after_failed_stop(evidence);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EndpointWatchdogStatusSignature {
    phase: SessionPhase,
    body_started: bool,
    preview_present: bool,
    latest_update_present: bool,
    armed: bool,
    paused: bool,
    generation: u64,
    stop_proposed: bool,
    lifecycle: crate::speech_decision_kernel::OwnerEndpointState,
    latest_audio_ms: Option<u64>,
    provider_audio_ms: Option<u64>,
    local_speech_end_ms: Option<u64>,
    qualified_owner_end_ms: Option<u64>,
    hold_reason: Option<&'static str>,
}

/// Session-scoped fallback for callback-order races.
///
/// Provider frames can synchronously emit a speaker update, a streaming
/// preview, and a two-pass supplement. Each can re-arm the generation-based
/// timer. A later local identity update may then invalidate the last queued
/// generation without producing another provider callback, leaving the
/// recording to the firmware's multi-second fallback. Polling the same guarded
/// clock keeps the one-second contract deterministic; `stop_dispatched` still
/// guarantees that this cannot issue a duplicate stop.
fn start_settled_target_endpoint_watchdog(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    stop_completed: &Arc<AtomicBool>,
    stop_state: &Arc<Mutex<EndpointStopDispatchState>>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
) {
    let inner = Arc::downgrade(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let stop_completed = Arc::clone(stop_completed);
    let stop_state = Arc::clone(stop_state);
    let endpoint_clock = Arc::clone(endpoint_clock);
    let asr = Arc::downgrade(asr);
    let watchdog_started_at = Instant::now();
    let asr_instance = asr.as_ptr() as usize;
    log::info!(
        "[asr] endpoint watchdog started session_id={session_id} asr_instance=0x{asr_instance:x}"
    );
    async_runtime::spawn(async move {
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        let mut tick_count = 0_u64;
        let mut first_tick_logged = false;
        let mut last_status_logged_at = Instant::now();
        let mut last_status_signature: Option<EndpointWatchdogStatusSignature> = None;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let (Some(inner), Some(asr)) = (inner.upgrade(), asr.upgrade()) else { return; };
            if inner.shutdown.load(Ordering::SeqCst) { return; }
            tick_count = tick_count.saturating_add(1);
            if stop_completed.load(Ordering::SeqCst) {
                log::info!(
                    "[asr] endpoint watchdog exited session_id={session_id} reason=stop_sent ticks={tick_count} lifetime_ms={}",
                    watchdog_started_at.elapsed().as_millis(),
                );
                return;
            }
            let (session_active, current_session_id, current_phase, cancelled) = {
                let state = inner.state.lock();
                (
                    state.session_id == session_id
                        && !state.cancelled
                        && matches!(
                            state.phase,
                            SessionPhase::Starting | SessionPhase::Listening
                        ),
                    state.session_id,
                    state.phase,
                    state.cancelled,
                )
            };
            if !session_active {
                log::info!(
                    "[asr] endpoint watchdog exited session_id={session_id} reason=session_inactive current_session_id={current_session_id} phase={current_phase:?} cancelled={cancelled} ticks={tick_count} lifetime_ms={}",
                    watchdog_started_at.elapsed().as_millis(),
                );
                return;
            }
            if !first_tick_logged {
                first_tick_logged = true;
                log::info!(
                    "[asr] endpoint watchdog first tick session_id={session_id} tick={tick_count} elapsed_ms={}",
                    watchdog_started_at.elapsed().as_millis(),
                );
            }
            let preview = current_embedded_audio_endpoint_preview(&inner);
            let raw_decision_snapshot = asr.endpoint_update_snapshot();
            let decision_audio_ms = raw_decision_snapshot
                .audio_duration_ms
                .or(raw_decision_snapshot.provider_audio_duration_ms);
            let endpoint_policy = resolve_target_speaker_endpoint_policy(
                &inner,
                session_id,
                preview.as_deref(),
                decision_audio_ms,
            );
            let use_manual_vad = manual_endpoint_vad_allowed(
                &inner,
                session_id,
                endpoint_policy,
                &raw_decision_snapshot,
            );
            let local_vad_evidence = asr.local_speech_activity_snapshot();
            let local_vad_revision = use_manual_vad.then_some(local_vad_evidence.revision);
            let decision_snapshot = if use_manual_vad {
                // Manual sessions use VAD as endpoint evidence. Automatic
                // sessions observe it only as a bounded STOP veto and retain
                // their owner-identity clock.
                asr.endpoint_update_with_local_speech_evidence_snapshot(
                    raw_decision_snapshot.clone(),
                    local_vad_evidence,
                )
            } else {
                raw_decision_snapshot
            };
            let pending_owner_delivery =
                pause_early_pending_body_delivery(&inner, session_id, &asr);
            let (update, hold_diagnostic, personal_owner_advanced) = {
                let mut clock = endpoint_clock.lock();
                let personal_observation = asr.personal_vad_snapshot();
                let personal_owner_advanced = asr.personal_vad_started()
                    && clock.note_personal_presence(personal_observation, Instant::now());
                clock.note_local_vad_evidence_at(local_vad_evidence, Instant::now());
                // If the provider never opened, keep feeding the reducer from
                // the local owner clock. This preserves the same single
                // watchdog decision path while removing generic room-energy
                // from the only remaining fallback.
                if asr.audio_delivery_failed() {
                    clock.observe_with_local_vad_revision(
                        &decision_snapshot,
                        endpoint_policy.body_started,
                        Instant::now(),
                        local_vad_revision,
                    );
                } else if clock.latest_update.is_none()
                    && current_embedded_audio_partial_preview(&inner)
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty())
                {
                    // A provider can deliver preview text while omitting all
                    // target-speaker rows. Seed the same controller from the
                    // live ASR snapshot instead of leaving healthy sessions
                    // without any endpoint clock.
                    clock.seed_from_snapshot_if_missing(
                        asr.endpoint_update_snapshot(),
                        true,
                        Instant::now(),
                    );
                    log::info!(
                        "[asr] endpoint watchdog seeded missing owner clock from provider snapshot session_id={session_id}"
                    );
                }
                let now = Instant::now();
                let owner_analysis_pending = asr.owner_analysis_pending_for_endpoint();
                clock.refresh_pending_body_delivery(pending_owner_delivery, now);
                let update = clock.reduce_session_policy_with_local_vad_revision(
                    now,
                    endpoint_policy,
                    &decision_snapshot,
                    owner_analysis_pending,
                    local_vad_revision,
                );
                let hold_diagnostic = clock.take_due_hold_diagnostic();
                (update, hold_diagnostic, personal_owner_advanced)
            };
            if personal_owner_advanced { note_embedded_asr_speech_activity(&inner, session_id); }
            if let Some((generation, reason)) = hold_diagnostic {
                let lifecycle = endpoint_clock.lock().lifecycle();
                log::info!(
                    "[asr] target endpoint hold session_id={session_id} generation={generation} reason={reason} lifecycle={lifecycle:?}"
                );
            }
            let now = Instant::now();
            let (
                status_signature,
                armed_age_ms,
                paused_age_ms,
                latest_update_age_ms,
                latest_audio_ms,
                provider_audio_ms,
                local_speech_end_ms,
                qualified_owner_end_ms,
                latest_hold_reason,
                lifecycle,
            ) = {
                let clock = endpoint_clock.lock();
                let latest_update = clock.latest_update.as_ref();
                let armed_age_ms = clock
                    .armed_at
                    .map(|at| now.saturating_duration_since(at).as_millis());
                let paused_age_ms = clock
                    .paused_armed_at
                    .map(|at| now.saturating_duration_since(at).as_millis());
                let latest_update_age_ms = clock
                    .latest_update_at
                    .map(|at| now.saturating_duration_since(at).as_millis());
                let latest_audio_ms = latest_update.and_then(|update| update.audio_duration_ms);
                let provider_audio_ms =
                    latest_update.and_then(|update| update.provider_audio_duration_ms);
                let local_speech_end_ms =
                    latest_update.and_then(|update| update.local_speech_end_ms);
                let qualified_owner_end_ms =
                    latest_update.and_then(|update| update.qualified_owner_speech_end_ms);
                let latest_hold_reason = clock
                    .last_reported_due_hold_diagnostic
                    .map(|(_, reason)| reason);
                let lifecycle = clock.lifecycle();
                let status_signature = EndpointWatchdogStatusSignature {
                    phase: current_phase,
                    body_started: endpoint_policy.body_started,
                    preview_present: preview.is_some(),
                    latest_update_present: latest_update.is_some(),
                    armed: clock.armed_at.is_some(),
                    paused: clock.paused_armed_at.is_some(),
                    generation: clock.generation,
                    stop_proposed: clock.stop_proposed,
                    lifecycle,
                    latest_audio_ms,
                    provider_audio_ms,
                    local_speech_end_ms,
                    qualified_owner_end_ms,
                    hold_reason: latest_hold_reason,
                };
                (
                    status_signature,
                    armed_age_ms,
                    paused_age_ms,
                    latest_update_age_ms,
                    latest_audio_ms,
                    provider_audio_ms,
                    local_speech_end_ms,
                    qualified_owner_end_ms,
                    latest_hold_reason,
                    lifecycle,
                )
            };
            let status_changed = last_status_signature != Some(status_signature);
            if status_changed || last_status_logged_at.elapsed() >= Duration::from_secs(1) {
                last_status_logged_at = now;
                last_status_signature = Some(status_signature);
                log::info!(
                    "[asr] endpoint watchdog status session_id={session_id} tick={tick_count} phase={current_phase:?} body_started={} preview_present={} latest_update_present={} armed_age_ms={armed_age_ms:?} paused_age_ms={paused_age_ms:?} latest_update_age_ms={latest_update_age_ms:?} generation={} stop_proposed={} lifecycle={lifecycle:?} hold_reason={latest_hold_reason:?} local_audio_ms={:?} provider_audio_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} owner_analysis_pending={}",
                    endpoint_policy.body_started,
                    preview.is_some(),
                    status_signature.latest_update_present,
                    status_signature.generation,
                    status_signature.stop_proposed,
                    latest_audio_ms,
                    provider_audio_ms,
                    local_speech_end_ms,
                    qualified_owner_end_ms,
                    asr.owner_analysis_pending_for_endpoint(),
                );
            }
            if update.is_none() && endpoint_policy.body_started {
                let growth = inner
                    .embedded_audio_preview
                    .lock()
                    .last_visible_growth_at(session_id);
                let keep_firmware = {
                    let now = Instant::now();
                    let mut clock = endpoint_clock.lock();
                    clock.should_keep_firmware_alive_for_continuation(now)
                        || clock.should_keep_firmware_alive_for_host_hang(
                            endpoint_policy.endpoint_timeout_ms,
                            growth,
                            now,
                        )
                };
                if keep_firmware {
                    note_embedded_asr_speech_activity(&inner, session_id);
                }
            }
            let stop_update_present = update.is_some();
            if let Some(update) = update {
                handle_target_speaker_endpoint_stop(
                    &inner,
                    session_id,
                    &stop_dispatched,
                    &stop_completed,
                    &stop_state,
                    &endpoint_clock,
                    &asr,
                    update,
                    endpoint_policy,
                );
            }
            // 上行黑洞快速判死（16:08 直连实锤；09:23 二次实锤后不再要求
            // body_started——黑洞会话永远等不来第一帧预览，正文闩锁不会翻，
            // 恰好漏掉全聋形态）：先于云端 8s 超时掐线，立即触发保留音频重放。
            if !stop_update_present && !stop_dispatched.load(Ordering::SeqCst) {
                asr.abort_if_uplink_stalled();
                // 2026-09-22 跟手①：无 STOP 待决时的句末稳定评估。稳定前缀当场
                // 上屏，终稿只插余量；任何失败静默回退现行为。
                if endpoint_policy.body_started {
                    // 跟手②组字流式:先喂增量(update),再评估停顿落定(commit),
                    // 同一拍内顺序保证 commit 前组字内容最新。
                    update_live_body_delivery(&inner, session_id, &asr, &endpoint_clock).await;
                }
            }
        }
    });
}

// This boundary owns editor delivery only. Receipt failure cannot grant
// recording-stop permission; the activity/confirmed-body endpoint owns STOP.
async fn update_live_body_delivery(
    inner: &Arc<Inner>,
    session_id: SessionId,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    if !crate::coordinator_state::editor_delivery_allowed(&inner.state.lock(), session_id) {
        return;
    }
    streaming_composition_update_tick(inner, session_id, asr).await;
    native_composition_update_tick(inner, session_id, asr).await;
    pause_early_delivery_tick(inner, session_id, asr, endpoint_clock).await;
}

fn arm_settled_target_endpoint_for_visible_body(
    inner: &Arc<Inner>,
    _session_id: SessionId,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    body_started: bool,
) {
    if !body_started {
        return;
    }
    let preview = current_embedded_audio_partial_preview(inner);
    let preview_ends_terminal = preview_ends_with_sentence_terminal(preview.as_deref());
    let preview_chars = preview.as_deref().map_or(0, |text| text.chars().count());
    let now = Instant::now();
    {
        let mut clock = endpoint_clock.lock();
        clock.note_visible_body_boundary(preview_ends_terminal, preview_chars, now);
        clock.arm_latest_for_visible_body(now, body_started);
    }
}

#[cfg(test)]
mod endpoint_clock_submission_tests {
    use super::*;

    #[test]
    fn question_endpoint_waits_for_recognition_but_never_for_an_editor_receipt() {
        use crate::asr::volcengine::PendingBodyDelivery;
        use crate::personal_vad::{PresenceObservation, PresenceState};
        use crate::voice_command::VoiceAction;
        let now = Instant::now();
        for action in [VoiceAction::Dictation, VoiceAction::Question] {
            for recognition_pending in [true, false] {
                let mut clock = automatic_body_clock();
                clock.latest_update = Some(owner_update(7_000, 4_000));
                clock.note_personal_presence(Some(PresenceObservation {
                    analyzed_end_ms: 7_000, last_positive_end_ms: Some(4_000),
                    last_unresolved_end_ms: None, source_history_complete: true,
                    state: PresenceState::ConfirmedOther, observed_at: now,
                }), now);
                let pending = pending_endpoint_body_for_action(action, Some(PendingBodyDelivery {
                    key: "The actual recognized question".into(), updated_at: now,
                    recognition_pending,
                }));
                clock.refresh_pending_body_delivery(pending, now);
                let expected = action == VoiceAction::Question && !recognition_pending;
                assert_eq!(clock.personal_presence_stop_due(7_000, now, 3_000), expected,
                    "action={action:?} recognition_pending={recognition_pending}");
            }
        }
    }
    use crate::insertion::{
        pending_receipt_handle_for_test, PasteReceiptCompletion, PasteReceiptOutcome,
        PasteReceiptProof,
    };

    #[test]
    #[ignore = "requires selected saved model/native-response evidence; no capture, editor or cloud"]
    fn recorded_unfinished_owner_body_cannot_authorize_physical_stop() {
        use crate::asr::volcengine::PendingBodyDelivery;
        use crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline;
        use crate::personal_vad::{fuse_presence, latest_target_source_end, publish_scored_window,
            PresenceObservation, PresenceState};
        use serde_json::{json, Value};
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_PENDING_BODY_REPLAY").unwrap());
        let read = |name| serde_json::from_slice::<Value>(&std::fs::read(root.join(name)).unwrap()).unwrap();
        let protocol = read("protocol.json");
        let models = read("model-windows.json");
        let bodies = read("native-pending-body.json");
        let state = |value: &Value| match value.as_str().unwrap() {
            "Owner" => PresenceState::Owner, "ConfirmedOther" => PresenceState::ConfirmedOther,
            "Unknown" => PresenceState::Unknown, value => panic!("unknown state {value}"),
        };
        let mut output = Vec::new();
        for case in protocol["cases"].as_array().unwrap().iter().filter(|case| case["ownerStillSpeaking"] == true) {
            let archive: Value = serde_json::from_slice(&std::fs::read(case["archive"].as_str().unwrap()).unwrap()).unwrap();
            let native: Vec<_> = archive["trace"]["entries"].as_array().unwrap().iter()
                .filter(|entry| entry["stage"] == "normalized_result").collect();
            let pending = &bodies["cases"].as_array().unwrap().iter().find(|row|row["id"] == case["id"]).unwrap()["frames"];
            let windows = &models["cases"].as_array().unwrap().iter().find(|row|row["id"] == case["id"]).unwrap()["windows"];
            let base = Instant::now();
            let mut clock = automatic_body_clock();
            let mut history = LocalSpeakerTimeline::default();
            let mut owner_end = 0;
            let mut native_index = 0;
            let mut points = Vec::new();
            for window in windows.as_array().unwrap() {
                let chunks = window["chunks"].as_array().unwrap();
                let scores: Vec<f32> = chunks.iter().map(|chunk|chunk["probability"].as_f64().unwrap() as f32).collect();
                let count = window["newChunks"].as_u64().unwrap() as usize;
                let first = chunks[chunks.len()-count]["endMs"].as_u64().unwrap()*16;
                let identity = state(window.get("endpointIdentity").unwrap_or(&window["identity"]));
                publish_scored_window(&mut history, &scores, first, count, Some(identity)).unwrap();
                owner_end = owner_end.max(latest_target_source_end(&scores, first, count).unwrap_or(0)/16);
                for chunk in &chunks[chunks.len()-count..] {
                    if fuse_presence(chunk["probability"].as_f64().unwrap() as f32, Some(identity)) == PresenceState::Owner {
                        owner_end = owner_end.max(chunk["endMs"].as_u64().unwrap());
                    }
                }
                let audio_ms = window["endMs"].as_u64().unwrap();
                let now = base + Duration::from_millis(audio_ms);
                clock.latest_update = Some(owner_update(audio_ms, owner_end));
                clock.note_personal_presence(Some(PresenceObservation {
                    analyzed_end_ms: audio_ms, last_positive_end_ms: Some(owner_end),
                    last_unresolved_end_ms: history.unresolved_activity_end_ms_after(owner_end),
                    source_history_complete: true, state: fuse_presence(*scores.last().unwrap(), Some(identity)), observed_at:now,
                }), now);
                while native_index < native.len() {
                    let entry = native[native_index];
                    let projection = &entry["speaker_projection"];
                    let source_frontier = ["local_identity_newest_first", "local_presence_newest_first"].iter()
                        .filter_map(|key|projection[*key].as_array()).flatten().filter_map(|span|span[1].as_u64()).max().unwrap_or(0);
                    let word_end = entry["provider_result"]["utterances"].as_array().into_iter().flatten()
                        .filter_map(|row|row["words"].as_array()).flatten().filter_map(|word|word["end_time"].as_u64()).max().unwrap_or(0);
                    if source_frontier.max(word_end) > audio_ms { break; }
                    let body = &pending.as_array().unwrap()[native_index]["pending"];
                    clock.refresh_pending_body_delivery(body["key"].as_str().map(|key| PendingBodyDelivery {
                        key:key.into(), updated_at:base, recognition_pending:body["recognitionPending"].as_bool().unwrap(),
                    }), now);
                    native_index += 1;
                }
                let due = clock.is_due(now, 3_000);
                assert!(!due, "saved unfinished owner capture allowed STOP at {audio_ms}ms");
                points.push(json!({"audioMs":audio_ms,"ownerEndMs":owner_end,"pendingBody":clock.pending_owner_delivery.is_some(),"stopDue":due}));
            }
            output.push(json!({"id":case["id"],"points":points,"nativeFramesUsed":native_index}));
        }
        assert!(!output.is_empty());
        std::fs::write(root.join("production-stop-clock-replay.json"),serde_json::to_vec_pretty(&json!({
            "cases":output,"cloudCalls":0,"actualEditor":false,"actualHardwareStop":false,
            "schedule":"model source clock and recorded native source/word frontiers; real receipts covered separately",
        })).unwrap()).unwrap();
    }

    fn automatic_body_clock() -> SettledTargetEndpointClock {
        SettledTargetEndpointClock {
            automatic_wake_session: true,
            ..Default::default()
        }
    }

    fn owner_update(audio_ms: u64, owner_ms: u64) -> crate::asr::volcengine::TargetSpeakerUpdate {
        crate::asr::volcengine::TargetSpeakerUpdate {
            speaker_id: Some("owner".to_string()),
            target_speech_end_ms: Some(owner_ms),
            provider_audio_duration_ms: Some(audio_ms),
            audio_duration_ms: Some(audio_ms),
            local_speech_end_ms: Some(owner_ms),
            qualified_owner_speech_end_ms: Some(owner_ms),
            qualified_owner_activity_advanced: true,
            local_speaker_classification_kind: Some(
                crate::asr::volcengine::LocalSpeakerClassificationKind::Target,
            ),
            local_speaker_signal_quality_sufficient: Some(true),
            local_speaker_observation_end_ms: Some(owner_ms),
            local_tentative_owner_speech_end_ms: None,
            local_target_speech_end_ms: Some(owner_ms),
            local_non_target_speech_end_ms: None,
            local_speaker_tracking_enabled: true,
            stable_attributed_speech_end_ms: Some(owner_ms),
            target_activity_advanced: true,
            pending_unattributed_speech: false,
            pending_activity_advanced: false,
            speaker_info_present: true,
        }
    }

    #[test]
    fn only_the_matching_dispatch_can_settle_its_in_flight_hold() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        let first = clock
            .begin_text_submission(started, "第一段正文")
            .expect("first dispatch");
        let second = clock
            .begin_text_submission(started + Duration::from_millis(10), "第二段正文")
            .expect("second dispatch");

        clock.settle_text_submission(Some(first), true, None);
        assert_eq!(
            clock.pending_body_dispatch.as_ref().map(|dispatch| dispatch.sequence),
            Some(second)
        );
        assert_eq!(clock.last_body_delivery_at, None);

        clock.settle_text_submission(Some(second), false, None);
        assert!(clock.pending_body_dispatch.is_none());
        assert_eq!(clock.last_body_delivery_at, None);
    }

    #[test]
    fn terminal_editor_receipt_releases_only_its_wait_not_speech_or_unfinished_body() {
        use crate::personal_vad::{PresenceObservation, PresenceState};
        use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence, PendingBodyDelivery};
        // 2026-10-15 ce8801eb: ticket 158 returned Unsupported at 10:01:27.375,
        // after seven confirmed receipts. The outer ticket was removed while
        // the personal endpoint kept waiting through capture 50000 ms/cancel.
        // Quiet and new-owner observations below are explicit component inputs,
        // not claims that the archived live VAD supplied those observations.
        for outcome in [PasteReceiptOutcome::Unsupported, PasteReceiptOutcome::TimedOut,
            PasteReceiptOutcome::Superseded, PasteReceiptOutcome::WorkerUnavailable]
        {
            let started = Instant::now();
            let mut clock = automatic_body_clock();
            clock.latest_update = Some(owner_update(35_500, 34_880));
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: 35_040, last_positive_end_ms: Some(34_880),
                last_unresolved_end_ms: None, source_history_complete: false,
                state: PresenceState::Owner, observed_at: started,
            }), started);
            let sequence = clock.begin_text_submission(started, "最后一批已经发出的文字").unwrap();
            let (handle, completion) = pending_receipt_handle_for_test(158);
            let ticket = handle.ticket();
            clock.settle_text_submission(Some(sequence), true, Some(handle));
            completion.send(PasteReceiptCompletion { ticket,
                observed_at: started + Duration::from_millis(57), outcome }).unwrap();
            clock.poll_body_receipts();
            assert!(clock.pending_body_receipts.is_empty());
            assert!(!clock.personal_presence_endpoint.as_ref().unwrap().due(35_500, started, 3_000),
                "a terminal negative cannot itself stop live owner speech");
            assert!(!clock.personal_presence_stop_due(35_500, started, 3_000),
                "the coordinator still protects the existing continuation window");
            let quiet_at = started + Duration::from_millis(4_500);
            clock.latest_update = Some(owner_update(40_000, 34_880));
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: 40_000, last_positive_end_ms: Some(34_880),
                last_unresolved_end_ms: Some(40_000), source_history_complete: false,
                state: PresenceState::Unknown, observed_at: quiet_at,
            }), quiet_at);
            assert!(!clock.personal_presence_stop_due(40_000, quiet_at, 3_000),
                "an editor failure is not proof of silence or another speaker");
            clock.local_vad_stop_observation = Some(LocalVadStopObservation::new(LocalSpeechEvidence {
                revision: 1, analyzed_through_ms: 40_000,
                state: LocalSpeechActivityState::NonSpeech,
                last_detected_speech_end_ms: Some(35_000),
                ..Default::default()
            }, quiet_at));
            assert!(clock.personal_presence_stop_due(40_000, quiet_at, 3_000),
                "independent completed quiet can release recording after the transaction has ended");
            clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
                key: "还没有确认的下一句".into(), updated_at: quiet_at, recognition_pending: true,
            }), quiet_at);
            assert!(!clock.personal_presence_stop_due(40_000, quiet_at, 3_000),
                "retiring a receipt cannot consume or confirm unfinished recognition");
            clock.refresh_pending_body_delivery(None, quiet_at);
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: 40_160, last_positive_end_ms: Some(40_160),
                last_unresolved_end_ms: None, source_history_complete: false,
                state: PresenceState::Owner, observed_at: quiet_at,
            }), quiet_at);
            assert!(!clock.personal_presence_stop_due(40_160, quiet_at, 3_000),
                "new owner activity must still veto the old independent quiet span");
        }
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "explicit captured startup-burst failure; no cloud/device/editor"]
    async fn saved_startup_burst_keeps_vad_live_and_reaches_real_quiet() {
        use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence};
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_VAD_BURST_REPLAY").unwrap());
        let bytes = std::fs::read(root.join("8ae1f493-2fba-42c3-9580-7dddfc1785d8.wav")).unwrap();
        assert_eq!(&bytes[36..40], b"data");
        let pcm = &bytes[44..];
        let burst_bytes = 54_000 * 32;
        assert!(pcm.len() > burst_bytes);
        let sink = Arc::new(Mutex::new(LocalSpeechEvidence::default()));
        let mut vad = LocalSpeechActivity::new(Arc::clone(&sink));
        // The live log advances 5.8 s in 26 ms, 13 s in 82 ms and 21.8 s
        // in 144 ms, before ordinary live input. Do not pace this history by
        // waiting for its consumer: that hid the installed queue-full failure.
        for (i, chunk) in pcm[..burst_bytes].chunks(3_200).enumerate() {
            vad.submit(i as u64 * 1_600, (i as u64 + 1) * 1_600, chunk);
        }
        let limit = Instant::now() + Duration::from_secs(8);
        while sink.lock().analyzed_through_ms + 32 < 54_000 {
            assert!(Instant::now() < limit, "original startup burst left VAD unavailable: {:?}", *sink.lock());
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let mut observations = vec![*sink.lock()];
        for (i, chunk) in pcm[burst_bytes..].chunks(3_200).enumerate() {
            let start = (burst_bytes / 2 + i * 1_600) as u64;
            let end = start + chunk.len() as u64 / 2;
            vad.submit(start, end, chunk);
            let limit = Instant::now() + Duration::from_secs(2);
            while sink.lock().analyzed_through_samples + 512 < end {
                assert!(Instant::now() < limit, "VAD stopped after accepting startup history");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            observations.push(*sink.lock());
        }
        let last = *sink.lock();
        assert_eq!(last.state, LocalSpeechActivityState::NonSpeech,
            "the original recorded tail must be assessed, not supplied as synthetic silence");
        let now = Instant::now();
        let total_ms = pcm.len() as u64 / 32;
        let source_started = now - Duration::from_millis(total_ms);
        let mut clock = automatic_body_clock();
        clock.note_personal_presence(Some(crate::personal_vad::PresenceObservation {
            analyzed_end_ms: total_ms, last_positive_end_ms: None,
            last_unresolved_end_ms: Some(total_ms), source_history_complete: false,
            state: crate::personal_vad::PresenceState::Unknown, observed_at: now,
        }), now);
        // This incident had three matching Confirmed editor receipts; no
        // unsubmitted body/receipt is invented or retired by this replay.
        for evidence in &observations {
            clock.note_local_vad_evidence_at(*evidence,
                source_started + Duration::from_millis(evidence.analyzed_through_ms));
        }
        let due = clock.personal_presence_stop_due(total_ms, now, 3_000);
        let output = serde_json::json!({"startupBurstMs":54000,"capturedMs":total_ms,
            "completedVadMs":last.analyzed_through_ms,"state":format!("{:?}",last.state),
            "lastSpeechMs":last.last_detected_speech_end_ms,"quietEndpointDue":due,
            "realRecordedTail":true,"cloudCalls":0,"actualEditor":false,"physicalStop":false});
        std::fs::write(root.join("startup-burst-replay.json"),serde_json::to_vec_pretty(&output).unwrap()).unwrap();
        assert!(due,"completed original quiet must not retain a permanent queue-full hold: {last:?}");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "explicit saved Unsupported-receipt failures; no live device, editor or cloud"]
    async fn saved_failed_input_quiet_audio_releases_the_finished_receipt_wait() {
        use crate::personal_vad::{PresenceObservation, PresenceState};
        use sha2::{Digest, Sha256};
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_RECEIPT_FAILURE_REPLAY").unwrap());
        let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("saved-failure-cases.json")).unwrap()).unwrap();
        let mut results = Vec::new();
        for case in cases.as_array().unwrap() {
            let bytes = std::fs::read(case["wav"].as_str().unwrap()).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert_eq!(&bytes[36..40], b"data");
            assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
            let pcm = &bytes[44..];
            let captured_ms = pcm.len() as u64 / 32;
            let trace: serde_json::Value = serde_json::from_slice(&std::fs::read(case["trace"].as_str().unwrap()).unwrap()).unwrap();
            let native = trace["trace"]["entries"].as_array().unwrap().iter().rev()
                .find(|row| row["stage"] == "normalized_result").unwrap();
            let spans = native["speaker_projection"]["local_presence_newest_first"].as_array().unwrap();
            let owner_end = spans.iter().filter(|span| span[2] == "Owner")
                .filter_map(|span| span[1].as_u64()).max().expect("saved owner evidence");
            let analyzed = spans.iter().filter_map(|span| span[1].as_u64()).max().unwrap();
            let unresolved = spans.iter().filter(|span| span[2] == "Unknown")
                .filter_map(|span| span[1].as_u64()).max();
            let sink = Arc::new(Mutex::new(crate::asr::volcengine::LocalSpeechEvidence::default()));
            let mut vad = LocalSpeechActivity::new(Arc::clone(&sink));
            let mut vad_observations = Vec::new();
            // Pace by the real VAD completion frontier so accelerated replay
            // cannot overflow its live queue or manufacture silence by timeout.
            for (i, chunk) in pcm.chunks(3_200).enumerate() {
                let start = (i * 3_200 / 2) as u64;
                let end = start + chunk.len() as u64 / 2;
                vad.submit(start, end, chunk);
                let limit = Instant::now() + Duration::from_secs(3);
                while sink.lock().analyzed_through_ms + 32 < end / 16 {
                    assert!(Instant::now() < limit, "saved VAD did not complete its own input");
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                vad_observations.push(*sink.lock());
            }
            let evidence = *sink.lock();
            let now = Instant::now();
            let mut clock = automatic_body_clock();
            clock.latest_update = Some(owner_update(captured_ms, owner_end));
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: analyzed, last_positive_end_ms: Some(owner_end),
                last_unresolved_end_ms: unresolved, source_history_complete: false,
                state: PresenceState::Unknown, observed_at: now - Duration::from_secs(1),
            }), now);
            let provider_ms = trace["audioDelivery"]["providerAudioDurationMs"].as_u64().unwrap();
            assert!(captured_ms > provider_ms + 3_000);
            let dispatched = now - Duration::from_millis(captured_ms - provider_ms);
            let sequence = clock.begin_text_submission(dispatched, "saved already-dispatched body").unwrap();
            let (handle, completion) = pending_receipt_handle_for_test(90_158);
            let ticket = handle.ticket();
            clock.settle_text_submission(Some(sequence), true, Some(handle));
            completion.send(PasteReceiptCompletion { ticket, observed_at: dispatched,
                outcome: PasteReceiptOutcome::Unsupported }).unwrap();
            clock.poll_body_receipts();
            // Replay the completed observations, not just the last snapshot.
            // A first-ever negative snapshot after an activity epoch cannot
            // retrospectively prove uninterrupted quiet. Live polling observes
            // the intervening epochs and completed negatives in source order.
            let source_started = now - Duration::from_millis(captured_ms);
            for observation in &vad_observations {
                clock.note_local_vad_evidence_at(*observation,
                    source_started + Duration::from_millis(observation.analyzed_through_ms));
            }
            let due = clock.personal_presence_stop_due(captured_ms, now, 3_000);
            results.push(serde_json::json!({"session":case["session"],
                "wavSha256":format!("{:x}",Sha256::digest(&bytes)),"capturedMs":captured_ms,
                "savedOwnerMs":owner_end,"vadAnalyzedMs":evidence.analyzed_through_ms,
                "vadState":format!("{:?}",evidence.state),"vadSpeechEndMs":evidence.last_detected_speech_end_ms,
                "savedTerminalOutcome":"Unsupported","sourceEndpointDue":due,
                "completedVadObservations":vad_observations.len(),
                "acceleratedComponentReplay":true,"actualEditor":false,"actualStop":false}));
            std::fs::write(root.join("saved-failure-replay-result.json"), serde_json::to_vec_pretty(&results).unwrap()).unwrap();
            assert!(due, "saved failed input {} remains blocked: VAD={evidence:?}", case["session"]);
        }
    }

    #[test]
    fn disconnected_or_consumed_editor_handle_cannot_orphan_the_personal_reservation() {
        use crate::personal_vad::{PresenceObservation, PresenceState};
        for consumed in [false, true] {
            let now = Instant::now();
            let mut clock = automatic_body_clock();
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: 4_000, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None, source_history_complete: false,
                state: PresenceState::Owner, observed_at: now,
            }), now);
            let sequence = clock.begin_text_submission(now, "后台输入工作线程已经退出").unwrap();
            let (mut handle, sender) = pending_receipt_handle_for_test(159);
            drop(sender);
            if consumed { let _ = handle.try_take(); }
            clock.settle_text_submission(Some(sequence), true, Some(handle));
            clock.poll_body_receipts();
            let later = now + Duration::from_millis(4_000);
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: 8_000, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None, source_history_complete: true,
                state: PresenceState::ConfirmedOther, observed_at: later,
            }), later);
            assert!(clock.personal_presence_stop_due(8_000, later, 3_000),
                "terminal handles must not keep the old receipt reservation alive");
        }
    }

    #[test]
    fn confirmed_receipt_moves_only_the_wall_clock_and_negative_outcomes_do_not() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        let sequence = clock
            .begin_text_submission(started, "已经提交的正文")
            .expect("dispatch");
        let (handle, completion) = pending_receipt_handle_for_test(701);
        let ticket = handle.ticket();
        clock.settle_text_submission(Some(sequence), true, Some(handle));

        let continued_at = started + Duration::from_millis(15);
        clock.tentative_owner_continuation_at = Some(continued_at);
        clock.tentative_owner_continuation_audio_ms = Some(4_200);
        let observed_at = started + Duration::from_millis(23);
        completion
            .send(PasteReceiptCompletion {
                ticket,
                observed_at,
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            })
            .expect("receipt receiver");
        clock.poll_body_receipts();
        assert_eq!(clock.last_body_delivery_at, Some(observed_at));
        assert_eq!(clock.tentative_owner_continuation_at, Some(continued_at));
        assert_eq!(clock.tentative_owner_continuation_audio_ms, Some(4_200));

        let second_dispatch = started + Duration::from_millis(100);
        let sequence = clock
            .begin_text_submission(second_dispatch, "下一段正文")
            .expect("second dispatch");
        let (handle, completion) = pending_receipt_handle_for_test(702);
        let ticket = handle.ticket();
        clock.settle_text_submission(Some(sequence), true, Some(handle));
        completion
            .send(PasteReceiptCompletion {
                ticket,
                observed_at: second_dispatch + Duration::from_secs(2),
                outcome: PasteReceiptOutcome::Unsupported,
            })
            .expect("receipt receiver");
        clock.poll_body_receipts();
        assert_eq!(clock.last_body_delivery_at, Some(second_dispatch));
        assert!(clock.pending_body_receipts.is_empty());
    }

    #[test]
    fn settle_uses_the_begin_audio_boundary_and_preserves_later_owner_evidence() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.latest_update = Some(owner_update(3_000, 2_900));
        let sequence = clock
            .begin_text_submission(started, "先提交这一段")
            .expect("dispatch");

        clock.latest_update = Some(owner_update(3_600, 3_500));
        clock.tentative_owner_continuation_at = Some(started + Duration::from_millis(20));
        clock.tentative_owner_continuation_audio_ms = Some(3_500);
        clock.tentative_owner_vad_epoch = Some(9);
        clock.settle_text_submission(Some(sequence), true, None);

        assert_eq!(clock.last_body_delivery_audio_ms, Some(3_000));
        assert_eq!(
            clock.tentative_owner_continuation_at,
            Some(started + Duration::from_millis(20))
        );
        assert_eq!(clock.tentative_owner_continuation_audio_ms, Some(3_500));
        assert_eq!(clock.tentative_owner_vad_epoch, Some(9));
    }

    #[test]
    fn an_old_confirmed_ticket_cannot_move_a_newer_body_clock() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        let first = clock
            .begin_text_submission(started, "第一段正文")
            .expect("first dispatch");
        let (old_handle, old_completion) = pending_receipt_handle_for_test(711);
        let old_ticket = old_handle.ticket();
        clock.settle_text_submission(Some(first), true, Some(old_handle));

        let second_at = started + Duration::from_millis(100);
        let second = clock
            .begin_text_submission(second_at, "第二段正文")
            .expect("second dispatch");
        clock.settle_text_submission(Some(second), true, None);
        assert!(old_completion
            .send(PasteReceiptCompletion {
                ticket: old_ticket,
                observed_at: started + Duration::from_secs(10),
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            })
            .is_err());
        clock.poll_body_receipts();

        assert_eq!(clock.last_body_submission_sequence, Some(second));
        assert_eq!(clock.last_body_delivery_at, Some(second_at));
        assert!(clock.pending_body_receipts.is_empty());
    }

    #[test]
    fn synchronous_body_commit_invalidates_an_older_paste_ticket() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        let first = clock
            .begin_text_submission(started, "剪贴板正文")
            .expect("paste dispatch");
        let (old_handle, old_completion) = pending_receipt_handle_for_test(712);
        let old_ticket = old_handle.ticket();
        clock.settle_text_submission(Some(first), true, Some(old_handle));

        let synchronous_at = started + Duration::from_millis(80);
        clock.note_text_delivery(synchronous_at, "后来的TSF正文");
        assert!(old_completion
            .send(PasteReceiptCompletion {
                ticket: old_ticket,
                observed_at: started + Duration::from_secs(10),
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            })
            .is_err());
        assert_eq!(clock.last_body_submission_sequence, None);
        assert_eq!(clock.last_body_delivery_at, Some(synchronous_at));
        assert!(clock.pending_body_receipts.is_empty());
    }

    #[test]
    fn approved_body_survives_age_and_punctuation_until_consumed_or_retracted() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.refresh_pending_owner_delivery(
            Some(("等待提交正文".to_string(), started)),
            started,
        );
        let first_key = clock
            .pending_owner_delivery
            .as_ref()
            .expect("hold")
            .key.clone();

        clock.refresh_pending_owner_delivery(
            Some(("等待提交正文".to_string(), started + Duration::from_secs(1))),
            started + Duration::from_secs(1),
        );
        clock.refresh_pending_owner_delivery(
            Some(("。".to_string(), started + Duration::from_secs(1))),
            started + Duration::from_secs(1),
        );
        assert_eq!(
            clock.pending_owner_delivery.as_ref().map(|hold| hold.key.as_str()),
            Some(first_key.as_str())
        );
        let after_expiry = started + Duration::from_secs(30);
        clock.refresh_pending_owner_delivery(
            Some(("等待提交正文".to_string(), after_expiry)), after_expiry,
        );
        assert!(clock.pending_owner_delivery_active(after_expiry));

        let next = started + Duration::from_secs(2);
        clock.refresh_pending_owner_delivery(Some(("新的正文".to_string(), next)), next);
        assert!(clock.pending_owner_delivery_active(next));
        clock.refresh_pending_owner_delivery(None, next + Duration::from_millis(1));
        assert!(clock.pending_owner_delivery.is_none());
    }

    #[test]
    fn unconfirmed_body_cannot_expire_into_permission_to_stop() {
        use crate::asr::volcengine::PendingBodyDelivery;
        use crate::personal_vad::{PresenceObservation, PresenceState};
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        for (offset, key) in [(0, "Still speaking"), (4_000, "Still speaking, with more words"), (7_000, "Still speaking, with more words.")] {
            let now = started + Duration::from_millis(offset);
            let audio_ms = 10_000 + offset;
            clock.latest_update = Some(owner_update(audio_ms, 4_000));
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: audio_ms, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None, source_history_complete: true,
                state: PresenceState::ConfirmedOther, observed_at: now,
            }), now);
            clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
                key: key.into(), updated_at: started - Duration::from_secs(2), recognition_pending: true,
            }), now);
            assert!(clock.pending_owner_delivery_active(now));
            assert!(!clock.personal_presence_stop_due(audio_ms, now, 3_000),
                "model mismatch, stale text age and an expired settling deadline cannot complete native recognition");
            assert!(!clock.is_due(now, 3_000));
        }
        let resolved = started + Duration::from_secs(8);
        clock.refresh_pending_body_delivery(None, resolved);
        let sequence = clock.begin_text_submission(resolved, "The completed body.").unwrap();
        let (handle, completion) = pending_receipt_handle_for_test(746);
        let ticket = handle.ticket();
        clock.settle_text_submission(Some(sequence), true, Some(handle));
        let received = resolved + Duration::from_millis(500);
        completion.send(PasteReceiptCompletion { ticket, observed_at: received,
            outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
        }).unwrap();
        clock.poll_body_receipts();
        for offset in [2_999, 3_000] {
            let now = received + Duration::from_millis(offset);
            let audio_ms = 18_500 + offset;
            clock.latest_update = Some(owner_update(audio_ms, 4_000));
            clock.note_personal_presence(Some(PresenceObservation {
                analyzed_end_ms: audio_ms, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None, source_history_complete: true,
                state: PresenceState::ConfirmedOther, observed_at: now,
            }), now);
            assert_eq!(clock.personal_presence_stop_due(audio_ms, now, 3_000), offset == 3_000);
        }
    }

    #[test]
    fn recognition_resolution_and_retraction_release_only_their_own_obligation() {
        use crate::asr::volcengine::PendingBodyDelivery;
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
            key: "body".into(), updated_at: started - Duration::from_secs(3), recognition_pending: true,
        }), started);
        assert!(clock.pending_owner_delivery_active(started));
        clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
            key: "body".into(), updated_at: started, recognition_pending: false,
        }), started);
        assert!(clock.pending_owner_delivery_active(started), "native finality alone cannot replace actual editor submission");
        clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
            key: "corrected body".into(), updated_at: started, recognition_pending: true,
        }), started);
        clock.refresh_pending_body_delivery(None, started);
        assert!(!clock.pending_owner_delivery_active(started), "consumption or authority retraction retires the pending occurrence");
    }

    #[test]
    fn command_only_capture_cannot_inherit_a_pre_body_delivery_obligation() {
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.automatic_no_body_armed = true;
        clock.refresh_pending_body_delivery(Some(crate::asr::volcengine::PendingBodyDelivery {
            key: "background before the accepted command".into(), updated_at: started, recognition_pending: true,
        }), started);
        assert!(!clock.pending_owner_delivery_active(started));
        clock.automatic_no_body_armed = false;
        clock.refresh_pending_body_delivery(Some(crate::asr::volcengine::PendingBodyDelivery {
            key: "actual body after the accepted command".into(), updated_at: started, recognition_pending: true,
        }), started);
        assert!(clock.pending_owner_delivery_active(started));
    }

    #[test]
    fn personal_receipt_starts_three_seconds_once_and_background_receipts_do_not_rearm_it() {
        use crate::personal_vad::PresenceObservation;
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.latest_update = Some(owner_update(4_000, 4_000));
        clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
            analyzed_end_ms: 4_000, last_positive_end_ms: Some(4_000),
            last_unresolved_end_ms: None,
            state: crate::personal_vad::PresenceState::Owner, observed_at: started,
        }), started);
        let sequence = clock.begin_text_submission(started, "The owner's actual body.").unwrap();
        let (handle, completion) = pending_receipt_handle_for_test(741);
        let ticket = handle.ticket();
        clock.settle_text_submission(Some(sequence), true, Some(handle));
        let polled_at = started + Duration::from_millis(3_200);
        clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
            analyzed_end_ms: 7_200, last_positive_end_ms: Some(4_000),
            last_unresolved_end_ms: None,
            state: crate::personal_vad::PresenceState::ConfirmedOther, observed_at: polled_at,
        }), polled_at);
        assert!(!clock.personal_presence_stop_due(7_200, polled_at, 3_000),
            "the physical STOP must wait for the actual editor receipt");
        let received_at = started + Duration::from_millis(2_500);
        completion.send(PasteReceiptCompletion {
            ticket, observed_at: received_at,
            outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
        }).unwrap();
        clock.poll_body_receipts();
        assert!(!clock.personal_presence_stop_due(7_200, polled_at, 3_000),
            "dispatch time cannot start the owner's three-second receipt window");
        for offset in [4_000, 5_000] {
            let now = started + Duration::from_millis(offset);
            clock.latest_update = Some(owner_update(4_000 + offset, 4_000));
            clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
                analyzed_end_ms: 4_000 + offset, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None,
                state: crate::personal_vad::PresenceState::ConfirmedOther, observed_at: now,
            }), now);
            let sequence = clock.begin_text_submission(now, "The phone's continuing words.").unwrap();
            let (handle, completion) = pending_receipt_handle_for_test(741 + offset);
            let ticket = handle.ticket();
            clock.settle_text_submission(Some(sequence), true, Some(handle));
            completion.send(PasteReceiptCompletion {
                ticket, observed_at: now,
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            }).unwrap();
            clock.poll_body_receipts();
            assert!(!clock.personal_presence_stop_due(4_000 + offset, now, 3_000));
        }
        for offset in [5_499, 5_500] {
            let now = started + Duration::from_millis(offset);
            clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
                analyzed_end_ms: 4_000 + offset, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None,
                state: crate::personal_vad::PresenceState::ConfirmedOther, observed_at: now,
            }), now);
            assert_eq!(clock.personal_presence_stop_due(4_000 + offset, now, 3_000), offset == 5_500,
                "only the owner's confirmed receipt owns the full continuation window");
        }
    }

    #[test]
    fn personal_receipt_preserves_uncertain_tail_without_background_rearming() {
        use crate::personal_vad::{PresenceObservation, PresenceState};
        let started = Instant::now();
        let mut clock = automatic_body_clock();
        clock.latest_update = Some(owner_update(4_000, 4_000));
        clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
            analyzed_end_ms: 4_000, last_positive_end_ms: Some(4_000),
            last_unresolved_end_ms: None, state: PresenceState::Owner, observed_at: started,
        }), started);
        clock.note_body_delivery(started);
        let dispatched = started + Duration::from_millis(6_000);
        clock.latest_update = Some(owner_update(10_000, 4_000));
        clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
            analyzed_end_ms: 10_000, last_positive_end_ms: Some(4_000),
            last_unresolved_end_ms: Some(10_000), state: PresenceState::Unknown,
            observed_at: dispatched,
        }), dispatched);
        let sequence = clock.begin_text_submission(dispatched, "The remaining uncertain sentence.").unwrap();
        let (handle, completion) = pending_receipt_handle_for_test(761);
        let ticket = handle.ticket();
        clock.settle_text_submission(Some(sequence), true, Some(handle));
        assert_eq!(clock.last_pending_personal_continuation_end_ms, Some(10_000));
        let received = dispatched + Duration::from_millis(400);
        completion.send(PasteReceiptCompletion {
            ticket, observed_at: received,
            outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
        }).unwrap();
        clock.poll_body_receipts();
        let before = received + Duration::from_millis(2_900);
        clock.latest_update = Some(owner_update(13_200, 4_000));
        clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
            analyzed_end_ms: 13_200, last_positive_end_ms: Some(4_000),
            last_unresolved_end_ms: Some(10_000), state: PresenceState::ConfirmedOther,
            observed_at: before,
        }), before);
        // This later, already-confirmed background body has no new possible-
        // owner source range and therefore cannot replace the matching receipt.
        clock.note_body_delivery(before);
        assert!(!clock.personal_presence_stop_due(13_300, before, 3_000));
        assert!(clock.personal_presence_stop_due(13_400, received + Duration::from_millis(3_000), 3_000));
    }

    #[test]
    fn personal_dispatch_does_not_spend_owner_activity_captured_after_it() {
        use crate::personal_vad::PresenceObservation;
        for state in [crate::personal_vad::PresenceState::Owner, crate::personal_vad::PresenceState::Unknown] {
            let started = Instant::now();
            let mut clock = automatic_body_clock();
            clock.latest_update = Some(owner_update(4_000, 4_000));
            clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
                analyzed_end_ms: 4_000, last_positive_end_ms: Some(4_000),
                last_unresolved_end_ms: None,
                state: crate::personal_vad::PresenceState::Owner, observed_at: started,
            }), started);
            let sequence = clock.begin_text_submission(started, "Earlier words.").unwrap();
            let returned_at = started + Duration::from_millis(400);
            clock.note_personal_presence(Some(PresenceObservation {
        source_history_complete: false,
                analyzed_end_ms: 4_400,
                last_positive_end_ms: Some(if state == crate::personal_vad::PresenceState::Owner { 4_400 } else { 4_000 }),
                last_unresolved_end_ms: (state == crate::personal_vad::PresenceState::Unknown).then_some(4_400),
                state, observed_at: returned_at,
            }), returned_at);
            clock.latest_update = Some(owner_update(4_400, 4_400));
            let (handle, completion) = pending_receipt_handle_for_test(751);
            let ticket = handle.ticket();
            clock.settle_text_submission(Some(sequence), true, Some(handle));
            assert_eq!(clock.last_pending_personal_continuation_end_ms, Some(4_000));
            completion.send(PasteReceiptCompletion {
                ticket, observed_at: returned_at,
                outcome: PasteReceiptOutcome::Confirmed(PasteReceiptProof::ExactEdit),
            }).unwrap();
            clock.poll_body_receipts();
            clock.refresh_pending_owner_delivery(Some(("Actual owner continuation.".into(), returned_at)), returned_at);
            assert!(clock.pending_owner_delivery_active(returned_at),
                "an older receipt cannot consume the new owner's delivery opportunity");
            assert_eq!(clock.last_pending_personal_continuation_end_ms, Some(4_400));
            clock.note_personal_presence(None, returned_at);
            assert!(!clock.personal_presence_stop_due(8_000, returned_at + Duration::from_secs(4), 3_000),
                "missing personal analysis never becomes completed owner absence");
        }
    }

    #[test]
    fn punctuation_does_not_create_a_submission_sequence() {
        let mut clock = automatic_body_clock();
        assert_eq!(clock.begin_text_submission(Instant::now(), "。"), None);
        assert!(clock.pending_body_dispatch.is_none());
    }

    fn personal_frontier_clock(now: Instant) -> SettledTargetEndpointClock {
        let mut clock = automatic_body_clock();
        clock.note_personal_presence(Some(crate::personal_vad::PresenceObservation {
            analyzed_end_ms: 108_000,
            last_positive_end_ms: Some(72_000),
            last_unresolved_end_ms: Some(103_200),
            source_history_complete: true,
            state: crate::personal_vad::PresenceState::ConfirmedOther,
            observed_at: now,
        }), now);
        clock
    }

    fn frontier_vad(epoch: u64, revision: u64, end: u64,
        onset: Option<u64>, state: crate::asr::volcengine::LocalSpeechActivityState,
    ) -> crate::asr::volcengine::LocalSpeechEvidence {
        crate::asr::volcengine::LocalSpeechEvidence {
            analyzed_through_ms: end,
            analyzed_through_samples: end * 16,
            activity_epoch: epoch, revision, pending_speech_start_ms: onset, state,
            ..Default::default()
        }
    }

    #[test]
    fn personal_source_frontier_revokes_stop_on_unanalyzed_new_onset() {
        use crate::asr::volcengine::LocalSpeechActivityState::PendingSpeech;
        let now = Instant::now();
        for onset in [108_000, 108_080] {
            let mut clock = personal_frontier_clock(now);
            assert!(clock.personal_presence_stop_due(108_160, now, 3_000));
            // This is the final admission's actual refresh, after a STOP was
            // proposed. A pending onset does not advance canonical Speech.
            clock.stop_proposed = true;
            let generation = clock.generation;
            clock.note_local_vad_evidence_at(frontier_vad(2, 4, 108_160,
                Some(onset), PendingSpeech), now);
            assert_eq!(clock.generation, generation);
            assert!(!clock.personal_presence_stop_due(108_160, now, 3_000),
                "old negative frontier cannot cover onset {onset}");
        }
    }

    #[test]
    fn personal_source_frontier_missed_short_epoch_waits_for_source_coverage() {
        use crate::asr::volcengine::LocalSpeechActivityState::NonSpeech;
        let now = Instant::now();
        let mut clock = personal_frontier_clock(now);
        clock.note_local_vad_evidence_at(frontier_vad(1, 1, 107_920, None, NonSpeech), now);
        clock.note_local_vad_evidence_at(frontier_vad(2, 3, 108_160, None, NonSpeech), now);
        assert!(!clock.personal_presence_stop_due(108_160, now, 3_000));
        clock.note_personal_presence(Some(crate::personal_vad::PresenceObservation {
            analyzed_end_ms: 108_160, last_positive_end_ms: Some(72_000),
            last_unresolved_end_ms: Some(103_200), source_history_complete: true,
            state: crate::personal_vad::PresenceState::ConfirmedOther, observed_at: now,
        }), now);
        assert!(clock.personal_presence_stop_due(108_160, now, 3_000),
            "completed-through fence includes equality after actual analysis");
    }

    #[test]
    fn personal_source_frontier_same_epoch_background_does_not_renew_fence() {
        use crate::asr::volcengine::LocalSpeechActivityState::Speech;
        let now = Instant::now();
        let mut clock = personal_frontier_clock(now);
        clock.note_local_vad_evidence_at(frontier_vad(2, 1, 107_920, Some(103_200), Speech), now);
        clock.note_local_vad_evidence_at(frontier_vad(2, 2, 108_160, None, Speech), now);
        assert!(clock.personal_presence_stop_due(108_160, now, 3_000),
            "ongoing classified background does not buy an endless VAD hold");
    }

    #[test]
    fn personal_source_frontier_rejected_callbacks_cannot_erase_new_epoch() {
        use crate::asr::volcengine::LocalSpeechActivityState::{PendingSpeech, NonSpeech};
        let now = Instant::now();
        for stale in [frontier_vad(1, 3, 107_920, None, NonSpeech),
            frontier_vad(1, 5, 108_160, None, NonSpeech)] {
            let mut clock = personal_frontier_clock(now);
            clock.note_local_vad_evidence_at(frontier_vad(2, 4, 108_160,
                Some(108_080), PendingSpeech), now);
            clock.note_local_vad_evidence_at(stale, now);
            assert!(!clock.personal_presence_stop_due(108_160, now, 3_000));
        }
    }

    #[test]
    fn personal_source_frontier_fresh_independent_quiet_can_still_release_stall() {
        use crate::asr::volcengine::LocalSpeechActivityState::{PendingSpeech, NonSpeech};
        let now = Instant::now();
        let mut clock = personal_frontier_clock(now);
        clock.note_local_vad_evidence_at(frontier_vad(2, 4, 108_160,
            Some(108_080), PendingSpeech), now);
        let quiet_at = now + Duration::from_millis(160);
        clock.note_local_vad_evidence_at(frontier_vad(2, 5, 108_320, None, NonSpeech), quiet_at);
        assert!(!clock.personal_presence_stop_due(108_320, quiet_at, 3_000));
        let later = quiet_at + Duration::from_secs(3);
        clock.note_local_vad_evidence_at(frontier_vad(2, 6, 111_320, None, NonSpeech), later);
        assert!(clock.personal_presence_stop_due(111_320, later, 3_000),
            "independent completed new quiet must not wait forever for personal inference");
    }

    #[test]
    fn personal_source_frontier_onset_is_exclusive_but_observation_is_inclusive() {
        assert!(LocalVadEpochSourceFence::Onset(108_000).is_unobserved(108_000));
        assert!(!LocalVadEpochSourceFence::Onset(108_000).is_unobserved(108_001));
        assert!(LocalVadEpochSourceFence::ObservedThrough(108_160).is_unobserved(108_159));
        assert!(!LocalVadEpochSourceFence::ObservedThrough(108_160).is_unobserved(108_160));
    }
}
