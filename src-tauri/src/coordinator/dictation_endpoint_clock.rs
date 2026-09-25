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
    last_body_delivery_audio_ms: Option<u64>,
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
}

const RECENT_STRONG_NON_TARGET_WINDOW_MS: u64 = 900;
// A new local VAD onset can arrive just before the owner inactivity deadline.
// Automatic sessions use it only to veto STOP while the audio is live; it is
// never an owner watermark or a fresh three-second lease.
const AUTOMATIC_STOP_VAD_MAX_LAG_MS: u64 = 250;
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
            <= AUTOMATIC_STOP_VAD_MAX_LAG_MS
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
    fn note_body_delivery(&mut self, now: Instant) {
        if !self.automatic_wake_session || self.automatic_no_body_armed {
            return;
        }
        self.last_body_delivery_at = Some(now);
        self.last_body_delivery_audio_ms = self
            .latest_update
            .as_ref()
            .and_then(|update| update.audio_duration_ms);
        self.manual_terminal_bridge_until = None;
        self.manual_terminal_bridge_rearm_pending = false;
        // A committed chunk is the start of the public continuation window.
        // A later provider revision of that same audio is not a new utterance.
        self.reset_continuation_tracking();
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

    fn note_local_vad_evidence(
        &mut self,
        evidence: crate::asr::volcengine::LocalSpeechEvidence,
    ) {
        let previous_state = self.latest_local_vad_evidence.map(|previous| previous.state);
        if evidence.state == crate::asr::volcengine::LocalSpeechActivityState::Speech
            && previous_state
                != Some(crate::asr::volcengine::LocalSpeechActivityState::Speech)
        {
            self.canonical_speech_serial = self.canonical_speech_serial.wrapping_add(1);
        }
        self.latest_local_vad_evidence = Some(evidence);
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
        if local_vad_revision.is_some() {
            self.manual_vad_guard = true;
        }
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
        if self.stop_proposed {
            return false;
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
        let owner_analysis_pending =
            owner_analysis_pending && policy.automatic_no_body_started_at.is_none();
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
    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let stop_completed = Arc::clone(stop_completed);
    let stop_state = Arc::clone(stop_state);
    let endpoint_clock = Arc::clone(endpoint_clock);
    let asr = Arc::clone(asr);
    let watchdog_started_at = Instant::now();
    let asr_instance = Arc::as_ptr(&asr) as usize;
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
            let (update, hold_diagnostic) = {
                let mut clock = endpoint_clock.lock();
                clock.note_local_vad_evidence(local_vad_evidence);
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
                let owner_analysis_pending = asr.local_speaker_analysis_pending();
                let update = clock.reduce_session_policy_with_local_vad_revision(
                    now,
                    endpoint_policy,
                    &decision_snapshot,
                    owner_analysis_pending,
                    local_vad_revision,
                );
                let hold_diagnostic = clock.take_due_hold_diagnostic();
                (update, hold_diagnostic)
            };
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
                    asr.local_speaker_analysis_pending(),
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
                    streaming_composition_update_tick(&inner, session_id, &asr).await;
                    pause_early_delivery_tick(&inner, session_id, &asr, &endpoint_clock).await;
                }
            }
        }
    });
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
