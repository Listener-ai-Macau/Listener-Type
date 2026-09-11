/// Wall-clock companion to the provider audio clock.
///
/// The provider can publish a complete, speaker-attributed preview and then
/// stop advancing its covered-audio timestamp while local low-level noise
/// continues to trip the energy detector. In that state the ordinary endpoint
/// waits for the firmware fallback even though the user-visible text has been
/// settled for several seconds. Arm this clock only for a visible body with a
/// stable cloud target. Provisional speech cancels it, and a newer target
/// boundary rearms it, so it cannot race an actively growing owner utterance.
#[derive(Debug, Default)]
struct SettledTargetEndpointClock {
    generation: u64,
    armed_target_end_ms: Option<u64>,
    armed_at: Option<Instant>,
    armed_from_visible_body_fallback: bool,
    /// The accepted wake has not produced any body text.  This is not a
    /// second endpoint: it is an explicit mode of the same controller, armed
    /// from the automatic-wake guard's original clock.  Wake-phrase audio and
    /// provider bookkeeping cannot rearm or block this bounded abandonment.
    automatic_no_body_armed: bool,
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
    pending_was_seen: bool,
    /// The sole visible-recording stop authority. All callback evidence is
    /// reduced into this controller; no provider or firmware callback owns a
    /// second endpoint timer.
    product_endpoint: crate::speech_decision_kernel::OwnerEndpointController,
    last_visible_body_signature: Option<(bool, usize)>,
    /// Last local speech edge used to renew the firmware endpoint.  Provider
    /// callbacks can repeat the same snapshot; dedupe it so a stale snapshot
    /// cannot keep recording alive indefinitely.
    last_firmware_lease_owner_speech_ms: Option<u64>,
    /// A bounded diagnostic latch: when the endpoint deadline is reached but
    /// evidence keeps it in Hold/CatchingUp, report the first reason for this
    /// generation only.  This avoids per-50ms log spam while making a stuck
    /// endpoint distinguishable from an actively growing owner utterance.
    pending_due_hold_diagnostic: Option<(u64, &'static str)>,
    last_reported_due_hold_diagnostic: Option<(u64, &'static str)>,
}

const RECENT_STRONG_NON_TARGET_WINDOW_MS: u64 = 900;
const MANUAL_TERMINAL_BRIDGE_MAX_MS: u64 = 3_000;
const MANUAL_TERMINAL_BRIDGE_MIN_VISIBLE_CHARS: usize = 20;
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
    if latest_visible_body_ends_terminal == Some(false) {
        return "open_clause_tail";
    }
    if !(update.speaker_info_present && update.speaker_id.is_some())
        && update.local_target_speech_end_ms.is_none()
    {
        return "owner_identity_not_settled";
    }
    "endpoint_guard"
}

fn product_endpoint_evidence(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    authoritative_owner_watermark_ms: Option<u64>,
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
        pending_provider_text: update.pending_unattributed_speech,
        latest_speech_confirmed_non_target,
        unresolved_owner_tail: has_unresolved_recent_owner_speech(
            update,
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        ),
    }
}

