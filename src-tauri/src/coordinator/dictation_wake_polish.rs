// Wake candidate, speaker gate, streaming polish helpers.
// Included into `coordinator::dictation` via `include!`.

const LOCAL_SPEAKER_CLASSIFY_WINDOW_MS: usize = 1_200;
const LOCAL_SPEAKER_CLASSIFY_WINDOW_BYTES: usize = LOCAL_SPEAKER_CLASSIFY_WINDOW_MS * 32;
const LOCAL_SPEAKER_CLASSIFY_MIN_MS: usize = 1_000;
const LOCAL_SPEAKER_CLASSIFY_MIN_BYTES: usize = LOCAL_SPEAKER_CLASSIFY_MIN_MS * 32;
const LOCAL_SPEAKER_CLASSIFY_STEP_MS: u64 = 400;
// The isolated Paraformer helper is intentionally single-flight. A second
// candidate should get a bounded chance to use it after the active request
// finishes, but must never form an unbounded queue that delays terminal wake
// decisions or lets stale ambient candidates consume the helper forever.
const LOCAL_WAKE_HELPER_BUSY_RETRY_BUDGET_MS: u64 = 360;
const LOCAL_WAKE_HELPER_BUSY_RETRY_INTERVAL_MS: u64 = 40;
// 活窗 stage2 饿死计数器（2026-09-23 17:30 会话 569 实锤：词 1.52s 说完，
// 活窗 0.8/1.8/2.0s 三档全被上一隐藏窗 terminal-inflight 梯子占住单飞
// helper，第一拍拖到 3.1s PCM，胶囊慢 ~1.4s——环境只要有轻人声，每个 6s
// 隐藏窗都会烧完整终端梯子，轮转后死窗的收尾推理正好挡住活窗的词）。
// 活窗（streaming-* 分支）提交在 busy 重试期间 +1，拿到 helper 或放弃后
// 归零；terminal 梯子在窗口边界见 >0 时让位等它先过。进程级 static 与
// 单飞 helper 一一对应，不经 Inner 传递。
static WAKE_LIVE_STAGE2_STARVING: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
/// terminal 梯子单个窗口边界的让位上限：活窗一次确认 0.2-1s 加一拍 400ms
/// 重试的节奏，2s 足够放行；计数器异常不归零（任务泄漏）时也不把终端
/// 判定无限饿死——超时照常发下一窗。
const TERMINAL_CONFIRM_YIELD_TO_LIVE_MS: u64 = 2_000;

/// RAII：活窗提交从首次 busy 起计饿，作用域结束（拿到 helper / 放弃 /
/// 出错）自动归零，任何提前 return 都不会把计数器卡在高位。
struct LiveStage2StarvingGuard {
    _private: (),
}

impl LiveStage2StarvingGuard {
    fn arm() -> Self {
        WAKE_LIVE_STAGE2_STARVING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { _private: () }
    }
}

