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
    latest_update: Option<crate::asr::volcengine::TargetSpeakerUpdate>,
    pending_was_seen: bool,
}

impl SettledTargetEndpointClock {
    fn cancel_arm(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = None;
        self.armed_at = None;
    }

    /// Returns a generation token when a new one-second timer must be started.
    fn observe(
        &mut self,
        update: &crate::asr::volcengine::TargetSpeakerUpdate,
        body_started: bool,
        now: Instant,
    ) -> Option<u64> {
        self.latest_update = Some(update.clone());
        if update.pending_unattributed_speech {
            self.pending_was_seen = true;
            if self.armed_at.is_some() {
                self.cancel_arm();
            }
            return None;
        }

        let stable_target_end_ms = (update.speaker_info_present && update.speaker_id.is_some())
            .then_some(update.target_speech_end_ms)
            .flatten();
        let should_rearm = body_started
            && stable_target_end_ms.is_some()
            && (update.target_activity_advanced
                || self.pending_was_seen
                || self.armed_at.is_none()
                || self.armed_target_end_ms != stable_target_end_ms);
        self.pending_was_seen = false;
        if !should_rearm {
            return None;
        }

        self.generation = self.generation.wrapping_add(1);
        self.armed_target_end_ms = stable_target_end_ms;
        self.armed_at = Some(now);
        Some(self.generation)
    }

    fn due_update(
        &self,
        generation: u64,
        now: Instant,
        timeout_ms: u64,
    ) -> Option<crate::asr::volcengine::TargetSpeakerUpdate> {
        if self.generation != generation {
            return None;
        }
        let armed_at = self.armed_at?;
        if now.saturating_duration_since(armed_at) < Duration::from_millis(timeout_ms) {
            return None;
        }
        self.latest_update
            .as_ref()
            .filter(|update| {
                !update.pending_unattributed_speech
                    && update.speaker_info_present
                    && update.speaker_id.is_some()
                    && update.target_speech_end_ms.is_some()
            })
            .cloned()
    }

    fn arm_latest_for_visible_body(&mut self, now: Instant) -> Option<u64> {
        let update = self.latest_update.clone()?;
        self.observe(&update, true, now)
    }

    fn is_due(&self, now: Instant, timeout_ms: u64) -> bool {
        self.armed_at.is_some_and(|armed_at| {
            now.saturating_duration_since(armed_at) >= Duration::from_millis(timeout_ms)
        }) && self.latest_update.as_ref().is_some_and(|update| {
            !update.pending_unattributed_speech
                && update.speaker_info_present
                && update.speaker_id.is_some()
                && update.target_speech_end_ms.is_some()
        })
    }
}

fn target_speaker_end_timeout_ms_for_preview(_preview: Option<&str>) -> u64 {
    EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
}

fn preview_has_dangling_continuation(preview: Option<&str>) -> bool {
    let Some(text) = preview.map(str::trim).filter(|text| !text.is_empty()) else {
        return false;
    };
    let lexical_tail = text
        .trim_end_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '，' | ','
                        | '、'
                        | '。'
                        | '！'
                        | '？'
                        | '.'
                        | '!'
                        | '?'
                        | '…'
                        | ':'
                        | '：'
                        | ';'
                        | '；'
                )
        })
        .to_ascii_lowercase();
    const DANGLING_SUFFIXES: &[&str] = &[
        "然后",
        "但是",
        "不过",
        "而且",
        "并且",
        "另外",
        "还有",
        "接着",
        "最后",
        "所以",
        "因此",
        "因为",
        "如果",
        "假如",
        "虽然",
        "可是",
        "或者",
        "以及",
        "就是",
        "也就是",
        "比如",
        "例如",
        "首先",
        "其次",
        "至于",
        "那么",
        "那这样的话",
        "换句话说",
        "and",
        "but",
        "because",
        "so",
        "then",
        "finally",
        "also",
    ];
    DANGLING_SUFFIXES.iter().any(|suffix| {
        if suffix.is_ascii() {
            lexical_tail.split_whitespace().last() == Some(*suffix)
        } else {
            lexical_tail.ends_with(suffix)
        }
    })
}

fn schedule_settled_target_endpoint_timer(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    generation: u64,
) {
    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let endpoint_clock = Arc::clone(endpoint_clock);
    async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        ))
        .await;
        let update = endpoint_clock.lock().due_update(
            generation,
            Instant::now(),
            EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        );
        let Some(update) = update else {
            return;
        };
        handle_target_speaker_update(&inner, session_id, &stop_dispatched, update, true);
    });
}

fn arm_settled_target_endpoint_for_visible_body(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
) {
    let generation = endpoint_clock
        .lock()
        .arm_latest_for_visible_body(Instant::now());
    if let Some(generation) = generation {
        schedule_settled_target_endpoint_timer(
            inner,
            session_id,
            stop_dispatched,
            endpoint_clock,
            generation,
        );
    }
}