fn update_has_recent_strong_non_target(
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
) -> bool {
    let Some(non_target_end_ms) = update.local_non_target_speech_end_ms else {
        return false;
    };
    if update
        .local_target_speech_end_ms
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

impl SettledTargetEndpointClock {
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
            // Keep diagnostics current without allowing wake audio, room
            // energy or provider callbacks to restart the original deadline.
            self.latest_update = Some(update.clone());
            self.latest_update_at = Some(observed_at);
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = Some(started_at);
        self.armed_from_visible_body_fallback = true;
        self.automatic_no_body_armed = true;
        self.latest_update = Some(update.clone());
        self.latest_update_at = Some(started_at);
        self.clear_paused_arm();
        self.pending_was_seen = false;
        self.product_endpoint
            .arm(crate::speech_decision_kernel::EndpointEvidence::default());
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
    }

    fn should_renew_firmware_endpoint_lease(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
    ) -> bool {
        let owner_speech_ms = update.local_target_speech_end_ms;
        if owner_speech_ms.is_some()
            && owner_speech_ms == self.last_firmware_lease_owner_speech_ms
        {
            return false;
        }
        let latest_speech_confirmed_non_target = update
            .local_speech_end_ms
            .is_some_and(|speech_ms| local_speech_confidently_non_target(update, speech_ms));
        let owner_established = update.target_speech_end_ms.is_some()
            || update.local_target_speech_end_ms.is_some();
        let evidence = crate::speech_decision_kernel::FirmwareEndpointLeaseEvidence {
            visible_body: body_started,
            owner_established,
            owner_speech_watermark_ms: owner_speech_ms,
            provider_coverage_ms: update.provider_audio_duration_ms,
            unresolved_owner_tail: has_unresolved_recent_owner_speech(
                update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            ),
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

    /// Returns a generation token when a new one-second timer must be started.
    fn observe(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
    ) -> Option<u64> {
        if self.automatic_no_body_armed {
            if !body_started {
                self.latest_update = Some(update.clone());
                self.latest_update_at = Some(now);
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
            .and_then(|previous| previous.local_target_speech_end_ms);
        self.latest_update = Some(update.clone());
        self.latest_update_at = Some(now);
        let recent_strong_non_target = update_has_recent_strong_non_target(update);
        let pending_owner_tail = update.pending_unattributed_speech
            && !recent_strong_non_target
            && has_unresolved_recent_owner_speech(
                &update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            );
        if pending_owner_tail {
            self.pending_was_seen = true;
            if self.armed_at.is_some() {
                self.pause_arm_for_provisional_tail();
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
        let stable_owner_end_ms =
            authoritative_owner_endpoint_boundary(update, stable_cloud_target_end_ms);
        // A cloud row may briefly outrun the enrolled local owner clock while
        // room speech is being merged into the same provider speaker id. Once
        // the local speech edge exceeds the bounded uncertainty budget,
        // authoritative_owner_endpoint_boundary deliberately falls back to the
        // local owner watermark. Treat that downward transition as a real
        // authority change and re-arm the reducer; otherwise the product
        // arbiter keeps comparing the new local watermark with the old cloud
        // watermark and emits arbiter_hold forever.
        let local_authority_recovered_from_cloud = update
            .local_target_speech_end_ms
            .zip(update.local_speech_end_ms)
            .zip(stable_cloud_target_end_ms)
            .is_some_and(|((local_owner_ms, local_speech_ms), cloud_ms)| {
                cloud_ms > local_owner_ms
                    && local_speech_ms
                        > local_owner_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                    && stable_owner_end_ms == Some(local_owner_ms)
            });
        let local_owner_activity_advanced = update
            .local_target_speech_end_ms
            .zip(previous_local_owner_end_ms)
            .is_some_and(|(new_end_ms, previous_end_ms)| new_end_ms > previous_end_ms);
        let fresh_owner_activity = update.target_activity_advanced
            || update.pending_activity_advanced
            || local_owner_activity_advanced;
        let stable_target_should_rearm = stable_owner_end_ms.is_some()
            && (self.armed_at.is_none()
                || self.pending_was_seen
                || (local_authority_recovered_from_cloud && fresh_owner_activity)
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
        let should_rearm = body_started
            && (stable_target_should_rearm || unattributed_visible_body_should_arm)
            && (!recent_strong_non_target || self.armed_at.is_none());
        self.pending_was_seen = false;
        if !should_rearm {
            return None;
        }

        let restore_paused_owner_deadline = recent_strong_non_target
            && self.armed_at.is_none()
            && self.paused_armed_at.is_some();
        self.generation = self.generation.wrapping_add(1);
        if restore_paused_owner_deadline {
            self.armed_target_end_ms = self.paused_armed_target_end_ms;
            self.armed_at = self.paused_armed_at;
            // The paused boundary already came from visible owner text, and
            // the current frame now has sustained local other-speaker proof.
            // Do not require that provisional frame to repeat the cloud id.
            self.armed_from_visible_body_fallback = true;
        } else {
            self.armed_target_end_ms = stable_owner_end_ms;
            self.armed_at = Some(now);
            // Local Target can rearm the clock before cloud diarization catches
            // up, but local-only visible text still uses the existing guarded
            // visible-body fallback authority.
            self.armed_from_visible_body_fallback = stable_cloud_target_end_ms.is_none();
        }
        self.clear_paused_arm();
        self.product_endpoint.arm(product_endpoint_evidence(
            update,
            self.armed_target_end_ms,
        ));
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

    fn arm_latest_for_visible_body(&mut self, now: Instant) -> Option<u64> {
        self.leave_automatic_no_body_mode();
        let update = self.latest_update.clone()?;
        let recent_strong_non_target = update_has_recent_strong_non_target(&update);
        let pending_owner_tail = update.pending_unattributed_speech
            && !recent_strong_non_target
            && has_unresolved_recent_owner_speech(
                &update,
                EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
            );
        if pending_owner_tail {
            self.pending_was_seen = true;
            if self.armed_at.is_some() {
                self.pause_arm_for_provisional_tail();
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
        let stable_owner_end_ms =
            authoritative_owner_endpoint_boundary(&update, stable_cloud_target_end_ms);
        let local_authority_recovered_from_cloud = update
            .local_target_speech_end_ms
            .zip(update.local_speech_end_ms)
            .zip(stable_cloud_target_end_ms)
            .is_some_and(|((local_owner_ms, local_speech_ms), cloud_ms)| {
                cloud_ms > local_owner_ms
                    && local_speech_ms
                        > local_owner_ms.saturating_add(EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS)
                    && stable_owner_end_ms == Some(local_owner_ms)
            });
        let restore_paused_owner_deadline =
            recent_strong_non_target && self.paused_armed_at.is_some();
        let owner_boundary_advanced = stable_owner_end_ms.is_some_and(|new_end_ms| {
            self.armed_target_end_ms
                .is_none_or(|armed_end_ms| new_end_ms > armed_end_ms)
        });
        let fresh_owner_activity = update.target_activity_advanced || update.pending_activity_advanced;
        // This method is called from every visible preview callback.  A
        // callback is not itself fresh owner speech: restarting the wall clock
        // here makes a long but already-settled preview wait forever.  Re-arm
        // only for a strictly newer fused owner boundary or restoration of a
        // paused deadline after explicit other-speaker evidence.
        if self.armed_at.is_some()
            && !owner_boundary_advanced
            && !(local_authority_recovered_from_cloud && fresh_owner_activity)
            && !restore_paused_owner_deadline
            && !self.manual_terminal_bridge_rearm_pending
        {
            return None;
        }
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
        self.product_endpoint.arm(product_endpoint_evidence(
            &update,
            self.armed_target_end_ms,
        ));
        Some(self.generation)
    }

    fn is_due(&mut self, now: Instant, timeout_ms: u64) -> bool {
        let Some(armed_at) = self.armed_at else {
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
            // Three seconds without accepted body means this is a wake-only
            // session. The wake phrase's own owner tail and provider pending
            // state are not body evidence and cannot keep the capsule open.
            let decision = self.product_endpoint.decide_stop(
                crate::speech_decision_kernel::EndpointEvidence::default(),
                now,
                Duration::from_millis(ENDPOINT_PROVIDER_CATCH_UP_GRACE_MS),
            );
            return matches!(decision, crate::speech_decision_kernel::EndpointDecision::Stop);
        }
        if !Self::update_allows_endpoint(
            &update,
            self.armed_from_visible_body_fallback,
            self.latest_visible_body_ends_terminal,
            self.manual_terminal_bridge_until,
            armed_at,
            now,
        ) {
            self.note_due_hold_diagnostic(
                self.generation,
                endpoint_hold_reason(&update, self.latest_visible_body_ends_terminal),
            );
            return false;
        }
        let decision = self.product_endpoint.decide_stop(
            product_endpoint_evidence(&update, self.armed_target_end_ms),
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
        self.is_due(now, timeout_ms)
            .then(|| self.latest_update.clone())
            .flatten()
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
            now.saturating_duration_since(observed_at) >= Duration::from_millis(timeout_ms)
        });
        if stale {
            update.pending_unattributed_speech = false;
            update.pending_activity_advanced = false;
            // No callback means there is no newer owner edge to protect. Use
            // the last covered audio edge as the bounded stall watermark so a
            // frozen local tail cannot block the reducer forever.
            if let Some(audio_ms) = update.audio_duration_ms.or(update.provider_audio_duration_ms)
            {
                update.local_speech_end_ms = Some(audio_ms);
            }
        }
        Some(update)
    }

    fn reopen_after_failed_stop(&mut self) {
        if let Some(update) = self.latest_update.as_ref() {
            let evidence = if self.automatic_no_body_armed {
                crate::speech_decision_kernel::EndpointEvidence::default()
            } else {
                product_endpoint_evidence(update, self.armed_target_end_ms)
            };
            self.product_endpoint.reopen_after_failed_stop(evidence);
        }
    }
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
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
) {
    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let endpoint_clock = Arc::clone(endpoint_clock);
    let asr = Arc::clone(asr);
    async_runtime::spawn(async move {
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if stop_dispatched.load(Ordering::SeqCst) {
                return;
            }
            let session_active = {
                let state = inner.state.lock();
                state.session_id == session_id
                    && !state.cancelled
                    && matches!(
                        state.phase,
                        SessionPhase::Starting | SessionPhase::Listening
                    )
            };
            if !session_active {
                return;
            }
            let preview = current_embedded_audio_endpoint_preview(&inner);
            let decision_snapshot = asr.endpoint_update_snapshot();
            let decision_audio_ms = decision_snapshot
                .audio_duration_ms
                .or(decision_snapshot.provider_audio_duration_ms);
            let endpoint_policy = resolve_target_speaker_endpoint_policy(
                &inner,
                session_id,
                preview.as_deref(),
                decision_audio_ms,
            );
            let (update, hold_diagnostic) = {
                let mut clock = endpoint_clock.lock();
                // If the provider never opened, keep feeding the reducer from
                // the local owner clock. This preserves the same single
                // watchdog decision path while removing generic room-energy
                // from the only remaining fallback.
                if asr.audio_delivery_failed() {
                    clock.observe(
                        &decision_snapshot,
                        endpoint_policy.body_started,
                        Instant::now(),
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
                let update =
                    clock.reduce_session_policy(
                        now,
                        endpoint_policy,
                        &decision_snapshot,
                        owner_analysis_pending,
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
            if let Some(update) = update {
                handle_target_speaker_endpoint_stop(
                    &inner,
                    session_id,
                    &stop_dispatched,
                    &endpoint_clock,
                    update,
                    endpoint_policy,
                );
            }
        }
    });
}

fn arm_settled_target_endpoint_for_visible_body(
    inner: &Arc<Inner>,
    _session_id: SessionId,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    let preview = current_embedded_audio_partial_preview(inner);
    let preview_ends_terminal = preview_ends_with_sentence_terminal(preview.as_deref());
    let preview_chars = preview.as_deref().map_or(0, |text| text.chars().count());
    let now = Instant::now();
    {
        let mut clock = endpoint_clock.lock();
        clock.note_visible_body_boundary(preview_ends_terminal, preview_chars, now);
        clock.arm_latest_for_visible_body(now);
    }
}