impl Drop for LiveStage2StarvingGuard {
    fn drop(&mut self) {
        WAKE_LIVE_STAGE2_STARVING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Diagnostic-only byte coordinate. It is deliberately independent from the
/// business counters used for PCM accounting. Once a range cannot be formed,
/// this cursor stays unknown for the rest of the logical stream.
#[derive(Clone, Copy, Debug)]
struct PcmDiagnosticCursor {
    next: Option<u64>,
}

impl Default for PcmDiagnosticCursor {
    fn default() -> Self {
        Self { next: Some(0) }
    }
}

impl PcmDiagnosticCursor {
    fn position(self) -> Option<u64> {
        self.next
    }

    fn append(&mut self, bytes: usize) -> Option<crate::observability::PcmRange> {
        if bytes == 0 {
            return None;
        }
        let Some(start) = self.next else {
            return None;
        };
        let Some(range) = crate::observability::PcmRange::from_start_and_bytes(start, bytes)
        else {
            self.next = None;
            return None;
        };
        self.next = Some(range.end);
        Some(range)
    }
}

fn record_pcm_stage_mapping_for_run(
    observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    logical_stream_id: u64,
    stage: crate::observability::PcmStage,
    disposition: crate::observability::PcmStageDisposition,
    mapping: crate::observability::PcmMappingKind,
    destination_range: Option<crate::observability::PcmRange>,
    segment_id: Option<u32>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
    bytes: usize,
) {
    record_pcm_stage_mapping_fact(
        observation,
        logical_stream_id,
        None,
        stage,
        disposition,
        mapping,
        destination_range,
        vec![crate::observability::AsrSourceShareFact {
            segment_id,
            bytes: bytes as u64,
            destination_range,
            source_interval,
            collector_metadata: None,
            collector_emitted_range: None,
        }],
    );
}

fn make_pcm_stage_mapping_fact(
    logical_stream_id: u64,
    operation_id: Option<u64>,
    stage: crate::observability::PcmStage,
    disposition: crate::observability::PcmStageDisposition,
    mapping: crate::observability::PcmMappingKind,
    destination_range: Option<crate::observability::PcmRange>,
    source_shares: Vec<crate::observability::AsrSourceShareFact>,
) -> crate::observability::PcmStageMappingFact {
    crate::observability::PcmStageMappingFact {
        logical_stream_id,
        operation_id,
        stage,
        stream_kind: source_shares
            .iter()
            .find_map(|share| share.source_interval.map(|interval| interval.stream_kind))
            .unwrap_or(crate::observability::PcmStreamKind::CoordinatorInputPcm),
        pcm_format: crate::observability::PcmFormat::PcmS16LeMono16k,
        mapping,
        disposition,
        destination_range,
        source_shares,
    }
}

fn record_pcm_stage_mapping_fact(
    observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    logical_stream_id: u64,
    operation_id: Option<u64>,
    stage: crate::observability::PcmStage,
    disposition: crate::observability::PcmStageDisposition,
    mapping: crate::observability::PcmMappingKind,
    destination_range: Option<crate::observability::PcmRange>,
    source_shares: Vec<crate::observability::AsrSourceShareFact>,
) {
    let Some(observation) = observation else {
        return;
    };
    observation.record_pcm_stage_mapping(make_pcm_stage_mapping_fact(
        logical_stream_id,
        operation_id,
        stage,
        disposition,
        mapping,
        destination_range,
        source_shares,
    ));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NormalizedStagePortion {
    output_bytes: usize,
    destination_range: Option<crate::observability::PcmRange>,
    mapping: crate::observability::PcmMappingKind,
}

/// Map one source run onto the bytes that actually came out of the normalizer.
/// A length mismatch keeps the real output range but never guesses a scale or
/// claims position preservation.
fn normalized_stage_portion(
    normalized_start: Option<u64>,
    input_bytes: usize,
    output_bytes: usize,
    output_offset: usize,
    source_run_bytes: usize,
) -> NormalizedStagePortion {
    let portion = source_run_bytes.min(output_bytes.saturating_sub(output_offset));
    NormalizedStagePortion {
        output_bytes: portion,
        destination_range: (portion > 0)
            .then(|| {
                normalized_start.and_then(|start| {
                    start
                        .checked_add(u64::try_from(output_offset).ok()?)
                        .and_then(|start| {
                            crate::observability::PcmRange::from_start_and_bytes(start, portion)
                        })
                })
            })
            .flatten(),
        mapping: if input_bytes > 0 && input_bytes == output_bytes {
            crate::observability::PcmMappingKind::PositionPreserving
        } else {
            crate::observability::PcmMappingKind::Unknown
        },
    }
}

struct LocalSessionSpeakerTracker {
    profile_rx: Option<
        std::sync::mpsc::Receiver<
            Result<crate::speaker_verification::SessionSpeakerProfile, String>,
        >,
    >,
    profile: Option<crate::speaker_verification::SessionSpeakerProfile>,
    classification_rx: Option<
        std::sync::mpsc::Receiver<
            Result<(u64, crate::speaker_verification::SessionSpeakerObservation), String>,
        >,
    >,
    adaptation_gate: crate::speaker_verification::SessionSpeakerAdaptationGate,
    rolling_pcm: Vec<u8>,
    next_classification_audio_ms: u64,
    wake_phrase: String,
    enrolled_owner_matched: bool,
}

impl LocalSessionSpeakerTracker {
    fn classification_pending(&self) -> bool {
        self.classification_rx.is_some()
    }

    fn from_wake(
        pcm: Vec<u8>,
        wake_end_seconds: f32,
        wake_phrase: String,
        enrolled_owner_matched: bool,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let wake_phrase_for_profile = wake_phrase.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = crate::speaker_verification::session_profile_from_wake(
                &pcm,
                wake_end_seconds,
                &wake_phrase_for_profile,
                enrolled_owner_matched,
            );
            let _ = tx.send(result);
        });
        Self {
            profile_rx: Some(rx),
            profile: None,
            classification_rx: None,
            adaptation_gate: Default::default(),
            rolling_pcm: Vec::with_capacity(LOCAL_SPEAKER_CLASSIFY_WINDOW_BYTES),
            next_classification_audio_ms: LOCAL_SPEAKER_CLASSIFY_MIN_MS as u64,
            wake_phrase,
            enrolled_owner_matched,
        }
    }

    /// The r10-r17 field rounds accepted the wake yet the ASR arbitration saw
    /// `local_speaker_tracking_enabled=false`. Re-assert the anchor notes on
    /// every body observation so a lost anchor cannot survive past the first
    /// classification window.
    fn reanchor_if_lost(
        &self,
        asr: &crate::asr::volcengine::VolcengineStreamingASR,
    ) {
        asr.reanchor_local_speaker_tracking_if_lost(
            &self.wake_phrase,
            self.enrolled_owner_matched,
        );
    }

    fn profile_is_adaptive(&self) -> Option<bool> {
        self.profile.as_ref().map(|profile| profile.is_adaptive())
    }

    /// 银行自适应 body admission: an enrolled-match session's window that the
    /// persisted bank itself claims at >= ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE
    /// (above the Target threshold, so a media-mixed 0.26-0.44 window can
    /// never qualify) with sufficient signal is a fresh owner exemplar.
    /// Fire-and-forget on a blocking task: the persisted write is
    /// rate-limited and similarity-gated inside `adapt_owner_session_bank`,
    /// and this session's profile is already a snapshot, so the refresh only
    /// benefits later sessions. Open-acceptance sessions (`enrolled_owner_
    /// matched == false`) never reach the write.
    fn maybe_adapt_owner_bank_from_confident_body(
        &self,
        observation: &crate::speaker_verification::SessionSpeakerObservation,
    ) {
        if !self.enrolled_owner_matched
            || self
                .profile
                .as_ref()
                .is_none_or(|profile| profile.is_adaptive())
        {
            return;
        }
        let confident_owner = observation.real_speech_ms
            >= crate::speaker_verification::SESSION_SPEAKER_MIN_NON_TARGET_MS
            && observation.signal_quality_sufficient
            && matches!(
                observation.classification,
                crate::speaker_verification::SessionSpeakerClassification::Target { score }
                    if score
                        >= crate::speaker_verification::ADAPTIVE_BANK_MIN_CONFIDENT_BODY_SCORE
            );
        if !confident_owner {
            return;
        }
        let phrase = self.wake_phrase.clone();
        let embedding = observation.embedding().to_vec();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = crate::speaker_verification::adapt_owner_session_bank(&phrase, &embedding);
        });
    }

    fn observe(
        &mut self,
        pcm: &[u8],
        audio_end_ms: u64,
        has_speech_energy: bool,
        stable_target_end_ms: Option<u64>,
    ) -> Option<(
        u64,
        crate::speaker_verification::SessionSpeakerClassification,
        crate::speech_decision_kernel::TranscriptSpeakerEvidence,
        bool,
    )> {
        self.rolling_pcm.extend_from_slice(pcm);
        if self.rolling_pcm.len() > LOCAL_SPEAKER_CLASSIFY_WINDOW_BYTES {
            let overflow =
                (self.rolling_pcm.len() - LOCAL_SPEAKER_CLASSIFY_WINDOW_BYTES + 1) & !1usize;
            self.rolling_pcm.drain(..overflow);
        }

        if let Some(rx) = self.profile_rx.as_ref() {
            match rx.try_recv() {
                Ok(Ok(profile)) => {
                    self.profile = Some(profile);
                    self.profile_rx = None;
                    log::info!("[speaker-verification] local session-speaker tracker ready");
                }
                Ok(Err(err)) => {
                    self.profile_rx = None;
                    log::warn!(
                        "[speaker-verification] local session-speaker tracker unavailable: {err}"
                    );
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.profile_rx = None;
                    log::warn!(
                        "[speaker-verification] local session-speaker profile task disconnected"
                    );
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        let mut completed = None;
        if let Some(rx) = self.classification_rx.as_ref() {
            match rx.try_recv() {
                Ok(Ok((audio_end_ms, observation))) => {
                    self.adaptation_gate.note(audio_end_ms, observation.clone());
                    completed = Some((
                        audio_end_ms,
                        observation.classification,
                        observation.transcript_speaker_evidence,
                        observation.signal_quality_sufficient,
                    ));
                    self.maybe_adapt_owner_bank_from_confident_body(&observation);
                    self.classification_rx = None;
                }
                Ok(Err(err)) => {
                    self.classification_rx = None;
                    log::debug!(
                        "[speaker-verification] local session-speaker sample uncertain: {err}"
                    );
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.classification_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        if let Some(profile) = self.profile.as_mut() {
            let adapted = self
                .adaptation_gate
                .promote_covered(profile, stable_target_end_ms);
            if adapted > 0 {
                log::info!(
                    "[speaker-verification] local session target profile adapted exemplars_added={adapted} stable_target_end_ms={}",
                    stable_target_end_ms.unwrap_or_default()
                );
            }
        }

        if has_speech_energy
            && self.classification_rx.is_none()
            && self.rolling_pcm.len() >= LOCAL_SPEAKER_CLASSIFY_MIN_BYTES
            && audio_end_ms >= self.next_classification_audio_ms
        {
            if let Some(profile) = self.profile.clone() {
                let snapshot = self.rolling_pcm.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                self.classification_rx = Some(rx);
                self.next_classification_audio_ms =
                    audio_end_ms.saturating_add(LOCAL_SPEAKER_CLASSIFY_STEP_MS);
                tauri::async_runtime::spawn_blocking(move || {
                    let result =
                        crate::speaker_verification::observe_session_speaker(&profile, &snapshot)
                            .map(|classification| (audio_end_ms, classification));
                    let _ = tx.send(result);
                });
            }
        }
        completed
    }
}

impl EmbeddedAudioDictationSession {
    fn release_terminal_wake_body(
        &mut self,
        inner: &Arc<Inner>,
        body: TerminalWakeBody,
    ) -> Result<(), String> {
        if body.pcm.is_empty() {
            log::info!(
                "[wake-phrase] terminal continuation body is empty; fresh session will own all PCM candidate_id={}",
                body.candidate_id
            );
            return Ok(());
        }

        let release_pieces = terminal_wake_body_release_pieces(&body);
        let mut accepted_total = 0usize;
        for piece in release_pieces {
            let release_bytes = piece.bytes;
            let candidate_range = slice_candidate_range(
                body.candidate_range,
                piece.offset,
                release_bytes,
            );
            let destination_start = self.accepted_pcm_cursor.position();
            let accepted_bytes_before = self.streamed_pcm_bytes;
            let release_result = self.consume_streaming_pcm_from_segment_with_collector_metadata(
                inner,
                &body.pcm[piece.offset..piece.offset + release_bytes],
                None,
                piece.segment_id,
                None,
                false,
                piece.collector_metadata,
            );
            let release_source_interval = release_result.as_ref().ok().copied().flatten();
            let accepted_bytes = self
                .streamed_pcm_bytes
                .saturating_sub(accepted_bytes_before);
            let destination_range = destination_start
                .zip(self.accepted_pcm_cursor.position())
                .and_then(|(start, end)| {
                    (end >= start && end - start == accepted_bytes as u64).then_some(
                        crate::observability::PcmRange { start, end },
                    )
                });
            if accepted_bytes > 0 {
                let operation_id = self.next_source_admission_operation_id();
                record_terminal_wake_body_source_dependencies(
                    &body,
                    candidate_range,
                    destination_range,
                    release_source_interval,
                    release_bytes,
                    accepted_bytes,
                    SourceAdmissionOperationOwner::Session {
                        session_id: self.session_id,
                    },
                    operation_id,
                    &self.source_admission_ledger,
                );
            }
            match release_result {
                Ok(_) if accepted_bytes == release_bytes => {
                    accepted_total = accepted_total.saturating_add(accepted_bytes);
                }
                Ok(_) => {
                    log::error!(
                        "[wake-phrase] terminal continuation body release was not fully accepted session_id={} candidate_id={} release_bytes={} accepted_bytes={}",
                        self.session_id,
                        body.candidate_id,
                        release_bytes,
                        accepted_bytes
                    );
                    return Err("终止唤醒续录正文未完整交接".to_string());
                }
                Err(err) => {
                    log::error!(
                        "[wake-phrase] terminal continuation body release failed session_id={} candidate_id={} release_bytes={} accepted_bytes={} error={err}",
                        self.session_id,
                        body.candidate_id,
                        release_bytes,
                        accepted_bytes
                    );
                    return Err(err);
                }
            }
        }
        log::info!(
            "[wake-phrase] terminal continuation body released exactly once session_id={} candidate_id={} pcm_bytes={} accepted_pcm_bytes={} candidate_range={:?}",
            self.session_id,
            body.candidate_id,
            body.pcm.len(),
            accepted_total,
            body.candidate_range
        );
        Ok(())
    }

    fn next_source_admission_operation_id(&mut self) -> Option<u64> {
        let operation_id = self.source_admission_operation_id;
        if let Some(current) = operation_id {
            self.source_admission_operation_id = current.checked_add(1);
            if self.source_admission_operation_id.is_none() {
                self.source_admission_ledger
                    .lock()
                    .expect("source admission dependency ledger lock")
                    .mark_incomplete();
            }
        } else {
            self.source_admission_ledger
                .lock()
                .expect("source admission dependency ledger lock")
                .mark_incomplete();
        }
        operation_id
    }

    fn next_pcm_stage_operation_id(&mut self) -> Option<u64> {
        let operation_id = self.pcm_stage_operation_id;
        if let Some(current) = operation_id {
            self.pcm_stage_operation_id = current.checked_add(1);
            if self.pcm_stage_operation_id.is_none() {
                self.pcm_stage_ledger.mark_incomplete();
            }
        } else {
            self.pcm_stage_ledger.mark_incomplete();
        }
        operation_id
    }

    fn record_owned_pcm_stage_mapping_fact(
        &mut self,
        observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        logical_stream_id: u64,
        operation_id: Option<u64>,
        stage: crate::observability::PcmStage,
        disposition: crate::observability::PcmStageDisposition,
        mapping: crate::observability::PcmMappingKind,
        destination_range: Option<crate::observability::PcmRange>,
        source_shares: Vec<crate::observability::AsrSourceShareFact>,
    ) {
        let fact = make_pcm_stage_mapping_fact(
            logical_stream_id,
            operation_id,
            stage,
            disposition,
            mapping,
            destination_range,
            source_shares,
        );
        self.pcm_stage_ledger.record(fact.clone());
        if let Some(observation) = observation {
            observation.record_pcm_stage_mapping(fact);
        }
    }

    fn record_owned_pcm_stage_mapping_for_run(
        &mut self,
        observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        logical_stream_id: u64,
        operation_id: Option<u64>,
        stage: crate::observability::PcmStage,
        disposition: crate::observability::PcmStageDisposition,
        mapping: crate::observability::PcmMappingKind,
        destination_range: Option<crate::observability::PcmRange>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
        collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
        bytes: usize,
    ) {
        self.record_owned_pcm_stage_mapping_fact(
            observation,
            logical_stream_id,
            operation_id,
            stage,
            disposition,
            mapping,
            destination_range,
            vec![crate::observability::AsrSourceShareFact {
                segment_id,
                bytes: bytes as u64,
                destination_range,
                source_interval,
                collector_metadata,
                collector_emitted_range: collector_metadata
                    .map(|metadata| metadata.emitted_range),
            }],
        );
    }

    fn attach_pipeline_observation(
        &mut self,
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    ) {
        self.pipeline_observation = observation.clone();
        if let (Some(asr), Some(observation)) = (self.volcengine_asr.as_ref(), observation) {
            asr.set_pipeline_observation(observation);
        }
    }

    fn start_local_speaker_tracking(
        &mut self,
        wake_pcm: Vec<u8>,
        wake_end_seconds: f32,
        wake_phrase: String,
        enrolled_owner_matched: bool,
    ) {
        if let Some(asr) = self.volcengine_asr.as_ref() {
            if enrolled_owner_matched {
                asr.note_verified_local_speaker_tracking_started(&wake_phrase);
            } else {
                // Open policy may accept a phrase without proving identity. It
                // can seed an adaptive session profile, but must not enable the
                // strict enrolled-owner transcript/endpoint path.
                asr.note_local_speaker_tracking_started(&wake_phrase);
            }
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            asr.start_target_speaker_extraction(
                &wake_phrase,
                &wake_pcm,
                wake_end_seconds,
            );
        }
        self.local_speaker_tracker = Some(LocalSessionSpeakerTracker::from_wake(
            wake_pcm,
            wake_end_seconds,
            wake_phrase,
            enrolled_owner_matched,
        ));
    }

    fn consume_streaming_pcm(
        &mut self,
        inner: &Arc<Inner>,
        pcm: &[u8],
        raw_input_level_percent: Option<u8>,
    ) -> Result<(), String> {
        self.consume_streaming_pcm_from_segment(
            inner,
            pcm,
            raw_input_level_percent,
            None,
        )
        .map(|_| ())
    }

    fn consume_streaming_pcm_from_segment(
        &mut self,
        inner: &Arc<Inner>,
        pcm: &[u8],
        raw_input_level_percent: Option<u8>,
        segment_id: Option<u32>,
    ) -> Result<Option<crate::observability::PcmSourceInterval>, String> {
        self.consume_streaming_pcm_from_segment_with_observation(
            inner,
            pcm,
            raw_input_level_percent,
            segment_id,
            None,
            false,
        )
    }

    fn consume_streaming_pcm_from_segment_with_observation(
        &mut self,
        inner: &Arc<Inner>,
        pcm: &[u8],
        raw_input_level_percent: Option<u8>,
        segment_id: Option<u32>,
        source_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        source_observation_is_explicit: bool,
    ) -> Result<Option<crate::observability::PcmSourceInterval>, String> {
        self.consume_streaming_pcm_from_segment_with_collector_metadata(
            inner,
            pcm,
            raw_input_level_percent,
            segment_id,
            source_observation,
            source_observation_is_explicit,
            None,
        )
    }

    fn consume_streaming_pcm_from_segment_with_collector_metadata(
        &mut self,
        inner: &Arc<Inner>,
        pcm: &[u8],
        raw_input_level_percent: Option<u8>,
        segment_id: Option<u32>,
        source_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        source_observation_is_explicit: bool,
        collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    ) -> Result<Option<crate::observability::PcmSourceInterval>, String> {
        if pcm.is_empty() {
            return Ok(None);
        }
        let observation = if source_observation_is_explicit {
            source_observation
        } else {
            self.pipeline_observation.clone()
        };
        // The session-level observation owns destination coordinates. An
        // explicit source observation may be different (for mixed physical
        // sources), so it must not be the only place where destination bytes
        // are recorded.
        let destination_observation = self
            .pipeline_observation
            .clone()
            .or_else(|| observation.clone());
        if pcm.len() % 2 != 0 {
                if let Some(observation) = observation.as_ref() {
                    observation.record_consumer_result_for_segment(
                        segment_id,
                        false,
                        pcm.len(),
                    );
                }
            return Err("嵌入式音频 PCM chunk 长度不是 16-bit 对齐".to_string());
        }

        if embedded_audio_stop_feedback_latched(inner) {
            if !embedded_audio_streaming_session_accepts_pcm(inner, self.session_id) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                if let Some(observation) = observation.as_ref() {
                    observation.record_consumer_result_for_segment(
                        segment_id,
                        false,
                        pcm.len(),
                    );
                }
                return Ok(None);
            }
        } else {
            if !emit_embedded_audio_pcm_capsule_if_active(
                inner,
                self.session_id,
                CapsuleState::Recording,
                embedded_pcm_capsule_level(pcm, raw_input_level_percent),
            ) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                if let Some(observation) = observation.as_ref() {
                    observation.record_consumer_result_for_segment(
                        segment_id,
                        false,
                        pcm.len(),
                    );
                }
                return Ok(None);
            }
        }

        if let Some(observation) = observation.as_ref() {
            observation.record_consumer_result_for_segment(segment_id, true, pcm.len());
        }
        let pcm_operation_id = self.next_pcm_stage_operation_id();

        // Keep the production byte-operation order stable: archive first,
        // then coordinator streaming buffer. Mapping facts are emitted after
        // source interval allocation, but never reorder the PCM operations.
        let archive_destination_range = if let Some(archive_pcm) = self.archive_pcm.as_mut() {
            let destination_range = self.archive_pcm_cursor.append(pcm.len());
            if destination_range.is_none() {
                self.pcm_stage_ledger.mark_incomplete();
            }
            archive_pcm.extend_from_slice(pcm);
            destination_range
        } else {
            None
        };
        let accepted_destination_range = self.accepted_pcm_cursor.append(pcm.len());
        if accepted_destination_range.is_none() {
            self.pcm_stage_ledger.mark_incomplete();
        }
        self.streamed_pcm_bytes += pcm.len();
        self.streaming_pcm_buffer.extend_from_slice(pcm);
        let source_interval = self.append_streaming_pcm_source(
            pcm.len(),
            observation.clone(),
            segment_id,
            self.source_stream_id,
            collector_metadata,
        );
        self.record_owned_pcm_stage_mapping_for_run(
            destination_observation.as_ref(),
            self.source_stream_id,
            pcm_operation_id,
            crate::observability::PcmStage::CoordinatorAccepted,
            accepted_destination_range
                .map(|_| crate::observability::PcmStageDisposition::Appended)
                .unwrap_or(crate::observability::PcmStageDisposition::Unknown),
            accepted_destination_range
                .zip(source_interval)
                .map(|_| crate::observability::PcmMappingKind::PositionPreserving)
                .unwrap_or(crate::observability::PcmMappingKind::Unknown),
            accepted_destination_range,
            segment_id,
            source_interval,
            collector_metadata,
            pcm.len(),
        );
        self.record_owned_pcm_stage_mapping_for_run(
            destination_observation.as_ref(),
            self.source_stream_id,
            pcm_operation_id,
            crate::observability::PcmStage::Archive,
            if self.archive_pcm.is_some() {
                archive_destination_range
                    .map(|_| crate::observability::PcmStageDisposition::Appended)
                    .unwrap_or(crate::observability::PcmStageDisposition::Unknown)
            } else {
                crate::observability::PcmStageDisposition::Disabled
            },
            if self.archive_pcm.is_some() {
                archive_destination_range
                    .zip(source_interval)
                    .map(|_| crate::observability::PcmMappingKind::PositionPreserving)
                    .unwrap_or(crate::observability::PcmMappingKind::Unknown)
            } else {
                crate::observability::PcmMappingKind::Unknown
            },
            archive_destination_range,
            segment_id,
            source_interval,
            collector_metadata,
            pcm.len(),
        );
        self.consume_ready_streaming_pcm_blocks();
        Ok(source_interval)
    }

    fn flush_streaming_pcm(&mut self) {
        if self.streaming_pcm_buffer.is_empty() {
            return;
        }

        let trailing_pcm = std::mem::take(&mut self.streaming_pcm_buffer);
        let source_runs = self.drain_streaming_pcm_sources(trailing_pcm.len());
        self.consume_prepared_streaming_pcm(&trailing_pcm, &source_runs);
    }

    fn consume_ready_streaming_pcm_blocks(&mut self) {
        let ready_bytes = self.streaming_pcm_buffer.len() / EMBEDDED_AUDIO_FEED_CHUNK_BYTES
            * EMBEDDED_AUDIO_FEED_CHUNK_BYTES;
        if ready_bytes == 0 {
            return;
        }

        let trailing_pcm = self.streaming_pcm_buffer.split_off(ready_bytes);
        let ready_pcm = std::mem::replace(&mut self.streaming_pcm_buffer, trailing_pcm);
        let mut source_runs = self.drain_streaming_pcm_sources(ready_bytes);
        for pcm_block in ready_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            let block_source_runs = take_streaming_pcm_source_runs(
                &mut source_runs,
                pcm_block.len(),
            );
            self.consume_prepared_streaming_pcm(pcm_block, &block_source_runs);
        }
    }

    fn append_streaming_pcm_source(
        &mut self,
        bytes: usize,
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_stream_id: u64,
        collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    ) -> Option<crate::observability::PcmSourceInterval> {
        if bytes == 0 {
            return None;
        }
        let source_interval = if self.active_asr == "volcengine" {
            observation
                .as_ref()
                .and_then(|observation| {
                    observation.allocate_source_interval(source_stream_id, segment_id, bytes)
                })
        } else {
            None
        };
        let can_merge = self.streaming_pcm_sources.back().is_some_and(|last| {
            last.segment_id == segment_id
                && last.collector_metadata == collector_metadata
                && collector_emitted_ranges_are_adjacent(
                    last.collector_emitted_range,
                    collector_metadata.map(|metadata| metadata.emitted_range),
                )
                && match (last.source_interval, source_interval) {
                    (Some(left), Some(right)) => left.same_stream_and_adjacent(right),
                    (None, None) => true,
                    _ => false,
                }
                && match (&last.observation, &observation) {
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    (None, None) => true,
                    _ => false,
                }
        });
        if can_merge {
            let last = self
                .streaming_pcm_sources
                .back_mut()
                .expect("source run exists");
            last.bytes = last.bytes.saturating_add(bytes);
            if let (Some(last_interval), Some(interval)) =
                (last.source_interval.as_mut(), source_interval)
            {
                last_interval.range.end = interval.range.end;
            }
            if let (Some(last_range), Some(range)) = (
                last.collector_emitted_range.as_mut(),
                collector_metadata.map(|metadata| metadata.emitted_range),
            ) {
                last_range.end = range.end;
            }
            return source_interval;
        }
        self.streaming_pcm_sources
            .push_back(EmbeddedStreamingPcmSourceRun {
                bytes,
                observation,
                segment_id,
                source_interval,
                collector_metadata,
                collector_emitted_range: collector_metadata.map(|metadata| metadata.emitted_range),
            });
        source_interval
    }

    fn drain_streaming_pcm_sources(
        &mut self,
        bytes: usize,
    ) -> Vec<EmbeddedStreamingPcmSourceRun> {
        let mut source_runs: Vec<EmbeddedStreamingPcmSourceRun> = Vec::new();
        let mut remaining = bytes;
        while remaining > 0 {
            let Some(mut run) = self.streaming_pcm_sources.pop_front() else {
                break;
            };
            let portion = run.bytes.min(remaining);
            if portion > 0 {
                let portion_interval = if let Some(interval) = run.source_interval.as_mut() {
                    match interval.take_prefix(portion) {
                        Some(interval) => Some(interval),
                        None => {
                            run.observation.as_ref().map(|observation| {
                                observation.mark_source_interval_incomplete()
                            });
                            run.source_interval = None;
                            None
                        }
                    }
                } else {
                    None
                };
                let portion_collector_emitted_range =
                    take_collector_emitted_range(&mut run.collector_emitted_range, portion);
                let can_merge = source_runs.last().is_some_and(|last| {
                    last.segment_id == run.segment_id
                        && last.collector_metadata == run.collector_metadata
                        && collector_emitted_ranges_are_adjacent(
                            last.collector_emitted_range,
                            portion_collector_emitted_range,
                        )
                        && match (last.source_interval, portion_interval) {
                            (Some(left), Some(right)) => left.same_stream_and_adjacent(right),
                            (None, None) => true,
                            _ => false,
                        }
                        && match (&last.observation, &run.observation) {
                            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                            (None, None) => true,
                            _ => false,
                        }
                });
                if can_merge {
                    let last = source_runs.last_mut().expect("source run exists");
                    last.bytes = last.bytes.saturating_add(portion);
                    if let (Some(last_interval), Some(interval)) =
                        (last.source_interval.as_mut(), portion_interval)
                    {
                        last_interval.range.end = interval.range.end;
                    }
                    if let (Some(last_range), Some(range)) = (
                        last.collector_emitted_range.as_mut(),
                        portion_collector_emitted_range,
                    ) {
                        last_range.end = range.end;
                    }
                } else {
                    source_runs.push(EmbeddedStreamingPcmSourceRun {
                        bytes: portion,
                        observation: run.observation.clone(),
                        segment_id: run.segment_id,
                        source_interval: portion_interval,
                        collector_metadata: run.collector_metadata,
                        collector_emitted_range: portion_collector_emitted_range,
                    });
                }
                remaining -= portion;
                run.bytes -= portion;
            }
            if run.bytes > 0 {
                self.streaming_pcm_sources.push_front(run);
            }
        }
        source_runs
    }

    fn consume_prepared_streaming_pcm(
        &mut self,
        pcm: &[u8],
        source_runs: &[EmbeddedStreamingPcmSourceRun],
    ) {
        let source_pcm_offset_ms = (self.normalized_pcm_bytes as u64) / 32;
        let source_pcm_offset_samples = (self.normalized_pcm_bytes as u64) / 2;
        let chunk_ms = (pcm.len() / 32) as u64;
        let audio_end_samples = source_pcm_offset_samples.saturating_add((pcm.len() / 2) as u64);
        let (asr_pcm, gain_stats) = self.prepare_streaming_pcm_for_asr(pcm);
        let has_speech_energy = embedded_streaming_chunk_has_speech_energy(
            gain_stats.rms_before,
            gain_stats.peak_before,
        );
        if self.active_asr == "volcengine" && has_speech_energy {
            self.streaming_agc
                .first_voiced_pcm_ms
                .get_or_insert(source_pcm_offset_ms);
        }
        let audio_end_ms = source_pcm_offset_ms.saturating_add(chunk_ms);
        if let Some(asr) = self.volcengine_asr.as_ref() {
            // Keep the legacy raw energy edge for diagnostics and existing
            // speaker evidence. The endpoint watchdog gets an independent VAD
            // overlay below; it must not globally replace this field because
            // wake/identity code still uses the raw capture clock.
            asr.note_local_audio_activity_samples(
                audio_end_ms,
                audio_end_samples,
                has_speech_energy,
            );
            let local_speech_evidence =
                self.local_speech_activity
                    .submit(source_pcm_offset_samples, audio_end_samples, pcm);
            asr.note_local_speech_activity(local_speech_evidence);
        }
        let stable_target_end_ms = self
            .volcengine_asr
            .as_ref()
            .and_then(|asr| asr.stable_target_speech_end_ms());
        let local_speaker_evidence = self.local_speaker_tracker.as_mut().and_then(|tracker| {
            tracker.observe(
                pcm,
                source_pcm_offset_ms.saturating_add(chunk_ms),
                has_speech_energy,
                stable_target_end_ms,
            )
        });
        if let (Some(asr), Some(adaptive)) = (
            self.volcengine_asr.as_ref(),
            self.local_speaker_tracker
                .as_ref()
                .and_then(LocalSessionSpeakerTracker::profile_is_adaptive),
        ) {
            asr.note_local_speaker_profile_adaptive(adaptive);
        }
        if let (Some(asr), Some((audio_end_ms, classification, transcript_speaker_evidence, signal_quality_sufficient))) =
            (self.volcengine_asr.as_ref(), local_speaker_evidence)
        {
            asr.note_local_speaker_observation_with_quality(
                audio_end_ms,
                classification,
                transcript_speaker_evidence,
                signal_quality_sufficient,
            );
        }
        if let (Some(asr), Some(tracker)) =
            (self.volcengine_asr.as_ref(), self.local_speaker_tracker.as_ref())
        {
            tracker.reanchor_if_lost(asr);
            // Update this after publishing a completed observation.  That
            // keeps the old in-flight guard active while the result callback
            // is reduced, then exposes whether a successor job was started in
            // the same audio pass.
            asr.note_local_speaker_analysis_pending(tracker.classification_pending());
        }

        // 改A: track sustained trailing silence AFTER the body has started so the
        // caller can request a host-initiated device stop early. Leading silence
        // (before the user speaks the dictation body) and post-stop tails never
        // count toward the threshold.
        let (signal_rms, signal_peak) = embedded_pcm_streaming_agc_signal_level(pcm);
        if embedded_streaming_chunk_has_speech_energy(signal_rms, signal_peak) {
            self.proactive_stop_body_started = true;
            self.proactive_stop_silence_ms = 0;
        } else if self.proactive_stop_body_started {
            self.proactive_stop_silence_ms =
                self.proactive_stop_silence_ms.saturating_add(chunk_ms);
        }

        let normalized_input_bytes = pcm.len();
        let destination_observation = self.pipeline_observation.clone();
        self.consume_normalized_pcm_with_source_runs(
            normalized_input_bytes,
            &asr_pcm,
            source_runs,
            destination_observation,
            crate::observability::PcmMappingKind::PositionPreserving,
        );
    }

    /// Route the bytes produced by the normalizer while projecting only
    /// mappings that are actually proven. The caller supplies the real
    /// normalized output so the same production loop can be exercised for
    /// length-changing and missing-source diagnostics without changing the
    /// audio consumer's slicing or send order.
    fn consume_normalized_pcm_with_source_runs(
        &mut self,
        normalized_input_bytes: usize,
        asr_pcm: &[u8],
        source_runs: &[EmbeddedStreamingPcmSourceRun],
        destination_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        transform_mapping: crate::observability::PcmMappingKind,
    ) {
        let normalized_output_bytes = asr_pcm.len();
        let normalized_destination_start = self.normalized_pcm_cursor.position();
        let normalized_output_range = self.normalized_pcm_cursor.append(normalized_output_bytes);
        if normalized_output_bytes > 0 && normalized_output_range.is_none() {
            self.pcm_stage_ledger.mark_incomplete();
        }
        let operation_id = self.next_pcm_stage_operation_id();
        self.normalized_pcm_bytes += normalized_output_bytes;

        let source_bytes = source_runs
            .iter()
            .try_fold(0usize, |total, run| total.checked_add(run.bytes));
        let position_preserving = normalized_input_bytes == normalized_output_bytes
            && transform_mapping == crate::observability::PcmMappingKind::PositionPreserving
            && normalized_output_range.is_some()
            && source_bytes == Some(normalized_input_bytes)
            && !source_runs.is_empty()
            && source_runs.iter().all(|run| {
                run.observation.is_some() && run.source_interval.is_some()
            });

        if !position_preserving {
            // One normalized output operation owns one complete destination
            // range. Source observations receive optional local projections,
            // but the session ledger records this aggregate exactly once so
            // mixed sources and length changes cannot look like duplicate
            // output.
            let source_shares = if source_runs.is_empty() {
                vec![crate::observability::AsrSourceShareFact {
                    segment_id: None,
                    bytes: 0,
                    destination_range: None,
                    source_interval: None,
                    collector_metadata: None,
                    collector_emitted_range: None,
                }]
            } else {
                source_runs
                    .iter()
                    .map(|run| crate::observability::AsrSourceShareFact {
                        segment_id: run.segment_id,
                        bytes: run.bytes as u64,
                        destination_range: None,
                        source_interval: run.source_interval,
                        collector_metadata: run.collector_metadata,
                        collector_emitted_range: run.collector_emitted_range,
                    })
                    .collect()
            };
            let aggregate_fact = make_pcm_stage_mapping_fact(
                self.source_stream_id,
                operation_id,
                crate::observability::PcmStage::Normalized,
                if normalized_output_bytes == 0 {
                    crate::observability::PcmStageDisposition::Empty
                } else {
                    crate::observability::PcmStageDisposition::Appended
                },
                crate::observability::PcmMappingKind::Unknown,
                normalized_output_range,
                source_shares,
            );
            self.pcm_stage_ledger.record(aggregate_fact);

            if source_runs.is_empty() {
                if let Some(observation) = destination_observation.as_ref() {
                    observation.record_pcm_stage_mapping(make_pcm_stage_mapping_fact(
                        self.source_stream_id,
                        operation_id,
                        crate::observability::PcmStage::Normalized,
                        if normalized_output_bytes == 0 {
                            crate::observability::PcmStageDisposition::Empty
                        } else {
                            crate::observability::PcmStageDisposition::Appended
                        },
                        crate::observability::PcmMappingKind::Unknown,
                        normalized_output_range,
                        vec![crate::observability::AsrSourceShareFact {
                            segment_id: None,
                            bytes: 0,
                            destination_range: None,
                            source_interval: None,
                            collector_metadata: None,
                            collector_emitted_range: None,
                        }],
                    ));
                }
            } else {
                // These are source-local projections only. Their source
                // intervals remain known input facts, while destination
                // attribution stays absent after an unknown transform.
                for source_run in source_runs {
                    if let Some(observation) = source_run
                        .observation
                        .as_ref()
                        .or(destination_observation.as_ref())
                    {
                        observation.record_pcm_stage_mapping(make_pcm_stage_mapping_fact(
                            self.source_stream_id,
                            operation_id,
                            crate::observability::PcmStage::Normalized,
                            if normalized_output_bytes == 0 {
                                crate::observability::PcmStageDisposition::Empty
                            } else {
                                crate::observability::PcmStageDisposition::Appended
                            },
                            crate::observability::PcmMappingKind::Unknown,
                            normalized_output_range,
                            vec![crate::observability::AsrSourceShareFact {
                                segment_id: source_run.segment_id,
                                bytes: source_run.bytes as u64,
                                destination_range: None,
                                source_interval: source_run.source_interval,
                                collector_metadata: source_run.collector_metadata,
                                collector_emitted_range: source_run.collector_emitted_range,
                            }],
                        ));
                    }
                }
            }

            // Preserve the existing consumer slicing and order even though
            // no source interval is safe to attach to the output.
            let mut offset = 0usize;
            for source_run in source_runs {
                let portion = source_run
                    .bytes
                    .min(normalized_output_bytes.saturating_sub(offset));
                if portion > 0 {
                    self.consumer.consume_pcm_chunk_with_source_interval(
                        &asr_pcm[offset..offset + portion],
                        source_run.observation.clone(),
                        source_run.segment_id,
                        None,
                    );
                    offset += portion;
                }
            }
            if offset < normalized_output_bytes {
                self.consumer
                    .consume_pcm_chunk_with_source_interval(&asr_pcm[offset..], None, None, None);
            }
            return;
        }

        let mut offset = 0usize;
        for source_run in source_runs {
            let portion = normalized_stage_portion(
                normalized_destination_start,
                normalized_input_bytes,
                normalized_output_bytes,
                offset,
                source_run.bytes,
            );
            let source_interval = source_run.source_interval;
            self.record_owned_pcm_stage_mapping_fact(
                source_run
                    .observation
                    .as_ref()
                    .or(destination_observation.as_ref()),
                self.source_stream_id,
                operation_id,
                crate::observability::PcmStage::Normalized,
                crate::observability::PcmStageDisposition::Appended,
                crate::observability::PcmMappingKind::PositionPreserving,
                portion.destination_range,
                vec![crate::observability::AsrSourceShareFact {
                    segment_id: source_run.segment_id,
                    bytes: source_run.bytes as u64,
                    destination_range: portion.destination_range,
                    source_interval,
                    collector_metadata: source_run.collector_metadata,
                    collector_emitted_range: source_run.collector_emitted_range,
                }],
            );
            self.consumer.consume_pcm_chunk_with_source_interval(
                &asr_pcm[offset..offset + portion.output_bytes],
                source_run.observation.clone(),
                source_run.segment_id,
                source_interval,
            );
            offset += portion.output_bytes;
        }
    }

    fn prepare_streaming_pcm_for_asr(&mut self, pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
        if self.active_asr == "volcengine" {
            // The buffer is bounded to one established provider feed interval.
            return normalize_embedded_streaming_pcm_for_asr(pcm, &mut self.streaming_agc);
        }

        normalize_embedded_pcm_for_asr(pcm)
    }
}

fn take_streaming_pcm_source_runs(
    source_runs: &mut Vec<EmbeddedStreamingPcmSourceRun>,
    bytes: usize,
) -> Vec<EmbeddedStreamingPcmSourceRun> {
    let mut remaining = bytes;
    let mut taken: Vec<EmbeddedStreamingPcmSourceRun> = Vec::new();
    while remaining > 0 {
        let Some(first) = source_runs.first() else {
            break;
        };
        if first.bytes == 0 {
            source_runs.remove(0);
            continue;
        }
        let run_bytes = first.bytes;
        let observation = first.observation.clone();
        let segment_id = first.segment_id;
        let collector_metadata = first.collector_metadata;
        let mut collector_emitted_range = first.collector_emitted_range;
        let mut source_interval = first.source_interval;
        let portion = run_bytes.min(remaining);
        if portion == 0 {
            source_runs.remove(0);
            continue;
        }
        let portion_interval = if let Some(interval) = source_interval.as_mut() {
            match interval.take_prefix(portion) {
                Some(interval) => Some(interval),
                None => {
                    observation.as_ref().map(|observation| {
                        observation.mark_source_interval_incomplete()
                    });
                    source_interval = None;
                    if let Some(first) = source_runs.first_mut() {
                        first.source_interval = None;
                    }
                    None
                }
            }
        } else {
            None
        };
        let portion_collector_emitted_range =
            take_collector_emitted_range(&mut collector_emitted_range, portion);
        let can_merge = taken.last().is_some_and(|last| {
            last.segment_id == segment_id
                && last.collector_metadata == collector_metadata
                && collector_emitted_ranges_are_adjacent(
                    last.collector_emitted_range,
                    portion_collector_emitted_range,
                )
                && match (last.source_interval, portion_interval) {
                    (Some(left), Some(right)) => left.same_stream_and_adjacent(right),
                    (None, None) => true,
                    _ => false,
                }
                && match (&last.observation, &observation) {
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    (None, None) => true,
                    _ => false,
                }
        });
        if can_merge {
            let last = taken.last_mut().expect("source run exists");
            last.bytes = last.bytes.saturating_add(portion);
            if let (Some(last_interval), Some(interval)) =
                (last.source_interval.as_mut(), portion_interval)
            {
                last_interval.range.end = interval.range.end;
            }
            if let (Some(last_range), Some(range)) = (
                last.collector_emitted_range.as_mut(),
                portion_collector_emitted_range,
            ) {
                last_range.end = range.end;
            }
        } else {
            taken.push(EmbeddedStreamingPcmSourceRun {
                bytes: portion,
                observation,
                segment_id,
                source_interval: portion_interval,
                collector_metadata,
                collector_emitted_range: portion_collector_emitted_range,
            });
        }
        remaining -= portion;
        if let Some(first) = source_runs.first_mut() {
            first.bytes -= portion;
            first.source_interval = source_interval;
            first.collector_emitted_range = collector_emitted_range;
            if first.bytes == 0 {
                source_runs.remove(0);
            }
        }
    }
    taken
}

fn collector_emitted_ranges_are_adjacent(
    left: Option<crate::embedded_audio::StreamingPcmRange>,
    right: Option<crate::embedded_audio::StreamingPcmRange>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.end == right.start,
        (None, None) => true,
        _ => false,
    }
}

fn take_collector_emitted_range(
    range: &mut Option<crate::embedded_audio::StreamingPcmRange>,
    bytes: usize,
) -> Option<crate::embedded_audio::StreamingPcmRange> {
    let current = (*range)?;
    let bytes = u64::try_from(bytes).ok()?;
    let end = current.start.checked_add(bytes)?;
    if end > current.end {
        return None;
    }
    *range = Some(crate::embedded_audio::StreamingPcmRange {
        start: end,
        end: current.end,
    });
    Some(crate::embedded_audio::StreamingPcmRange {
        start: current.start,
        end,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferedSpeakerCandidateKind {
    Enrollment,
    Verification,
    Rejected,
}

#[derive(Debug, Default)]
struct WakeInterferenceBaseline {
    score: f32,
    samples: u8,
}

const WAKE_INTERFERENCE_BASELINE_MIN_SAMPLES: u8 = 3;
const WAKE_INTERFERENCE_OWNER_RISE_MIN_SCORE: f32 = 0.28;
const WAKE_INTERFERENCE_OWNER_RISE_MARGIN: f32 = 0.12;

impl WakeInterferenceBaseline {
    /// Learn only terminal no-phrase mixtures. A relative owner-score rise is
    /// not accepted as wake; it merely makes the expensive separated-owner
    /// phrase check reachable when both mixed-audio phrase models are masked.
    fn observe(&mut self, score: f32, mixed_phrase_seen: bool) -> bool {
        if !score.is_finite() || mixed_phrase_seen {
            return false;
        }
        // A candidate may reach terminal arbitration before a second owner
        // snapshot exists. In that case a single sufficiently strong owner
        // score is enough to request the bounded separated-track check; the
        // separated result still has to pass phrase + owner verification and
        // can never activate the product session by itself. Once a candidate
        // has a few ambient samples, retain the stricter relative-rise rule.
        let owner_rise = (self.samples == 0 && score >= WAKE_INTERFERENCE_OWNER_RISE_MIN_SCORE)
            || (self.samples >= WAKE_INTERFERENCE_BASELINE_MIN_SAMPLES
                && score >= WAKE_INTERFERENCE_OWNER_RISE_MIN_SCORE
                && score >= self.score + WAKE_INTERFERENCE_OWNER_RISE_MARGIN);
        if owner_rise {
            return true;
        }
        if self.samples == 0 {
            self.score = score;
        } else {
            // Slow EWMA keeps a long interference bed stable without letting
            // one possible owner attempt become the new baseline.
            self.score = (self.score * 7.0 + score) / 8.0;
        }
        self.samples = self.samples.saturating_add(1);
        false
    }
}

static WAKE_DIAGNOSTIC_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);
static WAKE_DIAGNOSTIC_CLEANUP_RUNNING: AtomicBool = AtomicBool::new(false);

include!("dictation_wake_diagnostic_retention.rs");

fn schedule_default_wake_diagnostic_cleanup(directory: std::path::PathBuf) {
    if WAKE_DIAGNOSTIC_CLEANUP_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let spawn = std::thread::Builder::new()
        .name("wake-diag-retention".to_string())
        .spawn(move || {
            match prune_default_wake_diagnostics(&directory) {
                Ok(removed) if removed > 0 => log::info!(
                    "[wake-phrase] diagnostic rolling cleanup removed={} max_files={} max_bytes={} max_age_days=7",
                    removed,
                    WAKE_DIAGNOSTIC_RETENTION_MAX_FILES,
                    WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES
                ),
                Ok(_) => {}
                Err(err) => log::warn!("[wake-phrase] diagnostic rolling cleanup failed: {err}"),
            }
            WAKE_DIAGNOSTIC_CLEANUP_RUNNING.store(false, Ordering::SeqCst);
        });
    if let Err(err) = spawn {
        WAKE_DIAGNOSTIC_CLEANUP_RUNNING.store(false, Ordering::SeqCst);
        log::warn!("[wake-phrase] diagnostic cleanup thread unavailable: {err}");
    }
}

fn next_wake_diagnostic_capture_count(is_default_directory: bool, current: usize) -> Option<usize> {
    if is_default_directory || current < WAKE_DIAGNOSTIC_MAX_CANDIDATES {
        current.checked_add(1)
    } else {
        None
    }
}

fn explicit_wake_diagnostic_directory(value: Option<String>) -> Option<std::path::PathBuf> {
    value
        .filter(|directory| !directory.trim().is_empty())
        .map(std::path::PathBuf::from)
}

fn pcm16_wav_bytes(pcm: &[u8]) -> Vec<u8> {
    let pcm_len = pcm.len().min(WAKE_DIAGNOSTIC_MAX_PCM_BYTES) & !1usize;
    let pcm = &pcm[..pcm_len];
    let data_size = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(data_size)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

fn save_bounded_wake_diagnostic(embedded_session_id: u32, outcome: &'static str, pcm: &[u8]) {
    // Wake candidates contain ambient room speech. Production must never write
    // them implicitly: capture is available only in an explicitly selected
    // operator directory, and that directory still uses the bounded retention
    // worker below.
    let Some(directory) =
        explicit_wake_diagnostic_directory(std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).ok())
    else {
        return;
    };
    let Ok(index) =
        WAKE_DIAGNOSTIC_CAPTURE_COUNT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            next_wake_diagnostic_capture_count(false, current)
        })
    else {
        return;
    };
    if let Err(err) = fs::create_dir_all(&directory) {
        log::warn!("[wake-phrase] diagnostic directory unavailable: {err}");
        return;
    }
    let wav = pcm16_wav_bytes(pcm);
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!(
        "wake-candidate-{timestamp_ms}-{}-{index:03}-session-{embedded_session_id}-{outcome}.wav",
        std::process::id()
    ));
    match fs::write(&path, &wav) {
        Ok(()) => {
            log::info!(
                "[wake-phrase] bounded local diagnostic saved index={} embedded_session_id={} outcome={} pcm_ms={}",
                index,
                embedded_session_id,
                outcome,
                wav.len().saturating_sub(44) / 32
            );
            schedule_default_wake_diagnostic_cleanup(directory);
        }
        Err(err) => log::warn!("[wake-phrase] diagnostic WAV write failed: {err}"),
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy)]
struct LocalWakeConfirmationDiagnosticContext {
    embedded_session_id: u32,
    attempt: Option<usize>,
    source_origin_bytes: usize,
    branch: &'static str,
}

#[cfg(target_os = "windows")]
fn save_local_wake_confirmation_inputs(
    context: LocalWakeConfirmationDiagnosticContext,
    phase: &'static str,
    raw_pcm: &[u8],
    boosted_pcm: &[u8],
) {
    // This is deliberately opt-in and runs inside the existing blocking helper
    // task. It records the exact two buffers submitted to the helper, not a
    // reconstructed candidate snapshot, while leaving the audio actor free.
    let Some(directory) =
        explicit_wake_diagnostic_directory(std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).ok())
    else {
        return;
    };
    let Ok(index) = WAKE_DIAGNOSTIC_CAPTURE_COUNT.fetch_update(
        Ordering::SeqCst,
        Ordering::SeqCst,
        |current| next_wake_diagnostic_capture_count(false, current),
    ) else {
        return;
    };
    if let Err(err) = fs::create_dir_all(&directory) {
        log::warn!("[wake-phrase] stage2 diagnostic directory unavailable: {err}");
        return;
    }
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let attempt = context
        .attempt
        .map(|value| value.to_string())
        .unwrap_or_else(|| "terminal".to_string());
    let stem = format!(
        "wake-stage2-{timestamp_ms}-{}-{index:03}-session-{}-attempt-{attempt}-branch-{}-phase-{}-origin-{}",
        std::process::id(),
        context.embedded_session_id,
        context.branch,
        phase,
        context.source_origin_bytes / 32,
    );
    let raw_path = directory.join(format!("{stem}-raw.wav"));
    let boosted_path = directory.join(format!("{stem}-boosted.wav"));
    let raw_wav = pcm16_wav_bytes(raw_pcm);
    let boosted_wav = pcm16_wav_bytes(boosted_pcm);
    let raw_result = fs::write(&raw_path, &raw_wav);
    let boosted_result = fs::write(&boosted_path, &boosted_wav);
    match (raw_result, boosted_result) {
        (Ok(()), Ok(())) => {
            log::info!(
                "[wake-phrase] stage2 helper input captured embedded_session_id={} attempt={} branch={} phase={} origin_pcm_ms={} raw_pcm_ms={} boosted_pcm_ms={} raw_path={} boosted_path={}",
                context.embedded_session_id,
                attempt,
                context.branch,
                phase,
                context.source_origin_bytes / 32,
                raw_pcm.len() / 32,
                boosted_pcm.len() / 32,
                raw_path.display(),
                boosted_path.display(),
            );
            schedule_default_wake_diagnostic_cleanup(directory);
        }
        (raw, boosted) => {
            log::warn!(
                "[wake-phrase] stage2 helper input capture incomplete embedded_session_id={} attempt={} branch={} phase={} raw_ok={} boosted_ok={} raw_path={} boosted_path={}",
                context.embedded_session_id,
                attempt,
                context.branch,
                phase,
                raw.is_ok(),
                boosted.is_ok(),
                raw_path.display(),
                boosted_path.display(),
            );
        }
    }
}

fn clear_device_key_dictation_takeover_pending(inner: &Arc<Inner>) {
    inner
        .recording_lifecycle
        .lock()
        .clear_device_key_takeover_intent();
}

pub(super) fn hidden_automatic_candidate_active(inner: &Arc<Inner>) -> bool {
    inner.recording_lifecycle.lock().hidden_candidate_active()
}

/// Device-key Dictation Start: prefer promote/ACTIVATE over TOGGLE-stop of a live
/// hidden automatic session. Returns true when the host should send VREC:ACTIVATE.
pub(super) fn note_device_key_dictation_start_intent(inner: &Arc<Inner>) -> bool {
    let candidate_live = inner
        .recording_lifecycle
        .lock()
        .note_device_key_takeover_intent();
    if candidate_live {
        log::info!(
            "[speaker-verification] device-key start promotes active hidden automatic candidate"
        );
        return true;
    }
    // Candidate not host-visible yet (KWS init / notify lag). Keep takeover pending;
    // firmware also maps TOGGLE→activate for hidden automatic sessions.
    log::info!(
        "[speaker-verification] device-key start takeover pending until hidden candidate is ready"
    );
    false
}

pub(super) fn request_hidden_automatic_candidate_promotion(inner: &Arc<Inner>) -> bool {
    inner
        .recording_lifecycle
        .lock()
        .request_candidate_promotion()
}

fn take_hidden_automatic_candidate_promotion(
    inner: &Arc<Inner>,
    embedded_session_id: u32,
) -> bool {
    inner
        .recording_lifecycle
        .lock()
        .take_candidate_promotion(embedded_session_id)
}

fn note_hidden_wake_interference_owner_score(
    baseline: &mut WakeInterferenceBaseline,
    score: f32,
    mixed_phrase_seen: bool,
) -> (bool, f32, u8) {
    let owner_rise = baseline.observe(score, mixed_phrase_seen);
    (owner_rise, baseline.score, baseline.samples)
}

fn discard_pre_press_candidate_pcm(pcm: &mut Vec<u8>) -> usize {
    let discarded_pcm_bytes = pcm.len();
    pcm.clear();
    discarded_pcm_bytes
}

fn buffered_speaker_candidate_kind(
    start_origin: crate::embedded_audio::SessionStartOrigin,
    enrollment_armed: bool,
    enrolled: bool,
) -> Option<BufferedSpeakerCandidateKind> {
    if enrollment_armed {
        return Some(BufferedSpeakerCandidateKind::Enrollment);
    }
    match start_origin {
        crate::embedded_audio::SessionStartOrigin::User => None,
        // Automatic wake always enters the Verification gate path.
        // `speaker_verification::verify` already open-gates when no voiceprint is
        // enrolled (phrase hit alone accepts). Rejecting here when !enrolled made
        // "delete voiceprint" permanently disable wake — opposite of product intent.
        crate::embedded_audio::SessionStartOrigin::VoiceActivation => {
            let _ = enrolled;
            Some(BufferedSpeakerCandidateKind::Verification)
        }
        crate::embedded_audio::SessionStartOrigin::Unknown(_) => {
            Some(BufferedSpeakerCandidateKind::Rejected)
        }
    }
}

static NEXT_BUFFERED_CANDIDATE_ID: AtomicU64 = AtomicU64::new(1);

fn next_buffered_candidate_id() -> u64 {
    NEXT_BUFFERED_CANDIDATE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct BufferedCandidateSourceRun {
    bytes: usize,
    capture_generation: Option<u64>,
    segment_id: Option<u32>,
    candidate_range: Option<crate::observability::CandidateRange>,
    collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    collector_emitted_range: Option<crate::embedded_audio::StreamingPcmRange>,
    capture_admission_context: Option<CaptureAdmissionSourceContext>,
}

/// The bounded hand-off from a terminal wake candidate to the fresh body
/// session. `wake_pcm` remains the speaker-tracking anchor; `body` is the
/// distinct post-wake slice that is legal dictation input. Keeping the two
/// payloads separate prevents the wake anchor from being mistaken for user
/// speech while still preserving the original candidate/source coordinates.
pub(super) struct TerminalWakeContinuation {
    session_id: Option<SessionId>,
    wake_pcm: Vec<u8>,
    wake_end_seconds: f32,
    wake_phrase: String,
    enrolled_owner_matched: bool,
    body: TerminalWakeBody,
    expires_at: Instant,
}

struct TerminalWakeBody {
    candidate_id: u64,
    pcm: Vec<u8>,
    candidate_range: Option<crate::observability::CandidateRange>,
    source_runs: VecDeque<BufferedCandidateSourceRun>,
    source_admission_ledger: Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
}

struct TerminalWakeBodyReleasePiece {
    offset: usize,
    bytes: usize,
    segment_id: Option<u32>,
    collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
}

impl TerminalWakeBody {
    fn source_admission_contexts_for_range(
        &self,
        range: Option<crate::observability::CandidateRange>,
    ) -> Vec<(Option<CaptureAdmissionSourceContext>, usize, usize)> {
        let Some(range) = range else {
            return Vec::new();
        };
        self.source_runs
            .iter()
            .filter_map(|run| {
                let source = run.candidate_range?;
                let start = range.start.max(source.start);
                let end = range.end.min(source.end);
                if start >= end {
                    return None;
                }
                let offset = usize::try_from(start.checked_sub(range.start)?).ok()?;
                let bytes = usize::try_from(end.checked_sub(start)?).ok()?;
                Some((run.capture_admission_context.clone(), offset, bytes))
            })
            .collect()
    }

    fn possible_source_admission_contexts(
        &self,
        range: Option<crate::observability::CandidateRange>,
    ) -> Vec<Option<CaptureAdmissionSourceContext>> {
        self.source_runs
            .iter()
            .filter_map(|run| {
                if let (Some(requested), Some(source)) = (range, run.candidate_range) {
                    let overlaps = requested.start < source.end && source.start < requested.end;
                    if !overlaps {
                        return None;
                    }
                }
                Some(run.capture_admission_context.clone())
            })
            .collect()
    }
}

fn terminal_wake_body_release_pieces(
    body: &TerminalWakeBody,
) -> Vec<TerminalWakeBodyReleasePiece> {
    if body.pcm.is_empty() {
        return Vec::new();
    }
    let Some(body_range) = body.candidate_range else {
        return vec![TerminalWakeBodyReleasePiece {
            offset: 0,
            bytes: body.pcm.len(),
            segment_id: None,
            collector_metadata: None,
        }];
    };

    let mut mapped = body
        .source_runs
        .iter()
        .filter_map(|run| {
            let source = run.candidate_range?;
            let start = body_range.start.max(source.start);
            let end = body_range.end.min(source.end);
            if start >= end {
                return None;
            }
            let offset = usize::try_from(start.checked_sub(body_range.start)?).ok()?;
            let bytes = usize::try_from(end.checked_sub(start)?).ok()?;
            Some((
                offset,
                bytes,
                run.segment_id,
                run.collector_metadata,
            ))
        })
        .collect::<Vec<_>>();
    mapped.sort_by_key(|(offset, _, _, _)| *offset);

    let mut pieces = Vec::new();
    let mut cursor = 0usize;
    for (offset, bytes, segment_id, collector_metadata) in mapped {
        if cursor >= body.pcm.len() {
            break;
        }
        let end = offset.saturating_add(bytes).min(body.pcm.len());
        if end <= cursor {
            // Overlapping source runs cannot be assigned a definite owner;
            // keep the already-emitted bytes one-shot rather than duplicating
            // them into the fresh session.
            continue;
        }
        if offset > cursor {
            pieces.push(TerminalWakeBodyReleasePiece {
                offset: cursor,
                bytes: offset - cursor,
                segment_id: None,
                collector_metadata: None,
            });
        }
        let piece_offset = offset.max(cursor);
        pieces.push(TerminalWakeBodyReleasePiece {
            offset: piece_offset,
            bytes: end - piece_offset,
            segment_id,
            collector_metadata,
        });
        cursor = end;
    }
    if cursor < body.pcm.len() {
        pieces.push(TerminalWakeBodyReleasePiece {
            offset: cursor,
            bytes: body.pcm.len() - cursor,
            segment_id: None,
            collector_metadata: None,
        });
    }
    pieces
}

struct BufferedSpeakerCandidate {
    candidate_id: u64,
    kind: BufferedSpeakerCandidateKind,
    pcm: Vec<u8>,
    /// Candidate-coordinate base of the currently retained PCM slice. A
    /// failed addition is sticky UNKNOWN; retaining the old value would make
    /// the next release appear to belong to the wrong candidate range.
    pcm_base_offset: Option<u64>,
    candidate_cursor: PcmDiagnosticCursor,
    source_runs: VecDeque<BufferedCandidateSourceRun>,
    source_admission_ledger: Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
    fact_ledger: crate::observability::CandidateFactLedger,
    next_operation_id: Option<u64>,
    source_admission_operation_id: Option<u64>,
    wake_detector: Option<crate::wake_phrase::StreamingDetector>,
    /// Non-blocking detector init: begin buffers PCM immediately while this runs
    /// (~0.5–1s). Awaiting StreamingDetector::new before buffering made the
    /// capsule wait an extra second after the user already said 开始录音.
    wake_detector_init: Option<
        tauri::async_runtime::JoinHandle<Result<crate::wake_phrase::StreamingDetector, String>>,
    >,
    pending_phrase_match: Option<PendingAutomaticPhraseMatch>,
    /// A single borderline owner score is not enough to relax the enrolled
    /// voiceprint gate. Keep evidence across the bounded verification snapshots.
    owner_ambiguous_confirmations: u8,
    owner_best_ambiguous_score: f32,
    /// Voiceprint inference starts with the first live KWS hit and runs beside
    /// the bounded local-ASR secondary check. The secondary keeps its existing
    /// false-wake veto; this only removes the old serial 60 ms + inference wait.
    owner_verification_task: Option<
        tauri::async_runtime::JoinHandle<(
            Result<crate::speaker_verification::VerificationResult, String>,
            u64,
        )>,
    >,
    /// A candidate gets at most one speculative owner snapshot. Once that
    /// bounded task completes, do not immediately spawn another copy on every
    /// incoming PCM chunk; repeated inference starves preview/ASR processing.
    owner_verification_attempted: bool,
    /// Strong continuous Mandarin can hide the owner's phrase from both raw
    /// KWS and local ASR. Run one bounded, enrolled-owner extraction beside the
    /// established raw path; it may release only after the extracted output
    /// independently passes both phrase and voiceprint verification.
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_wake_extraction_task: Option<
        tauri::async_runtime::JoinHandle<
            Result<crate::asr::target_speaker_extraction::ExtractedWakeCandidate, String>,
        >,
    >,
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_wake_extraction_attempted: bool,
    #[cfg(target_os = "windows")]
    local_confirmation_task:
        Option<tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>>>,
    /// Wall clock of the in-flight secondary confirmation. An exploratory
    /// confirmation that already spent the full budget before KWS hit must not
    /// be charged a second post-hit grace period.
    #[cfg(target_os = "windows")]
    local_confirmation_task_started_at: Option<Instant>,
    #[cfg(target_os = "windows")]
    local_confirmation_window_origin_bytes: usize,
    #[cfg(target_os = "windows")]
    local_confirmation_task_origin_bytes: usize,
    #[cfg(target_os = "windows")]
    local_confirmation_task_has_keyword_model_hit: bool,
    #[cfg(target_os = "windows")]
    local_confirmation_prefix_retry: LocalConfirmationPrefixRetryState,
    /// 2026-09-22 跟手③:音近证据加密重试状态(见 dictation_wake_prefix_retry.rs)。
    #[cfg(target_os = "windows")]
    local_near_retry: LocalNearRetryState,
    local_confirmation_attempts: usize,
    local_confirmation_last_snapshot_bytes: usize,
    /// First KWS hit schedules an immediate local confirm instead of waiting for
    /// the next 5s/8s ladder rung (owner saw ~5–7s wake delay before accept).
    kws_prompted_local_confirm: bool,
    /// Local ASR returned Absent while KWS still hot (telemetry / retry pacing).
    kws_local_absent_count: u8,
    /// All completed midstream local Absent results. Terminal handling uses
    /// repeated evidence to skip an expensive ambient-only offline cascade.
    local_absent_count: u8,
    /// Local evidence that may fuse only with an independent KWS hit: either a
    /// complete phrase later in the window or a redacted zero/one-unit phonetic
    /// near-match. It never wakes by itself, but keeps the bounded terminal KWS
    /// cascade available after older-window Absents.
    local_kws_fusion_evidence: bool,
    /// Host KWS has produced a phrase candidate. This is still only a
    /// candidate: the separated track must pass the enrolled owner verifier.
    kws_phrase_detected: bool,
    /// Overlap-degraded local ASR can preserve only the start-aligned first
    /// half of 「开始录音」 while the enrolled owner still verifies. Count only
    /// repeated start-aligned confirmations; terminal policy may combine three
    /// of them with the independent enrolled voiceprint. A rolling window also
    /// has one bounded owner-gated near-match path with body text.
    local_owner_overlap_near_confirmations: u8,
    /// Near-phrase observations retained for terminal recovery. Rolling ASR
    /// windows may overlap, so these confirmations are correlated. A two-unit
    /// phonetic neighbour counts only at the start of a search window.
    owner_near_phrase_confirmations: u8,
    /// Terminal local ASR heard real speech but the phrase was fully masked
    /// (distance >= 3) — the extraction recovery exists for exactly this
    /// shape, but nothing started it because both phrase and owner evidence
    /// were masked too. Consumed only with an owner voiceprint floor.
    #[cfg(target_os = "windows")]
    local_owner_masked_phrase_evidence: bool,
    /// Interference calibration is scoped to this candidate. A process-global
    /// baseline let a previous room/foreign-speaker candidate bias later wake
    /// decisions, making sensitivity intermittent across recordings.
    wake_interference_baseline: WakeInterferenceBaseline,
    /// Absolute PCM interval inspected by the newest non-stale local Absent.
    /// A KWS fallback may not override it when it already covered that hit.
    #[cfg(target_os = "windows")]
    local_absent_coverage: Option<LocalConfirmationCoverage>,
    /// Wall clock of first live KWS hit, retained for latency diagnostics.
    /// Elapsed confirmation work is not positive phrase evidence.
    kws_first_hit_at: Option<Instant>,
    /// Candidate PCM length (ms) at first live KWS hit — gates when an Absent
    /// may count toward hard-reject (phrase must have had time to finish).
    kws_first_hit_pcm_ms: Option<usize>,
    /// Early Recording capsule while the remaining automatic-wake gates run.
    early_capsule_session_id: Option<SessionId>,
    /// Candidate elapsed time at the first real Recording publish.
    early_capsule_request_ms: Option<u64>,
    kws_fed_bytes: usize,
    /// Absolute PCM offset represented by second 0 of the current detector.
    /// Long ambient candidates rotate the detector with overlap so later wake
    /// phrases do not inherit several seconds of unrelated recurrent state.
    kws_stream_origin_bytes: usize,
    kws_total_ms: u64,
    started_at: Instant,
}

fn candidate_slice_range(
    base: Option<u64>,
    start: usize,
    bytes: usize,
) -> Option<crate::observability::CandidateRange> {
    let start = u64::try_from(start)
        .ok()
        .and_then(|offset| base?.checked_add(offset))?;
    let range = crate::observability::PcmRange::from_start_and_bytes(start, bytes)?;
    Some(crate::observability::CandidateRange {
        start: range.start,
        end: range.end,
    })
}

fn slice_candidate_range(
    range: Option<crate::observability::CandidateRange>,
    offset: usize,
    bytes: usize,
) -> Option<crate::observability::CandidateRange> {
    let range = range?;
    let sliced = slice_pcm_range(
        Some(crate::observability::PcmRange {
            start: range.start,
            end: range.end,
        }),
        offset,
        bytes,
    )?;
    Some(crate::observability::CandidateRange {
        start: sliced.start,
        end: sliced.end,
    })
}

fn slice_pcm_range(
    range: Option<crate::observability::PcmRange>,
    offset: usize,
    bytes: usize,
) -> Option<crate::observability::PcmRange> {
    let range = range?;
    let offset = u64::try_from(offset).ok()?;
    let bytes = u64::try_from(bytes).ok()?;
    let start = range.start.checked_add(offset)?;
    let end = start.checked_add(bytes)?;
    (end <= range.end).then_some(crate::observability::PcmRange { start, end })
}

fn slice_pcm_source_interval(
    interval: Option<crate::observability::PcmSourceInterval>,
    offset: usize,
    bytes: usize,
) -> Option<crate::observability::PcmSourceInterval> {
    let interval = interval?;
    Some(crate::observability::PcmSourceInterval {
        range: slice_pcm_range(Some(interval.range), offset, bytes)?,
        ..interval
    })
}

/// Attribute bytes released from a buffered candidate to the session only
/// when the release operation actually accepted the complete chunk and the
/// candidate's source runs form a gap-free mapping. A partial acceptance is
/// deliberately retained as possible evidence: receipt references remain
/// useful for diagnosis, but it must never become a definite body mapping.
fn record_candidate_release_source_dependencies(
    candidate: &BufferedSpeakerCandidate,
    candidate_range: Option<crate::observability::CandidateRange>,
    destination_range: Option<crate::observability::PcmRange>,
    release_source_interval: Option<crate::observability::PcmSourceInterval>,
    release_bytes: usize,
    accepted_bytes: usize,
    operation_owner: SourceAdmissionOperationOwner,
    operation_id: Option<u64>,
    ledger: &Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
) {
    if accepted_bytes == 0 {
        return;
    }

    let record_possible_reference = |context: Option<&CaptureAdmissionSourceContext>| {
        record_source_admission_dependency(
            ledger,
            context,
            operation_owner,
            SourceAdmissionUse::SessionBodyInputPossible,
            None,
            None,
            0,
            operation_id,
        );
    };
    let record_unknown_bytes = |bytes: usize| {
        if bytes > 0 {
            record_source_admission_dependency(
                ledger,
                None,
                operation_owner,
                SourceAdmissionUse::SessionBodyInputPossible,
                None,
                None,
                bytes,
                operation_id,
            );
        }
    };

    if accepted_bytes != release_bytes {
        for context in candidate.possible_source_admission_contexts(candidate_range) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    }

    let Some(candidate_range) = candidate_range else {
        for context in candidate.possible_source_admission_contexts(None) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    };
    let Some(destination_range) = destination_range else {
        for context in candidate.possible_source_admission_contexts(Some(candidate_range)) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    };

    let destination_bytes = usize::try_from(
        destination_range
            .end
            .saturating_sub(destination_range.start),
    )
    .ok();
    if destination_bytes != Some(release_bytes) {
        for context in candidate.possible_source_admission_contexts(Some(candidate_range)) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    }

    // A source run can be real but have no candidate coordinate (for example
    // after a cursor overflow). Keep its receipt as possible evidence even
    // though it cannot participate in the definite coverage sweep.
    for run in candidate
        .source_runs
        .iter()
        .filter(|run| run.candidate_range.is_none())
    {
        record_possible_reference(run.capture_admission_context.as_ref());
    }

    let mut source_runs = candidate.source_admission_contexts_for_range(Some(candidate_range));
    source_runs.retain(|(_, offset, bytes)| *bytes > 0 && *offset < release_bytes);
    source_runs.sort_by_key(|(_, offset, bytes)| (*offset, *bytes));
    source_runs.dedup_by(|right, left| {
        right.1 == left.1
            && right.2 == left.2
            && source_admission_contexts_same_identity(&right.0, &left.0)
    });

    let mut boundaries = vec![0usize, release_bytes];
    for (_, offset, bytes) in &source_runs {
        boundaries.push(*offset);
        boundaries.push(offset.saturating_add(*bytes).min(release_bytes));
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut unknown_bytes = 0usize;
    for bounds in boundaries.windows(2) {
        let [start, end] = [bounds[0], bounds[1]];
        if start >= end {
            continue;
        }
        let covering = source_runs
            .iter()
            .filter(|(_, offset, bytes)| {
                *offset <= start && start < offset.saturating_add(*bytes)
            })
            .collect::<Vec<_>>();
        if covering.len() != 1 {
            for (context, _, _) in &covering {
                record_possible_reference(context.as_ref());
            }
            unknown_bytes = unknown_bytes.saturating_add(end - start);
            continue;
        }

        let (context, _, _) = covering[0];
        let mapped_bytes = end - start;
        let session_range = slice_pcm_range(Some(destination_range), start, mapped_bytes);
        if let (Some(context), Some(session_range)) = (context.as_ref(), session_range) {
            record_source_admission_dependency(
                ledger,
                Some(context),
                operation_owner,
                SourceAdmissionUse::SessionBodyInput,
                Some(SourceAdmissionOwnerRange::Session {
                    start: session_range.start,
                    end: session_range.end,
                }),
                slice_pcm_source_interval(release_source_interval, start, mapped_bytes),
                mapped_bytes,
                operation_id,
            );
        } else {
            record_possible_reference(context.as_ref());
            unknown_bytes = unknown_bytes.saturating_add(mapped_bytes);
        }
    }
    record_unknown_bytes(unknown_bytes);
}

fn record_terminal_wake_body_source_dependencies(
    body: &TerminalWakeBody,
    candidate_range: Option<crate::observability::CandidateRange>,
    destination_range: Option<crate::observability::PcmRange>,
    release_source_interval: Option<crate::observability::PcmSourceInterval>,
    release_bytes: usize,
    accepted_bytes: usize,
    operation_owner: SourceAdmissionOperationOwner,
    operation_id: Option<u64>,
    ledger: &Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
) {
    if accepted_bytes == 0 {
        return;
    }

    let record_possible_reference = |context: Option<&CaptureAdmissionSourceContext>| {
        record_source_admission_dependency(
            ledger,
            context,
            operation_owner,
            SourceAdmissionUse::SessionBodyInputPossible,
            None,
            None,
            0,
            operation_id,
        );
    };
    let record_unknown_bytes = |bytes: usize| {
        if bytes > 0 {
            record_source_admission_dependency(
                ledger,
                None,
                operation_owner,
                SourceAdmissionUse::SessionBodyInputPossible,
                None,
                None,
                bytes,
                operation_id,
            );
        }
    };

    if accepted_bytes != release_bytes {
        for context in body.possible_source_admission_contexts(candidate_range) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    }
    let Some(candidate_range) = candidate_range else {
        for context in body.possible_source_admission_contexts(None) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    };
    let Some(destination_range) = destination_range else {
        for context in body.possible_source_admission_contexts(Some(candidate_range)) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    };

    let destination_bytes = usize::try_from(
        destination_range
            .end
            .saturating_sub(destination_range.start),
    )
    .ok();
    if destination_bytes != Some(release_bytes) {
        for context in body.possible_source_admission_contexts(Some(candidate_range)) {
            record_possible_reference(context.as_ref());
        }
        record_unknown_bytes(accepted_bytes);
        return;
    }

    for run in body
        .source_runs
        .iter()
        .filter(|run| run.candidate_range.is_none())
    {
        record_possible_reference(run.capture_admission_context.as_ref());
    }

    let mut source_runs = body.source_admission_contexts_for_range(Some(candidate_range));
    source_runs.retain(|(_, offset, bytes)| *bytes > 0 && *offset < release_bytes);
    source_runs.sort_by_key(|(_, offset, bytes)| (*offset, *bytes));
    source_runs.dedup_by(|right, left| {
        right.1 == left.1
            && right.2 == left.2
            && source_admission_contexts_same_identity(&right.0, &left.0)
    });

    let mut boundaries = vec![0usize, release_bytes];
    for (_, offset, bytes) in &source_runs {
        boundaries.push(*offset);
        boundaries.push(offset.saturating_add(*bytes).min(release_bytes));
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut unknown_bytes = 0usize;
    for bounds in boundaries.windows(2) {
        let [start, end] = [bounds[0], bounds[1]];
        if start >= end {
            continue;
        }
        let covering = source_runs
            .iter()
            .filter(|(_, offset, bytes)| {
                *offset <= start && start < offset.saturating_add(*bytes)
            })
            .collect::<Vec<_>>();
        if covering.len() != 1 {
            for (context, _, _) in &covering {
                record_possible_reference(context.as_ref());
            }
            unknown_bytes = unknown_bytes.saturating_add(end - start);
            continue;
        }

        let (context, _, _) = covering[0];
        let mapped_bytes = end - start;
        let session_range = slice_pcm_range(Some(destination_range), start, mapped_bytes);
        if let (Some(context), Some(session_range)) = (context.as_ref(), session_range) {
            record_source_admission_dependency(
                ledger,
                Some(context),
                operation_owner,
                SourceAdmissionUse::SessionBodyInput,
                Some(SourceAdmissionOwnerRange::Session {
                    start: session_range.start,
                    end: session_range.end,
                }),
                slice_pcm_source_interval(release_source_interval, start, mapped_bytes),
                mapped_bytes,
                operation_id,
            );
        } else {
            record_possible_reference(context.as_ref());
            unknown_bytes = unknown_bytes.saturating_add(mapped_bytes);
        }
    }
    record_unknown_bytes(unknown_bytes);
}

fn source_admission_contexts_same_identity(
    left: &Option<CaptureAdmissionSourceContext>,
    right: &Option<CaptureAdmissionSourceContext>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => match (&left.capture_receipt, &right.capture_receipt) {
            (Some(left), Some(right)) => left.same_reference(right),
            (None, None) => {
                left.capture_generation == right.capture_generation
                    && left.capture_fact == right.capture_fact
                    && left.actor_fact == right.actor_fact
                    && left.actor_chunk_metadata == right.actor_chunk_metadata
            }
            _ => false,
        },
        _ => false,
    }
}

fn collector_emitted_range_for_candidate_overlap(
    collector_range: Option<crate::embedded_audio::StreamingPcmRange>,
    source_range: Option<crate::observability::CandidateRange>,
    selected_range: Option<crate::observability::CandidateRange>,
) -> Option<crate::embedded_audio::StreamingPcmRange> {
    let collector_range = collector_range?;
    let (Some(source_range), Some(selected_range)) = (source_range, selected_range) else {
        return selected_range.is_none().then_some(collector_range);
    };
    let offset = selected_range.start.checked_sub(source_range.start)?;
    let bytes = selected_range.end.checked_sub(selected_range.start)?;
    let start = collector_range.start.checked_add(offset)?;
    let end = start.checked_add(bytes)?;
    (end <= collector_range.end).then_some(crate::embedded_audio::StreamingPcmRange {
        start,
        end,
    })
}

impl BufferedSpeakerCandidate {
    fn next_source_admission_operation_id(&mut self) -> Option<u64> {
        let operation_id = self.source_admission_operation_id;
        if let Some(current) = operation_id {
            self.source_admission_operation_id = current.checked_add(1);
            if self.source_admission_operation_id.is_none() {
                self.source_admission_ledger
                    .lock()
                    .expect("source admission dependency ledger lock")
                    .mark_incomplete();
            }
        } else {
            self.source_admission_ledger
                .lock()
                .expect("source admission dependency ledger lock")
                .mark_incomplete();
        }
        operation_id
    }

    fn next_operation_id(&mut self) -> Option<u64> {
        let operation_id = self.next_operation_id;
        if let Some(current) = operation_id {
            self.next_operation_id = current.checked_add(1);
            if self.next_operation_id.is_none() {
                self.fact_ledger.mark_incomplete();
            }
        } else {
            self.fact_ledger.mark_incomplete();
        }
        operation_id
    }

    fn source_facts_for_range(
        &self,
        range: Option<crate::observability::CandidateRange>,
    ) -> Vec<crate::observability::CandidateSourceRunFact> {
        self.source_runs
            .iter()
            .filter_map(|run| {
                if range.is_none() {
                    return Some(crate::observability::CandidateSourceRunFact {
                        capture_generation: run.capture_generation,
                        segment_id: run.segment_id,
                        candidate_range: None,
                        collector_metadata: run.collector_metadata,
                        collector_emitted_range: run.collector_emitted_range,
                        bytes: 0,
                    });
                }
                let candidate_range = match (range, run.candidate_range) {
                    (Some(requested), Some(source)) => {
                        let start = requested.start.max(source.start);
                        let end = requested.end.min(source.end);
                        (start < end).then_some(crate::observability::CandidateRange { start, end })
                    }
                    (None, _) => run.candidate_range,
                    _ => None,
                };
                if range.is_some()
                    && run.candidate_range.is_some()
                    && candidate_range.is_none()
                {
                    return None;
                }
                let bytes = candidate_range
                    .map(|item| item.end.saturating_sub(item.start))
                    .unwrap_or(run.bytes as u64);
                let collector_emitted_range = collector_emitted_range_for_candidate_overlap(
                    run.collector_emitted_range,
                    run.candidate_range,
                    candidate_range,
                );
                Some(crate::observability::CandidateSourceRunFact {
                    capture_generation: run.capture_generation,
                    segment_id: run.segment_id,
                    candidate_range,
                    collector_metadata: run.collector_metadata,
                    collector_emitted_range,
                    bytes,
                })
            })
            .collect()
    }

    fn source_admission_contexts_for_range(
        &self,
        range: Option<crate::observability::CandidateRange>,
    ) -> Vec<(Option<CaptureAdmissionSourceContext>, usize, usize)> {
        let Some(range) = range else {
            return Vec::new();
        };
        self.source_runs
            .iter()
            .filter_map(|run| {
                let source = run.candidate_range?;
                let start = range.start.max(source.start);
                let end = range.end.min(source.end);
                if start >= end {
                    return None;
                }
                let offset = usize::try_from(start.checked_sub(range.start)?).ok()?;
                let bytes = usize::try_from(end.checked_sub(start)?).ok()?;
                Some((run.capture_admission_context.clone(), offset, bytes))
            })
            .collect()
    }

    fn possible_source_admission_contexts(
        &self,
        range: Option<crate::observability::CandidateRange>,
    ) -> Vec<Option<CaptureAdmissionSourceContext>> {
        self.source_runs
            .iter()
            .filter_map(|run| {
                if let (Some(requested), Some(source)) = (range, run.candidate_range) {
                    let overlaps = requested.start < source.end && source.start < requested.end;
                    if !overlaps {
                        return None;
                    }
                }
                Some(run.capture_admission_context.clone())
            })
            .collect()
    }

    fn record_fact(
        &mut self,
        observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        kind: crate::observability::CandidateFactKind,
        candidate_range: Option<crate::observability::CandidateRange>,
        kws_feed_range: Option<crate::observability::CandidateRange>,
        release_destination_range: Option<crate::observability::PcmRange>,
        release_coordinator_session_id: Option<String>,
        release_source_stream_id: Option<u64>,
        source_runs: Vec<crate::observability::CandidateSourceRunFact>,
        bytes: usize,
        reason: Option<&str>,
    ) {
        let operation_id = self.next_operation_id();
        self.record_fact_with_operation_id(
            observation,
            operation_id,
            kind,
            candidate_range,
            kws_feed_range,
            release_destination_range,
            release_coordinator_session_id,
            release_source_stream_id,
            source_runs,
            bytes,
            reason,
        );
    }

    fn record_fact_with_operation_id(
        &mut self,
        observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        operation_id: Option<u64>,
        kind: crate::observability::CandidateFactKind,
        candidate_range: Option<crate::observability::CandidateRange>,
        kws_feed_range: Option<crate::observability::CandidateRange>,
        release_destination_range: Option<crate::observability::PcmRange>,
        release_coordinator_session_id: Option<String>,
        release_source_stream_id: Option<u64>,
        source_runs: Vec<crate::observability::CandidateSourceRunFact>,
        bytes: usize,
        reason: Option<&str>,
    ) {
        let fact = crate::observability::CandidateFact {
            candidate_id: self.candidate_id,
            operation_id,
            kind,
            candidate_range,
            kws_feed_range,
            release_destination_range,
            release_coordinator_session_id,
            release_source_stream_id,
            source_runs,
            bytes: bytes as u64,
            reason: reason.map(str::to_string),
        };
        self.fact_ledger.record(fact.clone());
        if let Some(observation) = observation {
            observation.record_candidate_fact(fact);
        }
    }

    fn current_candidate_range(&self) -> Option<crate::observability::CandidateRange> {
        candidate_slice_range(self.pcm_base_offset, 0, self.pcm.len())
    }

    fn terminal_wake_body(&self, post_wake_offset: usize) -> TerminalWakeBody {
        // The candidate is always 16-bit PCM. Keep the hand-off boundary
        // sample-aligned even if a future detector returns a byte-odd offset.
        let body_offset = post_wake_offset.min(self.pcm.len()) & !1usize;
        let body_pcm = self.pcm[body_offset..].to_vec();
        TerminalWakeBody {
            candidate_id: self.candidate_id,
            pcm: body_pcm.clone(),
            candidate_range: candidate_slice_range(
                self.pcm_base_offset,
                body_offset,
                body_pcm.len(),
            ),
            source_runs: self.source_runs.clone(),
            source_admission_ledger: Arc::clone(&self.source_admission_ledger),
        }
    }

    fn record_outcome_fact(
        &mut self,
        observation: Option<&Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        kind: crate::observability::CandidateFactKind,
        reason: &'static str,
        bytes: usize,
    ) {
        let candidate_range = self.current_candidate_range();
        let source_runs = self.source_facts_for_range(candidate_range);
        self.record_fact(
            observation,
            kind,
            candidate_range,
            None,
            None,
            None,
            None,
            source_runs,
            bytes,
            Some(reason),
        );
    }
}

struct PendingAutomaticPhraseMatch {
    wake_match: crate::wake_phrase::Match,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    local_confirmation_ms: u64,
    owner_verification_start_ms: usize,
    owner_verified_by_extraction: bool,
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
struct ExtractedOwnerWakeEvidence {
    wake_match: crate::wake_phrase::Match,
    local_confirmation_ms: u64,
    extraction_ms: u64,
    owner_score: f32,
    owner_verified_by_extraction: bool,
    residual_ratio: f64,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalConfirmationCoverage {
    start_bytes: usize,
    end_bytes: usize,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct LocalWakeConfirmation {
    matched: bool,
    phrase_relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    phonetic_prefix_units: usize,
    phonetic_best_distance: usize,
    phonetic_best_window_start: usize,
    inference_ms: u64,
    snapshot_pcm_ms: usize,
    recovered_keyword_end_seconds: Option<f32>,
}

#[cfg(all(target_os = "windows", test))]
static LAST_LOCAL_WAKE_DIAGNOSTIC_RESULT:
    std::sync::OnceLock<std::sync::Mutex<Option<(String, Option<String>)>>> =
    std::sync::OnceLock::new();

#[cfg(all(target_os = "windows", test))]
fn take_last_local_wake_diagnostic_result() -> Option<(String, Option<String>)> {
    LAST_LOCAL_WAKE_DIAGNOSTIC_RESULT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("local wake diagnostic result lock")
        .take()
}

const MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES: usize = 2_100_000;
const STREAMING_KWS_FEED_BATCH_BYTES: usize = 1_600;
const STREAMING_KWS_ROTATE_AFTER_MS: usize = 2_400;
const STREAMING_KWS_ROTATE_AFTER_BYTES: usize = STREAMING_KWS_ROTATE_AFTER_MS * 32;
const STREAMING_KWS_ROTATE_OVERLAP_MS: usize = 1_400;
const STREAMING_KWS_ROTATE_OVERLAP_BYTES: usize = STREAMING_KWS_ROTATE_OVERLAP_MS * 32;
/// Healthy sessions have exactly one endpoint authority: the owner activity
/// controller driven by speaker evidence. A raw-energy proactive stop cannot
/// distinguish a thinking pause from an utterance boundary, so keep this path
/// permanently dormant. The field remains for telemetry/backward-compatible
/// session state; only the provider-failure safety path below may dispatch it.
const EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS: u64 = u64::MAX;
// A failed provider cannot emit the normal content-aware endpoint. Keep a
// separate local safety bound for that failure mode only; healthy sessions
// continue to use the disabled-by-design 30s guard.
const EMBEDDED_ASR_FAILURE_PROACTIVE_STOP_SILENCE_MS: u64 = 1_200;

fn proactive_stop_silence_threshold_ms(asr_delivery_failed: bool) -> u64 {
    if asr_delivery_failed {
        EMBEDDED_ASR_FAILURE_PROACTIVE_STOP_SILENCE_MS
    } else {
        EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS
    }
}
// 2026-08-09 12:46:59 激活竞态：ACTIVATE 与旧唤醒段 complete 相隔 0.1s。旧段
// STOP 落在该窗口内且正文未开始时，视为段 rotation 而非用户说完，不 finalize。
const EMBEDDED_ACTIVATION_SEGMENT_RACE_WINDOW: Duration = Duration::from_millis(2_000);
const OWNER_VERIFICATION_START_MS: usize = 1_100;
const OWNER_VERIFICATION_START_BYTES: usize = OWNER_VERIFICATION_START_MS * 32;
const OWNER_VERIFICATION_SNAPSHOT_MS: [usize; 3] = [OWNER_VERIFICATION_START_MS, 1_800, 2_400];
// Real accepted captures complete the four-character Mandarin phrase at 0.8 s
// (but not 0.7 s). Run the exact-start local confirmation speculatively here;
// incomplete/absent results stay eligible for KWS and later ladder retries.
const LOCAL_CONFIRMATION_START_MS: usize = 800;
const LOCAL_CONFIRMATION_START_BYTES: usize = LOCAL_CONFIRMATION_START_MS * 32;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
const TARGET_WAKE_EXTRACTION_START_MS: usize = 2_400;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
const TARGET_WAKE_EXTRACTION_START_BYTES: usize = TARGET_WAKE_EXTRACTION_START_MS * 32;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
const TARGET_WAKE_EXTRACTION_PREFETCHED_WAIT_MS: u64 = 2_200;
// A separator started from terminal owner evidence has no pre-terminal head
// start. Installed traces put extraction alone at 2.1-2.5 s, so applying the
// prefetched budget killed the recovery before its phrase/owner gates ran.
// This larger budget is used only after the ordinary wake path has failed.
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
const TARGET_WAKE_EXTRACTION_LAZY_TERMINAL_WAIT_MS: u64 = 3_200;
/* A negotiated firmware pre-roll burst can deliver several seconds of already
 * captured audio in under one second. Starting exploratory local ASR while
 * that backlog drains steals the single-flight helper from the KWS burst
 * consumer. But 2.4 s was too late: 2026-09-22 21:2x-21:5x 实测 (用户"胶囊
 * 还是慢", kws_hit=false 的唤醒走探索路径) 第一拍全被推迟到 2.4s PCM——
 * 词 1.5-2.3s 说完时 stage2 还没看过一眼;且第一拍之后各档本就背靠背连发
 * (间隔≈推理耗时), 档距不是瓶颈, 开场这一刀才是。1.2s PCM ≈ 2x 回放下
 * 0.6s 墙钟, 只占一拍 26-440ms 推理, 与 KWS 撞车的代价(≤200ms 排队)远小
 * 于探索路径整场晚 1.2s。Real-time/raw transport never meets the >2x
 * condition and keeps the bounded 0.8/1.8/2.0 s ladder unchanged. */
const FAST_PREROLL_LOCAL_CONFIRM_DEFER_UNTIL_MS: usize = 1_200;
// The isolated Paraformer helper is deliberately single-flight. Installed
// sessions 569/578/583 showed that the old 1.4 s exploratory pass occupied it
// for 168-186 ms while the complete phrase arrived, pushing the useful 1.6 s
// pass out to 1.7-1.84 s PCM and the capsule to ~1.4 s wall time. Across the
// installed log the 1.4 s rung ran 30 times and its only match had already
// overshot to 3.633 s PCM. Keep the 0.8 s fast-speech check, then wait for a
// realistic complete-phrase window at 1.8 s. A strong start prefix gets the
// existing 140 ms new-audio retry, so slow speech does not wait for 2.0 s.
const LOCAL_CONFIRMATION_SNAPSHOT_MS: [usize; 6] = [
    LOCAL_CONFIRMATION_START_MS,
    1_800,
    2_000,
    2_400,
    3_000,
    5_000,
];
/// Once KWS already heard the phrase, do not wait for the 1.8s ladder floor.
/// ~0.7s covers a full "开始录音" on real captures (0.8s was leaving ~100ms of
/// dead air on the KeywordModel fail-open path → phrase_tail ~500ms spikes).
const KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS: usize = 700;
const KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_BYTES: usize = KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS * 32;
/// After a failed immediate confirm, re-try every 400ms of new audio while KWS
/// stays hot — avoids sitting on the 2.4/3.0/5.0s ladder rungs.
const KWS_LOCAL_CONFIRM_RETRY_MS: usize = 400;
const KWS_LOCAL_CONFIRM_RETRY_BYTES: usize = KWS_LOCAL_CONFIRM_RETRY_MS * 32;
/// Streaming KWS already supplies an absolute phrase boundary. Stage-2 only
/// needs nearby speech for precision; sending a long ambient prefix makes
/// Paraformer latency scale with unrelated audio. Cap remains 5_000 ms
/// (LST-WAKE-009); a shorter focus tail is tried on Absent before counting
/// hard-reject evidence.
const KWS_LOCAL_CONFIRM_MAX_PCM_MS: usize = 5_000;
const KWS_LOCAL_CONFIRM_MAX_PCM_BYTES: usize = KWS_LOCAL_CONFIRM_MAX_PCM_MS * 32;
/// Phrase-focused retry after a 5 s tail Absent. Real quiet miss session 288
/// ends the wake near 3.1 s inside a ~3.9 s candidate; a 1.6 s tail isolates
/// the phrase better than ambient-polluted full-candidate Paraformer ASR.
const KWS_LOCAL_CONFIRM_FOCUS_PCM_MS: usize = 1_600;
const KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES: usize = KWS_LOCAL_CONFIRM_FOCUS_PCM_MS * 32;
/// Do not spend the two-Absent hard-reject budget until the candidate has
/// continued ~one full Mandarin wake phrase after the first KWS hit. Session
/// 288 evidence: streaming KWS at 1.92 s with offline full-phrase end at
/// 3.1 s — two early Absents killed the candidate before the phrase finished.
const KWS_ABSENT_COUNT_MIN_POST_HIT_MS: usize = 1_000;
/// XiaoAi-style cascade after sensitive KWS hit:
///   stage-1 KWS (high recall) → stage-2 local wake verifier (precision)
/// Wait only inside the accepted phrase-tail budget for stage-2; then fail-open
/// as KeywordModel so an intermittently slow helper never makes wake feel
/// unresponsive. Explicit Absent still rejects. Installed session 66 spent
/// 297 ms here after KWS had already supplied the phrase and pushed capsule
/// latency to 1,306 ms. Fixed-matrix evidence also showed that a new ~250 ms
/// confirmation cannot finish inside the old 100 ms grace; waiting the full
/// interval only moved common 1.91 s KWS hits to 2.01 s. Keep a 60 ms grace so
/// actor polling plus recording-control/capsule dispatch remain inside both the
/// phrase-tail and one-second start-to-capsule targets. Any already-completed
/// explicit Absent remains authoritative.
const KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 60;
/// Explicit local Absent count before midstream hard-reject (blocks short
/// prefix false wakes like "开始啥的"; one retry for noisy short clips).
const KWS_SECONDARY_ABSENT_REJECT_COUNT: u8 = 2;
/// KWS and local ASR do not place the phrase tail on exactly the same frame.
/// Sessions 643/862: local ASR had already produced an authoritative non-match,
/// while KWS later estimated the same false keyword tail 201/280 ms farther
/// into the stream. Treat that small boundary disagreement as coverage so a
/// 60 ms secondary timeout cannot reverse explicit contradictory evidence.
/// Keep this below the duration needed for the four-syllable wake phrase so an
/// older unrelated Absent cannot veto a genuinely later phrase.
const LOCAL_ABSENT_KEYWORD_TAIL_SLACK_MS: usize = 300;
const LOCAL_ABSENT_KEYWORD_TAIL_SLACK_BYTES: usize = LOCAL_ABSENT_KEYWORD_TAIL_SLACK_MS * 32;
/// Ordinary room speech can keep firmware VA sessions open for ~4.5 s. Limit
/// the initial candidate to the 0.8/1.8/2.0 s ladder, then allow exactly
/// one focused confirmation in every later rolling window. Real device captures
/// carry ~1 s pre-roll, so an older candidate-wide Absent cap permanently
/// disabled recognition after the first few windows. Per-window work stays
/// bounded, and a later KWS hit still receives its full stage-2 confirmation.
#[allow(dead_code)]
const LOCAL_ONLY_EXPLORATORY_ABSENT_LIMIT: u8 = 4;
/// After the initial ladder plus one focused retry, repeated explicit Absent is
/// authoritative enough to skip the expensive terminal recall cascade. A
/// timeout around spawn_blocking releases the BLE actor but cannot cancel the
/// native KWS work, so running it for every ambient candidate causes seconds of
/// hidden CPU contention and visible WebView/capsule stalls.
const TERMINAL_OFFLINE_SKIP_ABSENT_COUNT: u8 = 4;
const PHONETIC_NEAR_MAX_DISTANCE: usize = 1;
const TERMINAL_INFLIGHT_CONFIRM_BUDGET_MS: u64 = 1_200;
const MIN_TERMINAL_OFFLINE_PCM_BYTES: usize = 16_000 * 2 * 2;
const TERMINAL_OFFLINE_RECALL_BUDGET_MS: u64 = 500;
const WAKE_END_PAD_SECONDS: f32 = 0.12;
const LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS: f32 = 1.20;

fn rolling_kws_rotation_start(
    total_pcm_bytes: usize,
    stream_origin_bytes: usize,
    phrase_already_hit: bool,
) -> Option<usize> {
    (!phrase_already_hit
        && total_pcm_bytes.saturating_sub(stream_origin_bytes) >= STREAMING_KWS_ROTATE_AFTER_BYTES)
        .then(|| total_pcm_bytes.saturating_sub(STREAMING_KWS_ROTATE_OVERLAP_BYTES) & !1usize)
}

#[cfg(target_os = "windows")]
fn should_advance_local_confirmation_window(
    rotated: bool,
    keyword_model_hit: bool,
    current_origin_bytes: usize,
    next_origin_bytes: usize,
    local_confirmation_attempts: usize,
    local_confirmation_in_flight: bool,
) -> bool {
    let _ = (
        rotated,
        keyword_model_hit,
        current_origin_bytes,
        next_origin_bytes,
        local_confirmation_attempts,
        local_confirmation_in_flight,
    );
    // KWS may roll its stream; local confirmation must not follow. Fast BLE
    // pre-roll can burn the 0.8 s and 1.8 s rungs in one wall-clock second and
    // then rotate origin to ~1 s, cutting 开始录音 out of the window. Keep the
    // candidate head until Accept or terminal reject.
    false
}

const FIRMWARE_PREROLL_QUIET_SKIP_MAX_MS: usize = 400;
const FIRMWARE_PREROLL_QUIET_SKIP_MAX_BYTES: usize = FIRMWARE_PREROLL_QUIET_SKIP_MAX_MS * 32;
const FIRMWARE_PREROLL_QUIET_PEAK: u16 = 256;

fn leading_quiet_prefix_bytes(pcm: &[u8]) -> usize {
    let limit = FIRMWARE_PREROLL_QUIET_SKIP_MAX_BYTES.min(pcm.len()) & !1usize;
    let mut offset = 0usize;
    while offset + 2 <= limit {
        let sample = i16::from_le_bytes([pcm[offset], pcm[offset + 1]]).unsigned_abs();
        if sample > FIRMWARE_PREROLL_QUIET_PEAK {
            return offset & !1usize;
        }
        offset += 2;
    }
    limit
}

#[cfg(target_os = "windows")]
fn local_confirmation_snapshot_for_window(
    total_pcm_bytes: usize,
    window_origin_bytes: usize,
    attempts: usize,
) -> Option<usize> {
    let window_pcm_bytes = total_pcm_bytes.saturating_sub(window_origin_bytes);
    next_local_confirmation_snapshot_bytes(attempts)
        .filter(|snapshot_bytes| window_pcm_bytes >= *snapshot_bytes)
}

#[cfg(target_os = "windows")]
fn exploratory_local_confirmation_allowed(
    keyword_model_hit: bool,
    _absent_count: u8,
    window_origin_bytes: usize,
    window_attempts: usize,
) -> bool {
    if keyword_model_hit {
        return true;
    }
    if window_origin_bytes == 0 {
        // Live 2293891870: four Absents on 0.8–2.4 s snapshots disabled the
        // 3.0 s and 5.0 s rungs, then terminal ASR never saw the phrase.
        // Keep-wake: early Absents must not abort the rest of the ladder.
        return true;
    }
    window_attempts == 0
}

#[cfg(target_os = "windows")]
fn local_confirmation_task_is_stale(
    task_origin_bytes: usize,
    current_origin_bytes: usize,
    task_has_keyword_model_hit: bool,
) -> bool {
    !task_has_keyword_model_hit && task_origin_bytes < current_origin_bytes
}
#[cfg(target_os = "windows")]
fn stale_local_confirmation_can_activate(
    stale: bool,
    result: &LocalWakeConfirmation,
    task_has_keyword_model_hit: bool,
) -> bool {
    // Rotation discards ambiguous evidence, not a completed activation-grade positive.
    stale
        && result.matched
        && local_confirmation_can_activate(task_has_keyword_model_hit, result.phrase_relation)
}
fn offset_streaming_wake_match(
    found: Option<crate::wake_phrase::Match>,
    stream_origin_bytes: usize,
) -> Option<crate::wake_phrase::Match> {
    found.map(|mut found| {
        let origin_seconds = stream_origin_bytes as f32 / 32_000.0;
        found.start_seconds = found.start_seconds.map(|start| start + origin_seconds);
        found.end_seconds += origin_seconds;
        found
    })
}

fn owner_verification_window_ready(pcm_bytes: usize, enrolled: bool) -> bool {
    // No enrolled voiceprint → phrase hit alone is enough; do not stall for the
    // 1.1s owner speech window (that delay only exists for embedding quality).
    if !enrolled {
        return true;
    }
    pcm_bytes >= OWNER_VERIFICATION_START_BYTES
}

fn next_owner_verification_retry_ms(pcm_ms: usize) -> Option<usize> {
    OWNER_VERIFICATION_SNAPSHOT_MS
        .iter()
        .copied()
        .find(|snapshot_ms| *snapshot_ms > pcm_ms)
}

fn next_local_confirmation_snapshot_bytes(attempts: usize) -> Option<usize> {
    denzic_voice_activation_v1_core::confirmation_snapshot_ms(
        attempts,
        &LOCAL_CONFIRMATION_SNAPSHOT_MS,
    )
    .map(|milliseconds| milliseconds * 32)
}

fn local_confirmation_pcm(pcm: &[u8], has_keyword_model_hit: bool) -> Vec<u8> {
    tail_pcm_window(
        pcm,
        if has_keyword_model_hit {
            KWS_LOCAL_CONFIRM_MAX_PCM_BYTES
        } else {
            pcm.len()
        },
    )
}

fn tail_pcm_window(pcm: &[u8], max_bytes: usize) -> Vec<u8> {
    if max_bytes == 0 || pcm.len() <= max_bytes {
        return pcm.to_vec();
    }
    let start = pcm.len() - max_bytes;
    let aligned_start = start + start % 2;
    pcm[aligned_start..].to_vec()
}

fn kws_phrase_focus_pcm(pcm: &[u8]) -> Vec<u8> {
    tail_pcm_window(pcm, KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES)
}

/// An explicit KWS-path Absent only hard-rejects after the candidate has grown
/// by about one full phrase past the first hit. Earlier Absents still retry.
fn kws_absent_counts_toward_reject(
    first_hit_pcm_ms: Option<usize>,
    snapshot_pcm_ms: usize,
) -> bool {
    match first_hit_pcm_ms {
        None => true,
        Some(hit_ms) => snapshot_pcm_ms >= hit_ms.saturating_add(KWS_ABSENT_COUNT_MIN_POST_HIT_MS),
    }
}

#[cfg(target_os = "windows")]
fn phonetic_near_phrase_evidence(
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
) -> bool {
    !confirmation.matched
        && confirmation.phonetic_best_distance <= PHONETIC_NEAR_MAX_DISTANCE
        && confirmation.transcript_chars >= phrase_chars.saturating_sub(1).max(1)
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalInflightLocalDecision {
    AcceptLocal,
    PreserveKwsFusion,
    RecordAbsent,
}

#[cfg(target_os = "windows")]
fn terminal_inflight_local_decision(
    confirmation: &LocalWakeConfirmation,
    task_has_keyword_model_hit: bool,
    phrase_chars: usize,
) -> TerminalInflightLocalDecision {
    if confirmation.matched
        && local_confirmation_can_activate(task_has_keyword_model_hit, confirmation.phrase_relation)
    {
        TerminalInflightLocalDecision::AcceptLocal
    } else if confirmation.matched || phonetic_near_phrase_evidence(confirmation, phrase_chars) {
        TerminalInflightLocalDecision::PreserveKwsFusion
    } else {
        TerminalInflightLocalDecision::RecordAbsent
    }
}

const TERMINAL_LOCAL_TAIL_MS: usize = 2_500;
const TERMINAL_LOCAL_TAIL_BYTES: usize = TERMINAL_LOCAL_TAIL_MS * 32;

fn terminal_local_result_can_activate(result: &LocalWakeConfirmation) -> bool {
    local_confirmation_can_activate(false, result.phrase_relation)
        || result.phrase_relation == crate::wake_phrase::LocalPhraseRelation::PresentLater
}

fn terminal_local_confirmation_pcm(pcm: &[u8]) -> (Vec<u8>, usize) {
    terminal_local_confirmation_windows(pcm)
        .into_iter()
        .next()
        .unwrap_or_else(|| (pcm.to_vec(), 0))
}

fn terminal_local_confirmation_windows(pcm: &[u8]) -> Vec<(Vec<u8>, usize)> {
    if pcm.len() <= TERMINAL_LOCAL_TAIL_BYTES {
        return vec![(pcm.to_vec(), 0)];
    }
    let tail_start = (pcm.len() - TERMINAL_LOCAL_TAIL_BYTES) & !1usize;
    let head_len = TERMINAL_LOCAL_TAIL_BYTES.min(pcm.len()) & !1usize;
    let mut windows = vec![(pcm[tail_start..].to_vec(), tail_start)];
    if tail_start >= head_len {
        windows.push((pcm[..head_len].to_vec(), 0));
    }
    // Cover the middle gap between head and tail (~0.8 s on a 5.8 s clip).
    windows.push((pcm.to_vec(), 0));
    windows
}

fn should_run_terminal_offline_recall(
    pcm_bytes: usize,
    _local_absent_count: u8,
    _local_kws_fusion_evidence: bool,
    _enrolled_owner_matched: bool,
) -> bool {
    // Live 2026-09-10 second-wake miss (0744): four Absents on 0.8–2.4 s
    // pre-roll snapshots skipped the full-buffer confirm. The next candidate
    // (0745) ExactStart at 5 s. Early Absents must not veto last-chance
    // local ASR on the complete hidden VA window.
    pcm_bytes >= MIN_TERMINAL_OFFLINE_PCM_BYTES
}

fn terminal_inflight_confirmation_remaining_ms(_elapsed_ms: u64) -> u64 {
    // Live 2150: the 5s ladder confirm had already run 283ms when STOP
    // arrived. A 250ms budget charged from task start left remaining=0, so
    // ExactStart never landed; replacement windows then transcribed 0–2 chars.
    // Last-chance wait is from now, not from when the rung started.
    TERMINAL_INFLIGHT_CONFIRM_BUDGET_MS
}

/// Bytes of candidate PCM to discard before ASR for an automatic wake accept.
/// Uses KWS/local end time when available; LocalTranscript must not force 0 —
/// that shipped pre-wake speech ("好贵啊…开始录音，帮我看…") into the capsule.
fn post_wake_pcm_offset_bytes(wake_end_seconds: f32, pcm_len: usize) -> usize {
    denzic_voice_activation_v1_core::pcm_offset_after_activation(
        wake_end_seconds,
        WAKE_END_PAD_SECONDS,
        32_000,
        pcm_len,
        2,
    )
}

const WAKE_SPEAKER_ANCHOR_MS: usize = 800;
const KEYWORD_SEGMENT_LEAD_PAD_MS: usize = 120;

fn wake_speaker_anchor_pcm_offset_bytes(
    wake_start_seconds: Option<f32>,
    wake_end_seconds: f32,
    pcm_len: usize,
) -> usize {
    let wake_end_bytes =
        ((wake_end_seconds.max(0.0) * 32_000.0).round() as usize).min(pcm_len) & !1usize;
    let fallback = wake_end_bytes.saturating_sub(WAKE_SPEAKER_ANCHOR_MS * 32) & !1usize;
    let Some(wake_start_seconds) = wake_start_seconds else {
        return fallback;
    };
    if !wake_start_seconds.is_finite()
        || wake_start_seconds < 0.0
        || wake_start_seconds > wake_end_seconds
    {
        return fallback;
    }
    let keyword_start_bytes =
        ((wake_start_seconds * 32_000.0).round() as usize).min(wake_end_bytes) & !1usize;
    keyword_start_bytes.saturating_sub(KEYWORD_SEGMENT_LEAD_PAD_MS * 32) & !1usize
}

fn wake_phrase_tail_to_capsule_ms(wake_end_seconds: f32, capsule_request_ms: u64) -> u64 {
    if !wake_end_seconds.is_finite() || wake_end_seconds <= 0.0 {
        return capsule_request_ms;
    }
    let wake_end_ms = (wake_end_seconds * 1_000.0).round().max(0.0) as u64;
    capsule_request_ms.saturating_sub(wake_end_ms)
}

#[cfg(target_os = "windows")]
fn refined_wake_end_seconds(
    keyword_end_seconds: f32,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
) -> f32 {
    denzic_voice_activation_v1_core::refined_local_wake_end_seconds(
        denzic_voice_activation_v1_core::LocalConfirmationBoundaryInput {
            keyword_end_seconds,
            recovered_keyword_end_seconds: confirmation.recovered_keyword_end_seconds,
            phrase_relation: confirmation.phrase_relation,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
            snapshot_pcm_ms: confirmation.snapshot_pcm_ms,
            end_pad_seconds: WAKE_END_PAD_SECONDS,
            local_endpoint_max_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
        },
    )
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingSecondaryDecision {
    AwaitSecondary,
    HoldAfterExplicitAbsent,
}

#[cfg(target_os = "windows")]
fn pending_secondary_decision(
    keyword_model_hit: bool,
    waited_ms: u64,
    explicit_absent_count: u8,
) -> PendingSecondaryDecision {
    if !keyword_model_hit || waited_ms < KWS_SECONDARY_CONFIRM_BUDGET_MS {
        return PendingSecondaryDecision::AwaitSecondary;
    }
    if explicit_absent_count > 0 {
        PendingSecondaryDecision::HoldAfterExplicitAbsent
    } else {
        // A still-running task has supplied neither positive nor negative
        // phrase evidence. CPU contention and pre-hit work must not turn a
        // KWS candidate into an accepted dictation. Continue buffering; the
        // completed-confirmation and terminal paths own the actual decision.
        PendingSecondaryDecision::AwaitSecondary
    }
}

#[cfg(target_os = "windows")]
fn effective_secondary_waited_ms(kws_waited_ms: u64, confirmation_attempt_ms: u64) -> u64 {
    kws_waited_ms.max(confirmation_attempt_ms)
}

#[cfg(target_os = "windows")]
fn local_absent_covers_keyword_endpoint(
    coverage: Option<LocalConfirmationCoverage>,
    keyword_stream_origin_bytes: usize,
    keyword_end_seconds: f32,
) -> bool {
    if !keyword_end_seconds.is_finite() || keyword_end_seconds <= 0.0 {
        return false;
    }
    let keyword_end_bytes = (keyword_end_seconds * 32_000.0).round() as usize;
    coverage.is_some_and(|covered| {
        covered.start_bytes <= keyword_stream_origin_bytes
            && covered
                .end_bytes
                .saturating_add(LOCAL_ABSENT_KEYWORD_TAIL_SLACK_BYTES)
                >= keyword_end_bytes
    })
}

#[cfg(target_os = "windows")]
fn keyword_fallback_absent_count(
    candidate: &BufferedSpeakerCandidate,
    wake_match: &crate::wake_phrase::Match,
) -> u8 {
    candidate
        .kws_local_absent_count
        .max(u8::from(local_absent_covers_keyword_endpoint(
            candidate.local_absent_coverage,
            candidate.kws_stream_origin_bytes,
            wake_match.end_seconds,
        )))
}

#[cfg(target_os = "windows")]
fn completed_secondary_absent_is_authoritative(
    relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    phrase_chars: usize,
) -> bool {
    matches!(
        denzic_voice_activation_v1_core::decide_completed_secondary(
            denzic_voice_activation_v1_core::CompletedSecondaryInput {
                relation,
                transcript_chars,
                phrase_chars,
            },
        ),
        denzic_voice_activation_v1_core::CompletedSecondaryDecision::RejectExplicitAbsent
    )
}

#[cfg(target_os = "windows")]
fn authoritative_local_absent_coverage(
    relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    phrase_chars: usize,
    start_bytes: usize,
    end_bytes: usize,
) -> Option<LocalConfirmationCoverage> {
    completed_secondary_absent_is_authoritative(relation, transcript_chars, phrase_chars).then_some(
        LocalConfirmationCoverage {
            start_bytes,
            end_bytes,
        },
    )
}

#[cfg(target_os = "windows")]
fn run_local_wake_confirmation_once(
    context: LocalWakeConfirmationDiagnosticContext,
    pcm: &[u8],
    phrase: &str,
    phase: &'static str,
) -> Result<LocalWakeConfirmation, String> {
    let snapshot_pcm_ms = pcm.len() / 32;
    // Boost toward KWS/ASR training levels (min 8x): candidate PCM arrives far
    // below them, and an unboosted phrase reads as garbled Absent (session 548).
    let boosted = crate::wake_phrase::gain_normalized_pcm16(pcm);
    save_local_wake_confirmation_inputs(context, phase, pcm, &boosted);
    log::info!(
        "[wake-phrase] stage2 helper submit embedded_session_id={} attempt={} branch={} phase={} origin_pcm_ms={} raw_pcm_ms={} boosted_pcm_ms={}",
        context.embedded_session_id,
        context
            .attempt
            .map(|value| value.to_string())
            .unwrap_or_else(|| "terminal".to_string()),
        context.branch,
        phase,
        context.source_origin_bytes / 32,
        pcm.len() / 32,
        boosted.len() / 32,
    );
    // 活窗分支在 busy 重试期间挂饿死计数：terminal 梯子看到会让位（见
    // confirm_terminal_local_windows）。terminal 分支自己不计数——它就是
    // 常见的占有人，计了会自我等待。让位要能交接：terminal 当前格推理
    // ≤1s 就结束、随后停在窗口边界等计数归零，活窗的 busy 耐心必须覆盖
    // 这个交接窗（360ms 等不到），放宽到与让位上限同宽的 2s；terminal
    // 分支维持 360ms 短预算，不会被反向下游堵死。
    let is_live_stream_branch = context.branch.starts_with("streaming-");
    let busy_deadline = Instant::now()
        + Duration::from_millis(if is_live_stream_branch {
            TERMINAL_CONFIRM_YIELD_TO_LIVE_MS.max(LOCAL_WAKE_HELPER_BUSY_RETRY_BUDGET_MS)
        } else {
            LOCAL_WAKE_HELPER_BUSY_RETRY_BUDGET_MS
        });
    let mut live_starving: Option<LiveStage2StarvingGuard> = None;
    let result = loop {
        match crate::asr::local::wake_helper::confirm(&boosted, phrase, Duration::from_secs(4)) {
            Err(err)
                if crate::asr::local::wake_helper::is_busy_error(&err)
                    && Instant::now() < busy_deadline =>
            {
                if is_live_stream_branch && live_starving.is_none() {
                    log::info!(
                        "[wake-phrase] live stage2 confirm starving on busy helper embedded_session_id={} branch={} attempt={} phase={}",
                        context.embedded_session_id,
                        context.branch,
                        context
                            .attempt
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "terminal".to_string()),
                        phase,
                    );
                    live_starving = Some(LiveStage2StarvingGuard::arm());
                }
                std::thread::sleep(Duration::from_millis(
                    LOCAL_WAKE_HELPER_BUSY_RETRY_INTERVAL_MS,
                ));
            }
            result => break result,
        }
    }
    .map_err(|err| format!("local wake confirmation failed: {err}"))?;
    drop(live_starving);
    log::info!(
        "[wake-phrase] stage2 helper result embedded_session_id={} attempt={} branch={} phase={} request_id={} matched={} phrase_relation={:?} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={} transcript={:?}",
        context.embedded_session_id,
        context
            .attempt
            .map(|value| value.to_string())
            .unwrap_or_else(|| "terminal".to_string()),
        context.branch,
        phase,
        result.request_id,
        result.matched,
        result.phrase_relation,
        result.transcript_chars,
        result.phonetic_prefix_units,
        result.phonetic_best_distance,
        result.phonetic_best_window_start,
        result.inference_ms,
        &result.transcript_text,
    );
    #[cfg(test)]
    if explicit_wake_diagnostic_directory(std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).ok()).is_some() {
        *LAST_LOCAL_WAKE_DIAGNOSTIC_RESULT
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .expect("local wake diagnostic result lock") =
            Some((result.request_id.clone(), result.transcript_text.clone()));
    }
    Ok(LocalWakeConfirmation {
        matched: result.matched,
        phrase_relation: result.phrase_relation,
        transcript_chars: result.transcript_chars,
        phonetic_prefix_units: result.phonetic_prefix_units,
        phonetic_best_distance: result.phonetic_best_distance,
        phonetic_best_window_start: result.phonetic_best_window_start,
        inference_ms: result.inference_ms,
        snapshot_pcm_ms,
        recovered_keyword_end_seconds: None,
    })
}

#[cfg(target_os = "windows")]
fn spawn_local_wake_confirmation(
    _inner: &Arc<Inner>,
    context: LocalWakeConfirmationDiagnosticContext,
    pcm: Vec<u8>,
    phrase: String,
    exploratory_local_only: bool,
) -> tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        // Exploratory (no KWS yet) keeps the provided snapshot as-is.
        // KWS path: primary is the LST-WAKE-009 5 s tail; on Absent, retry a
        // short phrase-focus tail before counting hard-reject evidence.
        let primary = if exploratory_local_only {
            pcm.clone()
        } else {
            local_confirmation_pcm(&pcm, true)
        };
        let primary_context = LocalWakeConfirmationDiagnosticContext {
            source_origin_bytes: context
                .source_origin_bytes
                .saturating_add(pcm.len().saturating_sub(primary.len())),
            ..context
        };
        let mut result = run_local_wake_confirmation_once(
            primary_context,
            &primary,
            &phrase,
            "primary",
        )?;
        if !exploratory_local_only && !result.matched {
            let focus = kws_phrase_focus_pcm(&pcm);
            // Skip duplicate work when primary already was the short focus tail.
            if focus.len() < primary.len() {
                let focus_context = LocalWakeConfirmationDiagnosticContext {
                    source_origin_bytes: context
                        .source_origin_bytes
                        .saturating_add(pcm.len().saturating_sub(focus.len())),
                    ..context
                };
                let focused = run_local_wake_confirmation_once(
                    focus_context,
                    &focus,
                    &phrase,
                    "focus",
                )?;
                if focused.matched {
                    result = focused;
                } else {
                    result.inference_ms = result.inference_ms.saturating_add(focused.inference_ms);
                }
            }
        }
        // Do not synchronously run a second KWS pass after local ASR has already
        // confirmed the phrase. Installed session 212 proved that redundant
        // boundary-only pass can contend with the live spotter for ~2.9 s and
        // turn a completed 0.8 s confirmation into a 5.2 s visible wake. The
        // shared boundary policy already derives a bounded phrase endpoint from
        // snapshot_pcm_ms for start-aligned local transcripts.
        result.recovered_keyword_end_seconds = None;
        result.inference_ms = result
            .inference_ms
            .max(started.elapsed().as_millis() as u64);
        Ok(result)
    })
}

#[cfg(target_os = "windows")]
async fn confirm_terminal_local_windows(
    inner: &Arc<Inner>,
    pcm: &[u8],
    phrase: &str,
    local_confirmation_ms: &mut u64,
    embedded_session_id: u32,
) -> Option<(LocalWakeConfirmation, usize)> {
    let mut last = None;
    for (confirm_pcm, origin) in terminal_local_confirmation_windows(pcm) {
        // 窗口边界让位（2026-09-23 569 实锤）：有活窗 stage2 正被 busy 拒绝
        // 饿着时，死窗的下一格推理等它先过——本窗已在轮出路上，晚一格
        // 只影响本来就慢的迟到激活，而活窗等的是用户眼前的胶囊。让位
        // 只延迟不丢弃：超时（计数器异常）后照常发下一窗，seam 捕获能
        // 力不受损。
        let yield_started = Instant::now();
        let mut yielded_logged = false;
        while WAKE_LIVE_STAGE2_STARVING.load(std::sync::atomic::Ordering::Relaxed) > 0
            && yield_started.elapsed().as_millis()
                < TERMINAL_CONFIRM_YIELD_TO_LIVE_MS as u128
        {
            if !yielded_logged {
                yielded_logged = true;
                log::info!(
                    "[wake-phrase] terminal ladder yields to starving live stage2 embedded_session_id={} window_origin_pcm_ms={}",
                    embedded_session_id,
                    origin / 32
                );
            }
            tokio::time::sleep(Duration::from_millis(
                LOCAL_WAKE_HELPER_BUSY_RETRY_INTERVAL_MS,
            ))
            .await;
        }
        match spawn_local_wake_confirmation(
            inner,
            LocalWakeConfirmationDiagnosticContext {
                embedded_session_id,
                attempt: None,
                source_origin_bytes: origin,
                branch: "terminal-inflight",
            },
            confirm_pcm,
            phrase.to_string(),
            true,
        )
        .await
        {
            Ok(Ok(result)) => {
                *local_confirmation_ms = local_confirmation_ms.saturating_add(result.inference_ms);
                log::info!(
                    "[wake-phrase] terminal local confirmation finished embedded_session_id={} origin_pcm_ms={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={}",
                    embedded_session_id,
                    origin / 32,
                    result.matched,
                    result.phrase_relation,
                    result.snapshot_pcm_ms,
                    result.transcript_chars,
                    result.phonetic_prefix_units,
                    result.phonetic_best_distance,
                    result.phonetic_best_window_start,
                    result.inference_ms
                );
                if terminal_local_result_can_activate(&result) {
                    return Some((result, origin));
                }
                last = Some((result, origin));
            }
            Ok(Err(err)) => {
                log::warn!(
                    "[wake-phrase] terminal local confirmation failed embedded_session_id={embedded_session_id}: {err}"
                );
            }
            Err(err) => {
                log::warn!(
                    "[wake-phrase] terminal local confirmation join failed embedded_session_id={embedded_session_id}: {err}"
                );
            }
        }
    }
    last
}

struct PreservedCandidateFactLedger {
    candidate_id: u64,
    ledger: crate::observability::CandidateFactLedger,
    source_admission_ledger: Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
}

const CAPTURE_ADMISSION_BINDING_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CaptureAdmissionBindingStatus {
    CaptureGenerationMissing,
    CaptureFactMissing,
    CaptureReceiptMissing,
    CaptureReceiptFactMismatch,
    CaptureEvidenceIncomplete,
    ActorFactMissing,
    ActorAdmissionMismatch,
    ActorMetadataMissing,
    ActorMetadataMismatch,
    ActorMetadataIncomplete,
    ActorIgnored,
    ActorNotConsumed,
    MatchedConsumed,
}

const SOURCE_ADMISSION_DEPENDENCY_CAPACITY: usize = 4_096;
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceAdmissionUse {
    CandidateGateInput,
    SessionBodyInput,
    SessionBodyInputPossible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(super) enum SourceAdmissionOperationOwner {
    Candidate { candidate_id: u64 },
    Session { session_id: SessionId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(super) enum SourceAdmissionOwnerRange {
    Candidate { start: u64, end: u64 },
    Session { start: u64, end: u64 },
}

#[derive(Clone)]
struct CaptureAdmissionSourceContext {
    capture_generation: Option<u64>,
    capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    capture_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
    actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
}

impl CaptureAdmissionSourceContext {
    fn missing() -> Self {
        Self {
            capture_generation: None,
            capture_fact: None,
            capture_receipt: None,
            actor_fact: None,
            actor_chunk_metadata: None,
        }
    }

    fn status(&self, actor_consumed: bool) -> CaptureAdmissionBindingStatus {
        capture_admission_binding_status(
            self.capture_generation,
            self.capture_fact.as_ref(),
            self.capture_receipt.as_ref(),
            self.actor_fact.as_ref(),
            self.actor_chunk_metadata,
            actor_consumed,
        )
    }

    fn dependency(
        &self,
        operation_owner: SourceAdmissionOperationOwner,
        use_kind: SourceAdmissionUse,
        owner_range: Option<SourceAdmissionOwnerRange>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
        accepted_bytes: usize,
        operation_id: Option<u64>,
    ) -> SourceAdmissionDependency {
        SourceAdmissionDependency {
            operation_owner,
            capture_generation: self.capture_generation,
            capture_fact: self.capture_fact.clone(),
            capture_receipt: self.capture_receipt.clone(),
            actor_fact: self.actor_fact.clone(),
            actor_chunk_metadata: self.actor_chunk_metadata,
            use_kind,
            accepted_bytes: accepted_bytes as u64,
            operations: VecDeque::from([SourceAdmissionOperation {
                owner: operation_owner,
                operation_id,
                owner_range,
                source_interval,
                accepted_bytes: accepted_bytes as u64,
            }]),
            status_at_acceptance: self.status(true),
            tracking_incomplete: operation_id.is_none()
                || owner_range.is_none()
                || source_interval.is_none()
                || use_kind == SourceAdmissionUse::SessionBodyInputPossible
                || self.status(true) != CaptureAdmissionBindingStatus::MatchedConsumed,
            non_projection_incomplete: operation_id.is_none()
                || owner_range.is_none()
                || source_interval.is_none()
                || use_kind == SourceAdmissionUse::SessionBodyInputPossible
                || self.status(true) != CaptureAdmissionBindingStatus::MatchedConsumed,
        }
    }
}

#[derive(Clone)]
pub(super) struct SourceAdmissionDependency {
    pub(super) operation_owner: SourceAdmissionOperationOwner,
    pub(super) capture_generation: Option<u64>,
    pub(super) capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    pub(super) capture_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
    pub(super) actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    pub(super) actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    pub(super) use_kind: SourceAdmissionUse,
    pub(super) accepted_bytes: u64,
    pub(super) operations: VecDeque<SourceAdmissionOperation>,
    pub(super) status_at_acceptance: CaptureAdmissionBindingStatus,
    pub(super) tracking_incomplete: bool,
    /// A bounded projection can be incomplete while the active dependency
    /// remains fully attributable. Qualification must distinguish that loss
    /// from a real missing/ambiguous source operation.
    pub(super) non_projection_incomplete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceAdmissionOperation {
    pub(super) owner: SourceAdmissionOperationOwner,
    pub(super) operation_id: Option<u64>,
    pub(super) owner_range: Option<SourceAdmissionOwnerRange>,
    pub(super) source_interval: Option<crate::observability::PcmSourceInterval>,
    pub(super) accepted_bytes: u64,
}

impl SourceAdmissionDependency {
    fn current_status(&self) -> CaptureAdmissionBindingStatus {
        capture_admission_binding_status(
            self.capture_generation,
            self.capture_fact.as_ref(),
            self.capture_receipt.as_ref(),
            self.actor_fact.as_ref(),
            self.actor_chunk_metadata,
            true,
        )
    }

    fn same_identity(&self, other: &Self) -> bool {
        if self.operation_owner != other.operation_owner || self.use_kind != other.use_kind {
            return false;
        }
        match (&self.capture_receipt, &other.capture_receipt) {
            (Some(left), Some(right)) => left.same_reference(right),
            (None, None) => {
                self.capture_generation == other.capture_generation
                    && self.capture_fact == other.capture_fact
                    && self.actor_fact == other.actor_fact
                    && self.actor_chunk_metadata == other.actor_chunk_metadata
            }
            _ => false,
        }
    }

    fn add_operation(&mut self, other: &SourceAdmissionDependency) -> Result<OperationRecordResult, ()> {
        let Some(operation) = other.operations.front().copied() else {
            return Err(());
        };
        if operation.operation_id.is_some() {
            if let Some(existing) = self
                .operations
                .iter()
                .find(|current| current.same_key(&operation))
            {
                self.tracking_incomplete |= other.tracking_incomplete;
                self.non_projection_incomplete |= other.non_projection_incomplete;
                if existing.accepted_bytes == operation.accepted_bytes {
                    return Ok(OperationRecordResult::Duplicate);
                }
                self.tracking_incomplete = true;
                self.non_projection_incomplete = true;
                return Ok(OperationRecordResult::Conflict);
            }
        }
        let accepted_bytes = self
            .accepted_bytes
            .checked_add(operation.accepted_bytes)
            .ok_or(())?;
        self.operations.push_back(operation);
        self.accepted_bytes = accepted_bytes;
        self.tracking_incomplete |= other.tracking_incomplete;
        self.non_projection_incomplete |= other.non_projection_incomplete;
        Ok(OperationRecordResult::Added)
    }

    fn projection(&self) -> SourceAdmissionDependencyProjection {
        let operation = self.operations.front().copied().unwrap_or(SourceAdmissionOperation {
            owner: self.operation_owner,
            operation_id: None,
            owner_range: None,
            source_interval: None,
            accepted_bytes: self.accepted_bytes,
        });
        SourceAdmissionDependencyProjection {
            operation_owner: self.operation_owner,
            capture_generation: self.capture_generation,
            capture_fact: self.capture_fact.clone(),
            capture_receipt: self.capture_receipt.clone(),
            actor_fact: self.actor_fact.clone(),
            actor_chunk_metadata: self.actor_chunk_metadata,
            use_kind: self.use_kind,
            operation_id: operation.operation_id,
            owner_range: operation.owner_range,
            source_interval: operation.source_interval,
            accepted_bytes: operation.accepted_bytes,
            status_at_acceptance: self.status_at_acceptance,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OperationRecordResult {
    Added,
    Duplicate,
    Conflict,
}

impl SourceAdmissionOperation {
    fn same_key(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.operation_id.is_some()
            && self.operation_id == other.operation_id
            && self.owner_range == other.owner_range
            && self.source_interval == other.source_interval
    }
}

#[derive(Clone)]
struct SourceAdmissionDependencyProjection {
    operation_owner: SourceAdmissionOperationOwner,
    capture_generation: Option<u64>,
    capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    capture_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
    actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    use_kind: SourceAdmissionUse,
    operation_id: Option<u64>,
    owner_range: Option<SourceAdmissionOwnerRange>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
    accepted_bytes: u64,
    status_at_acceptance: CaptureAdmissionBindingStatus,
}

impl SourceAdmissionDependencyProjection {
    fn same_identity(&self, other: &SourceAdmissionDependency) -> bool {
        if self.operation_owner != other.operation_owner || self.use_kind != other.use_kind {
            return false;
        }
        match (&self.capture_receipt, &other.capture_receipt) {
            (Some(left), Some(right)) => left.same_reference(right),
            (None, None) => {
                self.capture_generation == other.capture_generation
                    && self.capture_fact == other.capture_fact
                    && self.actor_fact == other.actor_fact
                    && self.actor_chunk_metadata == other.actor_chunk_metadata
            }
            _ => false,
        }
    }

    fn same_operation(&self, other: &SourceAdmissionDependency) -> bool {
        let Some(operation) = other.operations.front().copied() else {
            return false;
        };
        self.operation_owner == operation.owner
            && self.operation_id.is_some()
            && self.operation_id == operation.operation_id
            && self.owner_range == operation.owner_range
            && self.source_interval == operation.source_interval
    }
}

#[derive(Default)]
pub(super) struct SourceAdmissionDependencyLedger {
    /// These entries live for the lifetime of the candidate/session owner.
    /// They retain receipt Arcs even when diagnostic projections rotate out.
    dependencies: VecDeque<SourceAdmissionDependency>,
    projections: VecDeque<SourceAdmissionDependencyProjection>,
    dropped_projection_count: u64,
    incomplete: bool,
    non_projection_incomplete: bool,
    confirmed_source_integrity_conflict:
        Option<crate::coordinator::source_integrity::SourceIntegrityConfirmedConflict>,
}

impl SourceAdmissionDependencyLedger {
    pub(super) fn record(&mut self, dependency: SourceAdmissionDependency) {
        if let Some(existing) = self
            .dependencies
            .iter_mut()
            .find(|existing| existing.same_identity(&dependency))
        {
            match existing.add_operation(&dependency) {
                Ok(OperationRecordResult::Added | OperationRecordResult::Duplicate) => {}
                Ok(OperationRecordResult::Conflict) => {
                    self.incomplete = true;
                }
                Err(()) => {
                    existing.tracking_incomplete = true;
                    existing.non_projection_incomplete = true;
                    self.incomplete = true;
                    self.non_projection_incomplete = true;
                }
            }
        } else {
            self.incomplete |= dependency.tracking_incomplete;
            self.non_projection_incomplete |= dependency.non_projection_incomplete;
            self.dependencies.push_back(dependency.clone());
        }

        let projection = dependency.projection();
        if !self.projections.iter().any(|existing| {
            existing.same_identity(&dependency) && existing.same_operation(&dependency)
        }) {
            if self.projections.len() >= SOURCE_ADMISSION_DEPENDENCY_CAPACITY {
                self.projections.pop_front();
                self.dropped_projection_count = self.dropped_projection_count.saturating_add(1);
                self.incomplete = true;
                if let Some(active) = self
                    .dependencies
                    .iter_mut()
                    .find(|active| active.same_identity(&dependency))
                {
                    active.tracking_incomplete = true;
                }
            }
            self.projections.push_back(projection);
        }
        if dependency.status_at_acceptance != CaptureAdmissionBindingStatus::MatchedConsumed
            || dependency.use_kind == SourceAdmissionUse::SessionBodyInputPossible
            || dependency
                .operations
                .front()
                .is_none_or(|operation| operation.owner_range.is_none())
        {
            self.incomplete = true;
        }
    }

    fn snapshots(&self) -> Vec<SourceAdmissionDependencySnapshot> {
        self.dependencies
            .iter()
            .map(|dependency| {
                let mut owner_ranges = Vec::new();
                let mut source_intervals = Vec::new();
                for projection in self
                    .projections
                    .iter()
                    .filter(|projection| projection.same_identity(dependency))
                {
                    if let Some(owner_range) = projection.owner_range {
                        if !owner_ranges.contains(&owner_range) {
                            owner_ranges.push(owner_range);
                        }
                    }
                    if let Some(source_interval) = projection.source_interval {
                        if !source_intervals.contains(&source_interval) {
                            source_intervals.push(source_interval);
                        }
                    }
                }
                let witness = dependency
                    .capture_receipt
                    .as_ref()
                    .map(|receipt| receipt.witness());
                let current_status = dependency.current_status();
                SourceAdmissionDependencySnapshot {
                    operation_owner: dependency.operation_owner,
                    use_kind: dependency.use_kind,
                    capture_generation: dependency.capture_generation,
                    capture_collector_instance_id: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.collector_instance_id),
                    capture_reset_epoch: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.reset_epoch),
                    capture_notification_id: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.notification_id),
                    capture_admission_id: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.admission_id),
                    physical_session_id: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.physical_session_id),
                    packet_sequence: dependency
                        .capture_fact
                        .as_ref()
                        .and_then(|fact| fact.packet_sequence),
                    owner_ranges,
                    source_intervals,
                    accepted_bytes: dependency.accepted_bytes,
                    status_at_acceptance: dependency.status_at_acceptance,
                    current_status,
                    superseded: witness.as_ref().map(|witness| witness.superseded),
                    superseded_by_admission_id: witness
                        .as_ref()
                        .and_then(|witness| witness.superseded_by_admission_id),
                    superseded_by_notification_id: witness
                        .as_ref()
                        .and_then(|witness| witness.superseded_by_notification_id),
                    tracking_incomplete: witness
                        .as_ref()
                        .is_some_and(|witness| witness.metadata_incomplete)
                        || current_status == CaptureAdmissionBindingStatus::CaptureEvidenceIncomplete
                        || dependency.tracking_incomplete
                        || dependency.operations.iter().any(|operation| {
                            operation.operation_id.is_none() || operation.owner_range.is_none()
                        })
                        || dependency.operations.iter().any(|operation| {
                            !self.projections.iter().any(|projection| {
                                projection.same_identity(dependency)
                                    && projection.operation_id == operation.operation_id
                                    && projection.owner_range == operation.owner_range
                                    && projection.source_interval == operation.source_interval
                            })
                        }),
                }
            })
            .collect()
    }

    pub(super) fn active_dependencies(&self) -> impl Iterator<Item = &SourceAdmissionDependency> {
        self.dependencies.iter()
    }

    #[cfg(test)]
    pub(super) fn active_dependencies_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut SourceAdmissionDependency> {
        self.dependencies.iter_mut()
    }

    pub(super) fn has_non_projection_incomplete(&self) -> bool {
        self.non_projection_incomplete
    }

    pub(super) fn confirmed_source_integrity_conflict(
        &self,
    ) -> Option<&crate::coordinator::source_integrity::SourceIntegrityConfirmedConflict> {
        self.confirmed_source_integrity_conflict.as_ref()
    }

    pub(super) fn latch_source_integrity_conflict(
        &mut self,
        conflict: crate::coordinator::source_integrity::SourceIntegrityConfirmedConflict,
    ) {
        if self.confirmed_source_integrity_conflict.is_none() {
            self.confirmed_source_integrity_conflict = Some(conflict);
        }
    }

    fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub(super) fn mark_incomplete(&mut self) {
        self.incomplete = true;
        self.non_projection_incomplete = true;
    }
}

fn record_source_admission_dependency(
    ledger: &Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
    context: Option<&CaptureAdmissionSourceContext>,
    operation_owner: SourceAdmissionOperationOwner,
    use_kind: SourceAdmissionUse,
    owner_range: Option<SourceAdmissionOwnerRange>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
    accepted_bytes: usize,
    operation_id: Option<u64>,
) {
    let context = context
        .cloned()
        .unwrap_or_else(CaptureAdmissionSourceContext::missing);
    ledger
        .lock()
        .expect("source admission dependency ledger lock")
        .record(context.dependency(
            operation_owner,
            use_kind,
            owner_range,
            source_interval,
            accepted_bytes,
            operation_id,
        ));
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceAdmissionDependencySnapshot {
    operation_owner: SourceAdmissionOperationOwner,
    use_kind: SourceAdmissionUse,
    capture_generation: Option<u64>,
    capture_collector_instance_id: Option<u64>,
    capture_reset_epoch: Option<u64>,
    capture_notification_id: Option<u64>,
    capture_admission_id: Option<u64>,
    physical_session_id: Option<u32>,
    packet_sequence: Option<u16>,
    owner_ranges: Vec<SourceAdmissionOwnerRange>,
    source_intervals: Vec<crate::observability::PcmSourceInterval>,
    accepted_bytes: u64,
    status_at_acceptance: CaptureAdmissionBindingStatus,
    current_status: CaptureAdmissionBindingStatus,
    superseded: Option<bool>,
    superseded_by_admission_id: Option<u64>,
    superseded_by_notification_id: Option<u64>,
    tracking_incomplete: bool,
}

#[derive(Debug)]
struct CaptureAdmissionBinding {
    capture_generation: Option<u64>,
    capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    capture_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
    actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
    actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    actor_consumed: bool,
    status: CaptureAdmissionBindingStatus,
}

const PRESERVED_CANDIDATE_LEDGER_CAPACITY: usize = 64;
const PRESERVED_CANDIDATE_LEDGER_DROP_ID_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug)]
struct AcceptedWakeCaptureEnsure {
    request_id: u32,
    previous_segment_id: u32,
    requested_at: Instant,
    deadline_at: Instant,
    replacement_wait_started_at: Option<Instant>,
    /// Firmware's explicit SessionStart marker proves which physical segment
    /// owns this ENSURE.  Keep it through terminal cleanup so ABORT can still
    /// target a continuation that was already started.
    confirmed_segment_id: Option<u32>,
}

struct EmbeddedStreamingDictation {
    collector: crate::embedded_audio::StreamingSessionCollector,
    /// Bounded source association evidence. This keeps the shared capture
    /// receipt alive across actor reset without affecting packet admission or
    /// any product qualification decision.
    capture_admission_bindings: VecDeque<CaptureAdmissionBinding>,
    capture_admission_bindings_incomplete: bool,
    session: Option<EmbeddedAudioDictationSession>,
    pipeline_observation:
        Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    speaker_candidate: Option<BufferedSpeakerCandidate>,
    /// Retain bounded candidate ledgers when they end without a coordinator
    /// session (reject/cancel/continuation staging) or without a capture
    /// observation sink. This is a collection, never a single overwrite.
    preserved_candidate_fact_ledgers: VecDeque<PreservedCandidateFactLedger>,
    preserved_candidate_ledger_drop_count: u64,
    preserved_candidate_ledger_dropped_ids: VecDeque<u64>,
    preserved_candidate_ledger_incomplete: bool,
    embedded_session_id: Option<u32>,
    /// A notify reopen may replay a middle-of-stream PCM tail after the
    /// corresponding SessionStart was lost. If recovery cannot be admitted
    /// (for example because an older owner is still stopping), quarantine that
    /// exact embedded session id until the firmware advances to a new session.
    /// Without this tombstone every subsequent PCM packet retried recovery and
    /// drove an actor-restart storm, starving fresh wake candidates.
    orphan_recovery_quarantine_session_id: Option<u32>,
    transcript: Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>,
    pending_stop_expected_packet_count: Option<u16>,
    /// When set, continuous background will force-finish a STOP that never
    /// recovered missing packets so capture can keep TYPE:READY open.
    pending_stop_force_after: Option<Instant>,
    /// 2026-08-09 12:46:59 激活竞态：VREC:ACTIVATE 发出后 0.1s 旧唤醒段
    /// complete（仅含 1845ms 唤醒词），其 STOP/complete 不得 finalize 听写会话；
    /// 正文在激活后的新设备段。值为（激活前旧段 embedded_session_id, 激活时刻）。
    /// 竞态窗口外或正文已开始时，旧段结束仍走正常 finalize。
    activation_segment_race_guard: Option<(u32, Instant)>,
    /// A successful ENSURE write is not proof that a new physical segment
    /// exists. Keep the request identity until the original segment is either
    /// confirmed or a matching replacement segment is bound.
    accepted_wake_capture_ensure: Option<AcceptedWakeCaptureEnsure>,
    terminal_received: bool,
    /// Per-notification result used only to classify the current capture
    /// binding. It is reset at the actor command boundary and never feeds
    /// product qualification.
    last_actor_pcm_consumed: bool,
    keep_listening_after_pipeline_errors: bool,
}

impl Default for EmbeddedStreamingDictation {
    fn default() -> Self {
        Self {
            collector: crate::embedded_audio::StreamingSessionCollector::default(),
            capture_admission_bindings: VecDeque::new(),
            capture_admission_bindings_incomplete: false,
            session: None,
            pipeline_observation: None,
            speaker_candidate: None,
            preserved_candidate_fact_ledgers: VecDeque::new(),
            preserved_candidate_ledger_drop_count: 0,
            preserved_candidate_ledger_dropped_ids: VecDeque::new(),
            preserved_candidate_ledger_incomplete: false,
            embedded_session_id: None,
            orphan_recovery_quarantine_session_id: None,
            transcript: None,
            pending_stop_expected_packet_count: None,
            pending_stop_force_after: None,
            activation_segment_race_guard: None,
            accepted_wake_capture_ensure: None,
            terminal_received: false,
            last_actor_pcm_consumed: false,
            keep_listening_after_pipeline_errors: false,
        }
    }
}

impl EmbeddedStreamingDictation {
    fn preserve_candidate_fact_ledger(&mut self, candidate: &mut BufferedSpeakerCandidate) {
        self.preserve_candidate_fact_ledger_with_source(
            candidate.candidate_id,
            std::mem::take(&mut candidate.fact_ledger),
            Arc::clone(&candidate.source_admission_ledger),
        );
    }

    fn preserve_session_candidate_fact_ledger(&mut self, session: &mut EmbeddedAudioDictationSession) {
        if let Some(ledger) = session.candidate_fact_ledger.take() {
            let candidate_id = session
                .candidate_id
                .or_else(|| ledger.facts().first().map(|fact| fact.candidate_id));
            if let Some(candidate_id) = candidate_id {
                self.preserve_candidate_fact_ledger_with_source(
                    candidate_id,
                    ledger,
                    Arc::clone(&session.source_admission_ledger),
                );
            }
        }
    }

    fn preserve_candidate_fact_ledger_with_id(
        &mut self,
        candidate_id: u64,
        ledger: crate::observability::CandidateFactLedger,
    ) {
        self.preserve_candidate_fact_ledger_with_source(
            candidate_id,
            ledger,
            Arc::new(std::sync::Mutex::new(
                SourceAdmissionDependencyLedger::default(),
            )),
        );
    }

    fn preserve_candidate_fact_ledger_with_source(
        &mut self,
        candidate_id: u64,
        ledger: crate::observability::CandidateFactLedger,
        source_admission_ledger: Arc<std::sync::Mutex<SourceAdmissionDependencyLedger>>,
    ) {
        self.preserved_candidate_ledger_incomplete |= ledger.is_incomplete();
        self.preserved_candidate_ledger_incomplete |= source_admission_ledger
            .lock()
            .expect("source admission dependency ledger lock")
            .is_incomplete();
        if self.preserved_candidate_fact_ledgers.len()
            >= PRESERVED_CANDIDATE_LEDGER_CAPACITY
        {
            let dropped_candidate_id = self
                .preserved_candidate_fact_ledgers
                .pop_front()
                .map(|item| item.candidate_id);
            self.preserved_candidate_ledger_drop_count = self
                .preserved_candidate_ledger_drop_count
                .saturating_add(1);
            self.preserved_candidate_ledger_incomplete = true;
            if self.preserved_candidate_ledger_dropped_ids.len()
                >= PRESERVED_CANDIDATE_LEDGER_DROP_ID_CAPACITY
            {
                self.preserved_candidate_ledger_dropped_ids.pop_front();
            }
            if let Some(dropped_candidate_id) = dropped_candidate_id {
                self.preserved_candidate_ledger_dropped_ids
                    .push_back(dropped_candidate_id);
            }
        }
        self.preserved_candidate_fact_ledgers
            .push_back(PreservedCandidateFactLedger {
                candidate_id,
                ledger,
                source_admission_ledger,
            });
    }

    #[cfg(test)]
    fn preserved_candidate_ledger_ids_for_test(&self) -> Vec<u64> {
        self.preserved_candidate_fact_ledgers
            .iter()
            .map(|item| item.candidate_id)
            .collect()
    }

    #[cfg(test)]
    fn preserved_candidate_ledger_state_for_test(&self) -> (Vec<u64>, u64, Vec<u64>, bool) {
        (
            self.preserved_candidate_ledger_ids_for_test(),
            self.preserved_candidate_ledger_drop_count,
            self.preserved_candidate_ledger_dropped_ids.iter().copied().collect(),
            self.preserved_candidate_ledger_incomplete,
        )
    }
}

/// 跑流式润色路径（opt-in，跨平台）。
///
/// 平台差异：
/// - **macOS**：`switch_to_ascii` 切到 ABC 输入源（规避 CJK / 日文 IME 拦截 Unicode 事件），
///   session 结束 `restore_input_source` 切回。`type_unicode_chunk` 走 CGEvent FFI。
/// - **Windows**：`switch_to_ascii` 临时切 en-US 键盘布局（防 CJK IME 吞字），
///   session 结束恢复；`type_unicode_chunk` 走 `SendInput(KEYEVENTF_UNICODE)`。
/// - **Linux（实验）**：`switch_to_ascii` 是 no-op；`type_unicode_chunk` 走 enigo
///   `Keyboard::text`。X11 / XTest 稳定，Wayland 看 compositor 给不给 libei 权限。
///
/// 通用流程：
/// 1. `switch_to_ascii`（macOS）/ no-op（其他）；失败则降级回一次性 `polish_or_passthrough`。
/// 2. 起一个 `spawn_blocking` 后台任务，从 mpsc 收 SSE delta，逐 delta 调
///    `type_unicode_chunk` 模拟键盘事件落到光标处。串行有序，无竞态。
/// 3. 调 `polish_or_passthrough_streaming`，`on_delta` 把 chunk 塞进 mpsc。
/// 4. 流结束 / 失败 / 取消 → drop mpsc 发送端 → typer 任务 drain 完剩余 delta 退出 →
///    `restore_input_source` 恢复用户原输入源（macOS 才有意义，其他平台 no-op）。
/// 5. 返回 `PreparedDeliveryText`：字符是否已经提交、实际提交前缀和最终收尾正文
///    都由同一条生产路径决定；调用方不会再次插入已经流式发送的正文。
///
/// **不在流式路径里做**：`apply_chinese_script_preference` / `apply_correction_rules`
/// 这两步在 v1 跳过 —— 字符已经一边流一边落出去了，不好回退。需要的话只能关 toggle 走
/// 一次性路径。
enum StreamingDeliveryResolution {
    Prepared(PreparedDeliveryText),
    UnsupportedFallback,
}

/// Production resolver for the provider/typer boundary. The provider outcome
/// is already terminal when this function is called, but the delivery is not
/// sealed until the typer task has drained every queued delta. Keeping the
/// await inside this function makes offline timing tests exercise the same
/// ordering as the real streaming path.
async fn resolve_streaming_delivery_after_typer_drain(
    outcome: super::StreamingPolishOutcome,
    raw_text: String,
    typer_handle: tokio::task::JoinHandle<(String, Option<String>)>,
) -> StreamingDeliveryResolution {
    let (typed_text, typer_failure) = typer_handle.await.unwrap_or_else(|error| {
        log::error!("[coord] streaming_insert: typer task join failed: {error}");
        (String::new(), Some(format!("typer join: {error}")))
    });
    let typed_chars = typed_text.chars().count();
    log::info!("[coord] streaming_insert: typer drained, typed {typed_chars} chars");

    match outcome {
        super::StreamingPolishOutcome::Streamed(text) => {
            note_llm_polish_success();
            log::info!(
                "[coord] streaming_insert SUCCESS: polished_chars={} typed_chars={} typer_err={:?}",
                text.chars().count(),
                typed_chars,
                typer_failure
            );
            // If no character reached the input mechanism, the caller must
            // use the ordinary one-shot fallback rather than sealing a
            // streamed delivery with an empty submitted body.
            if typed_chars == 0 {
                if let Some(reason) = typer_failure.as_ref() {
                    log::warn!(
                        "[coord] streaming_insert: zero chars typed despite polish success ({reason}); falling back to one-shot inserter"
                    );
                    return StreamingDeliveryResolution::Prepared(PreparedDeliveryText {
                        intended_text: text.clone(),
                        submitted_text: None,
                        final_text: text,
                        polish_error: Some(reason.clone()),
                        already_streamed: false,
                    });
                }
            }
            // When typing stops part-way through, history/clipboard/final
            // result must describe the prefix that actually reached the
            // target, not the provider's unseen tail.
            StreamingDeliveryResolution::Prepared(build_streamed_nonempty_delivery(
                text,
                typed_text,
                typer_failure,
            ))
        }
        super::StreamingPolishOutcome::UnsupportedFallback => {
            log::info!(
                "[coord] streaming_insert: dispatch reported unsupported, fall back to one-shot"
            );
            StreamingDeliveryResolution::UnsupportedFallback
        }
        super::StreamingPolishOutcome::Failed(reason) => {
            log::warn!(
                "[coord] streaming_insert FAILED: {reason}; typed {typed_chars} chars before failure"
            );
            let auth_failure = reason.contains("credentials were already rejected")
                || reason.contains("AuthenticationError")
                || reason.contains("status 401")
                || reason.contains("status 403")
                || reason.contains("Unauthorized");
            if !auth_failure {
                note_llm_polish_stall_failure();
            }
            if typed_chars > 0 {
                StreamingDeliveryResolution::Prepared(PreparedDeliveryText {
                    intended_text: raw_text,
                    submitted_text: Some(typed_text.clone()),
                    final_text: typed_text,
                    polish_error: if auth_failure {
                        None
                    } else {
                        Some(format!(
                            "streaming polish failed mid-stream after {typed_chars} chars: {reason}"
                        ))
                    },
                    already_streamed: true,
                })
            } else if auth_failure {
                StreamingDeliveryResolution::Prepared(PreparedDeliveryText {
                    intended_text: raw_text.clone(),
                    submitted_text: None,
                    final_text: raw_text,
                    polish_error: None,
                    already_streamed: false,
                })
            } else {
                StreamingDeliveryResolution::Prepared(PreparedDeliveryText {
                    intended_text: raw_text.clone(),
                    submitted_text: None,
                    final_text: raw_text,
                    polish_error: Some(reason),
                    already_streamed: false,
                })
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_streaming_polish(
    inner: &Arc<Inner>,
    raw: &RawTranscript,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: crate::types::ChineseScriptPreference,
    output_language_preference: crate::types::OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
    prefetch: Option<PolishPrefetch>,
    delivery_id: &str,
) -> PreparedDeliveryText {
    // 预热采用判定：终稿与预热输入逐字一致且预热未失败 → 采用（回放+live）；
    // 否则取消丢弃走正常请求。
    let prefetch = match prefetch {
        Some(p) if polish_prefetch_adoptable(&p, &raw.text) => {
            log::info!(
                "[coord] polish prefetch adopted input_chars={}",
                p.input.chars().count()
            );
            Some(p)
        }
        Some(p) => {
            log::info!(
                "[coord] polish prefetch discarded: final differs (prefetch={} final={})",
                p.input.chars().count(),
                raw.text.chars().count()
            );
            p.cancel.store(true, Ordering::SeqCst);
            None
        }
        None => None,
    };
    log::info!(
        "[coord] streaming_insert path ENTER (raw_chars={})",
        raw.text.chars().count()
    );

    let app = inner.app.lock().clone();
    let Some(app) = app else {
        log::warn!("[coord] streaming_insert: no AppHandle in Inner; fall back to one-shot");
        let (p, e) = polish_or_passthrough(
            raw,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app,
            prior_turns,
        )
        .await;
        return PreparedDeliveryText {
            intended_text: p.clone(),
            submitted_text: None,
            final_text: p,
            polish_error: e,
            already_streamed: false,
        };
    };

    // 1. 切到 ABC 输入源。失败则降级 —— 流式路径上 CJK IME 拦截不是可恢复错误。
    log::info!("[coord] streaming_insert: switching input source to ABC");
    let prev_ime = match crate::unicode_keystroke::switch_to_ascii(&app).await {
        Ok(prev) => {
            log::info!(
                "[coord] streaming_insert: switched to ABC (had_previous={})",
                prev.is_some()
            );
            prev
        }
        Err(e) => {
            log::warn!(
                "[coord] streaming_insert: switch_to_ascii failed: {e}; fall back to one-shot"
            );
            let (p, err) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            return PreparedDeliveryText {
                intended_text: p.clone(),
                submitted_text: None,
                final_text: p,
                polish_error: err,
                already_streamed: false,
            };
        }
    };

    // 2. 起 typer 后台任务：从 mpsc 收 delta，串行调 type_unicode_chunk。
    // 同时累积 typed_text：屏幕上真正落字的内容，用于（a）SSE 中途失败时让 history
    // 与用户实际看到的内容一致；（b）pr-agent #412 反馈 \"saved output diverges
    // from what the user actually sees\"。
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let delivery_id_for_typer = delivery_id.to_string();
    let typer_handle = tokio::task::spawn_blocking(move || {
        let mut rx = rx;
        let mut typed_text = String::new();
        let mut first_failure: Option<String> = None;
        while let Some(delta) = rx.blocking_recv() {
            if first_failure.is_some() {
                // 一旦类型链路出错（如 Secure Input 启用），后续 delta 全部丢弃，但仍
                // 把 mpsc drain 完，避免发送端阻塞。
                continue;
            }
            let delta_chars = delta.chars().count();
            log::debug!(
                "[delivery] streaming chunk send delivery_id={} chars={}",
                delivery_id_for_typer,
                delta_chars
            );
            match crate::unicode_keystroke::type_unicode_chunk(&delta) {
                Ok(typed_chars) => {
                    let appended = append_typed_prefix(&mut typed_text, &delta, typed_chars);
                    if appended < delta_chars {
                        let reason = format!(
                            "type_unicode_chunk typed only {appended}/{delta_chars} chars without error"
                        );
                        log::error!(
                            "[coord] streaming_insert: {reason} at typed={} chars; \
                             dropping remaining deltas",
                            typed_text.chars().count()
                        );
                        first_failure = Some(reason);
                    }
                }
                Err(e) => {
                    append_typed_prefix(&mut typed_text, &delta, e.typed_chars());
                    log::error!(
                        "[coord] streaming_insert: type_unicode_chunk failed at typed={} chars: {e}; \
                         dropping remaining deltas",
                        typed_text.chars().count()
                    );
                    first_failure = Some(e.to_string());
                }
            }
        }
        (typed_text, first_failure)
    });

    // 3. 调流式润色，on_delta 塞 mpsc；should_cancel 检查 dictation 取消旗。
    //    有预热时改为从预热流驱动（先回放缓冲 delta，再切 live）。
    let inner_for_cancel = Arc::clone(inner);
    let should_cancel = move || inner_for_cancel.state.lock().cancelled;
    let outcome = if let Some(prefetch) = prefetch {
        drive_polish_prefetch(prefetch, tx).await
    } else {
        super::polish_or_passthrough_streaming(
            raw,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app,
            prior_turns,
            move |delta: &str| {
                let _ = tx.send(delta.to_string());
            },
            should_cancel,
        )
        .await
    };
    // tx 已经被 move 进 on_delta 闭包；闭包随 polish_or_passthrough_streaming 返回
    // 而 drop，typer 那侧 blocking_recv 拿到 None 自然退出。

    // 4. The production resolver owns the await. This prevents a provider
    // completion from being sealed while queued input is still in flight.
    let resolution = resolve_streaming_delivery_after_typer_drain(
        outcome,
        raw.text.clone(),
        typer_handle,
    )
    .await;

    // 5. 无论流是否成功，都恢复用户原输入源。
    log::info!("[coord] streaming_insert: restoring input source");
    if let Err(e) = crate::unicode_keystroke::restore_input_source(&app, prev_ime).await {
        log::warn!("[coord] streaming_insert: restore_input_source failed: {e}");
    } else {
        log::info!("[coord] streaming_insert: input source restored");
    }

    // 6. Translate the drained production resolution. Unsupported still uses
    // the established one-shot fallback; all other branches were sealed by
    // the resolver after the typer await.
    match resolution {
        StreamingDeliveryResolution::Prepared(prepared) => prepared,
        StreamingDeliveryResolution::UnsupportedFallback => {
            log::info!(
                "[coord] streaming_insert: dispatch reported unsupported, fall back to one-shot"
            );
            let (p, e) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            PreparedDeliveryText {
                intended_text: p.clone(),
                submitted_text: None,
                final_text: p,
                polish_error: e,
                already_streamed: false,
            }
        }
    }
}

/// Resolve a provider-complete stream only after the typer result has been
/// drained. The caller handles the zero-byte case before entering this helper.
fn build_streamed_nonempty_delivery(
    intended_text: String,
    typed_text: String,
    typer_failure: Option<String>,
) -> PreparedDeliveryText {
    let (final_text, polish_error) = match typer_failure {
        Some(error) => (
            typed_text.clone(),
            Some(format!("typing partially failed: {error}")),
        ),
        None => (intended_text.clone(), None),
    };
    PreparedDeliveryText {
        intended_text,
        submitted_text: Some(typed_text),
        final_text,
        polish_error,
        already_streamed: true,
    }
}

/// 从预热流驱动 typer：先回放缓冲的 delta，再随流 live 转发，直到预热任务
/// 写入最终结果。只在采用判定通过后调用（输入与终稿逐字一致）。
async fn drive_polish_prefetch(
    prefetch: PolishPrefetch,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> super::StreamingPolishOutcome {
    loop {
        let notified = prefetch.notify.notified();
        let next = {
            let mut buf = prefetch.buf.lock();
            if let Some(chunk) = buf.chunks.pop_front() {
                Some(Ok(chunk))
            } else {
                buf.result.take().map(Err)
            }
        };
        match next {
            Some(Ok(chunk)) => {
                let _ = tx.send(chunk);
            }
            Some(Err(outcome)) => return outcome,
            None => notified.await,
        }
    }
}

fn finalize_polished_text(
    polished: String,
    translation_active: bool,
    _raw_uses_llm: bool,
    mode: PolishMode,
    polish_error: &Option<String>,
    chinese_script_preference: crate::types::ChineseScriptPreference,
    correction_rules: &[crate::types::CorrectionRule],
    already_streamed: bool,
) -> String {
    if already_streamed {
        return polished;
    }
    let should_force_script = if translation_active {
        polish_error.is_some()
    } else {
        mode == PolishMode::Raw || polish_error.is_some()
    };
    let polished = if should_force_script {
        apply_chinese_script_preference(&polished, chinese_script_preference)
    } else {
        polished
    };
    if correction_rules.is_empty() {
        polished
    } else {
        let corrected = apply_correction_rules(&polished, correction_rules);
        if corrected != polished {
            log::info!(
                "[coord] correction rules adjusted final text ({} → {} chars)",
                polished.chars().count(),
                corrected.chars().count()
            );
        }
        corrected
    }
}

fn streaming_insert_eligible(
    streaming_insert_enabled: bool,
    translation_active: bool,
    mode: PolishMode,
    raw_uses_llm: bool,
    wayland_session: bool,
) -> bool {
    streaming_insert_enabled
        && !translation_active
        && (mode != PolishMode::Raw || raw_uses_llm)
        && !wayland_session
}

fn wayland_done_message(status: InsertStatus, polish_failed: bool) -> Option<String> {
    match status {
        InsertStatus::Inserted | InsertStatus::PasteSent => None,
        InsertStatus::SubmittedUnconfirmed => {
            Some("Wayland 输入事件已发送，但未确认上屏；请检查目标窗口".to_string())
        }
        InsertStatus::CopiedFallback => Some(if polish_failed {
            "Wayland 未启用自动输入，已复制原文到剪贴板，请手动粘贴".to_string()
        } else {
            "Wayland 未启用自动输入，已复制到剪贴板，请手动粘贴".to_string()
        }),
        InsertStatus::Failed => Some("Wayland 未启用自动输入，剪贴板写入失败".to_string()),
    }
}

fn default_done_message(
    status: InsertStatus,
    polish_failed: bool,
    clipboard_retained: bool,
) -> Option<String> {
    // Successful on-screen delivery (type or paste) should feel quiet. Loud
    // "已粘贴原文 / 仍在剪贴板" after a working session made owner feel the
    // product failed even though text already landed (LLM key 401 case).
    // history still records polishFailed via error_code for diagnostics.
    match status {
        InsertStatus::Inserted | InsertStatus::PasteSent => None,
        InsertStatus::SubmittedUnconfirmed => Some(
            "输入事件已发送，但 Listener 未收到目标确认；请检查光标位置".to_string(),
        ),
        InsertStatus::CopiedFallback => Some(if polish_failed {
            if cfg!(target_os = "windows") {
                "润色不可用，已复制原文，请 Ctrl+V".to_string()
            } else {
                "润色不可用，已复制原文，请粘贴".to_string()
            }
        } else if cfg!(target_os = "windows") {
            "已复制到剪贴板，请 Ctrl+V".to_string()
        } else {
            "已复制到剪贴板，请粘贴".to_string()
        }),
        InsertStatus::Failed => Some(if clipboard_retained {
            if cfg!(target_os = "windows") {
                "上屏失败，内容在剪贴板，请 Ctrl+V".to_string()
            } else {
                "上屏失败，内容在剪贴板，请粘贴".to_string()
            }
        } else if polish_failed {
            "润色不可用，插入失败".to_string()
        } else {
            "插入失败".to_string()
        }),
    }
}

fn device_processing_final_succeeded(status: InsertStatus, error_code: Option<&str>) -> bool {
    if matches!(
        status,
        InsertStatus::Failed | InsertStatus::SubmittedUnconfirmed
    ) {
        return false;
    }
    matches!(error_code, None | Some("polishFailed"))
}
