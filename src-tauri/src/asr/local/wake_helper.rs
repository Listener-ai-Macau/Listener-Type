const WAKE_HELPER_BUSY_ERROR_CODE: &str = "local_wake_helper_busy";

pub fn is_busy_error(error: &str) -> bool {
    error.contains(WAKE_HELPER_BUSY_ERROR_CODE)
}

#[cfg(target_os = "windows")]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use pinyin::ToPinyin;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::super::foundry_provider::TempWavFile;
    use super::super::paraformer::ParaformerRuntime;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const HELPER_READY_TIMEOUT: Duration = Duration::from_secs(30);
    const HELPER_AUDIO_TIMEOUT: Duration = Duration::from_secs(4);

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WakeHelperResult {
        pub request_id: String,
        pub matched: bool,
        pub phrase_relation: crate::wake_phrase::LocalPhraseRelation,
        pub transcript_chars: usize,
        pub phonetic_prefix_units: usize,
        pub phonetic_best_distance: usize,
        pub phonetic_best_window_start: usize,
        pub inference_ms: u64,
        pub transcript_text: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ShadowTranscriptResult {
        pub text: String,
        pub inference_ms: u64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum HelperRequest {
        Confirm {
            request_id: String,
            wav_path: String,
            phrase: String,
        },
        Transcribe {
            request_id: String,
            wav_path: String,
        },
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum HelperResponse {
        Ready {
            model_id: Option<String>,
            warmup_ms: u64,
            error: Option<String>,
        },
        Result {
            request_id: String,
            matched: bool,
            phrase_relation: crate::wake_phrase::LocalPhraseRelation,
            transcript_chars: usize,
            phonetic_prefix_units: usize,
            phonetic_best_distance: usize,
            phonetic_best_window_start: usize,
            inference_ms: u64,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            transcript_text: Option<String>,
            error: Option<String>,
        },
        Transcript {
            request_id: String,
            text: String,
            inference_ms: u64,
            error: Option<String>,
        },
    }

    struct HelperProcess {
        child: Child,
        stdin: ChildStdin,
        responses: Receiver<String>,
    }

    impl HelperProcess {
        fn spawn() -> Result<Self, String> {
            // The production process uses its own executable. An explicitly
            // selected executable is allowed only for the ignored, offline
            // captured-PCM diagnostic, whose test harness has a different
            // current_exe and cannot serve the helper protocol itself.
            let diagnostic_mode = std::env::var_os("LISTENER_WAKE_DIAGNOSTIC_DIR")
                .is_some_and(|value| !value.to_string_lossy().trim().is_empty());
            let executable = if diagnostic_mode {
                std::env::var_os("LISTENER_WAKE_HELPER_EXE")
                    .map(std::path::PathBuf::from)
                    .or_else(|| std::env::current_exe().ok())
            } else {
                std::env::current_exe().ok()
            }
            .ok_or_else(|| "resolve Listener Type executable".to_string())?;
            let mut child = Command::new(executable)
                .arg("--local-wake-helper")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|err| format!("start local wake helper: {err}"))?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| "local wake helper stdin unavailable".to_string())?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "local wake helper stdout unavailable".to_string())?;
            let (response_tx, responses) = mpsc::channel();
            std::thread::Builder::new()
                .name("listener-wake-helper-reader".into())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else {
                            break;
                        };
                        if response_tx.send(line).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|err| format!("start local wake helper reader: {err}"))?;

            let mut process = Self {
                child,
                stdin,
                responses,
            };
            match process.receive(HELPER_READY_TIMEOUT)? {
                HelperResponse::Ready {
                    model_id: Some(model_id),
                    warmup_ms,
                    error: None,
                } => {
                    log::info!(
                        "[wake-phrase] local wake helper true-warm ready model_id={model_id} warmup_ms={warmup_ms}"
                    );
                    Ok(process)
                }
                HelperResponse::Ready { error, .. } => {
                    let _ = process.child.kill();
                    Err(error.unwrap_or_else(|| "local wake helper model unavailable".to_string()))
                }
                response => {
                    let _ = process.child.kill();
                    Err(format!(
                        "local wake helper returned unexpected startup response: {response:?}"
                    ))
                }
            }
        }

        fn receive(&mut self, timeout: Duration) -> Result<HelperResponse, String> {
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(format!(
                        "local wake helper timed out after {} ms",
                        timeout.as_millis()
                    ));
                }
                let line = self
                    .responses
                    .recv_timeout(remaining)
                    .map_err(|err| format!("local wake helper response unavailable: {err}"))?;
                if let Ok(response) = serde_json::from_str(&line) {
                    return Ok(response);
                }
            }
        }

        fn send(&mut self, request: &HelperRequest) -> Result<(), String> {
            let payload = serde_json::to_string(request)
                .map_err(|err| format!("encode local wake helper request: {err}"))?;
            self.stdin
                .write_all(payload.as_bytes())
                .and_then(|_| self.stdin.write_all(b"\n"))
                .and_then(|_| self.stdin.flush())
                .map_err(|err| format!("write local wake helper request: {err}"))
        }

        fn is_running(&mut self) -> bool {
            self.child.try_wait().ok().flatten().is_none()
        }
    }

    impl Drop for HelperProcess {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[derive(Default)]
    struct WakeHelperClient {
        process: Mutex<Option<HelperProcess>>,
    }

    impl WakeHelperClient {
        fn preload(&self) -> Result<(), String> {
            super::super::paraformer::prepare_assets()?;
            let mut process = self.process.lock();
            Self::ensure_process(&mut process)?;
            Ok(())
        }

        fn confirm(
            &self,
            pcm: &[u8],
            phrase: &str,
            timeout: Duration,
        ) -> Result<WakeHelperResult, String> {
            if pcm.is_empty() {
                return Err("local wake helper received empty PCM".to_string());
            }
            // A discarded candidate cannot cancel spawn_blocking work. Never let
            // those requests queue behind the single helper process: the live
            // candidate will retry from newer PCM through its bounded ladder.
            let mut process_slot = self
                .process
                .try_lock()
                .ok_or_else(|| super::WAKE_HELPER_BUSY_ERROR_CODE.to_string())?;
            let wav = TempWavFile::create_without_context_padding(pcm)
                .map_err(|err| format!("prepare local wake helper WAV: {err:#}"))?;
            let request_id = Uuid::new_v4().to_string();
            let request = HelperRequest::Confirm {
                request_id: request_id.clone(),
                wav_path: wav.path().to_string_lossy().into_owned(),
                phrase: phrase.to_string(),
            };

            let process = Self::ensure_process(&mut process_slot)?;
            if let Err(err) = process.send(&request) {
                *process_slot = None;
                return Err(err);
            }
            let response = match process.receive(timeout) {
                Ok(response) => response,
                Err(err) => {
                    *process_slot = None;
                    return Err(err);
                }
            };
            match response {
                HelperResponse::Result {
                    request_id: response_id,
                    matched,
                    phrase_relation,
                    transcript_chars,
                    phonetic_prefix_units,
                    phonetic_best_distance,
                    phonetic_best_window_start,
                    inference_ms,
                    transcript_text,
                    error,
                } if response_id == request_id => {
                    if let Some(error) = error {
                        Err(error)
                    } else {
                        Ok(WakeHelperResult {
                            request_id: response_id,
                            matched,
                            phrase_relation,
                            transcript_chars,
                            phonetic_prefix_units,
                            phonetic_best_distance,
                            phonetic_best_window_start,
                            inference_ms,
                            transcript_text,
                        })
                    }
                }
                response => {
                    *process_slot = None;
                    Err(format!(
                        "local wake helper returned mismatched response: {response:?}"
                    ))
                }
            }
        }

        fn transcribe_if_ready(
            &self,
            pcm: &[u8],
            timeout: Duration,
        ) -> Result<ShadowTranscriptResult, String> {
            if pcm.is_empty() {
                return Err("local shadow ASR received empty PCM".to_string());
            }
            // Finalization is latency-sensitive. It may reuse the already-warm
            // wake helper, but must never cold-start a model or queue behind a
            // wake confirmation after the user has stopped speaking.
            let mut process_slot = self
                .process
                .try_lock()
                .ok_or_else(|| super::WAKE_HELPER_BUSY_ERROR_CODE.to_string())?;
            let Some(process) = process_slot.as_mut() else {
                return Err("local_shadow_helper_not_ready".to_string());
            };
            if !process.is_running() {
                *process_slot = None;
                return Err("local_shadow_helper_not_ready".to_string());
            }

            let wav = TempWavFile::create_without_context_padding(pcm)
                .map_err(|err| format!("prepare local shadow ASR WAV: {err:#}"))?;
            let request_id = Uuid::new_v4().to_string();
            let request = HelperRequest::Transcribe {
                request_id: request_id.clone(),
                wav_path: wav.path().to_string_lossy().into_owned(),
            };
            if let Err(err) = process.send(&request) {
                *process_slot = None;
                return Err(err);
            }
            let response = match process.receive(timeout) {
                Ok(response) => response,
                Err(err) => {
                    *process_slot = None;
                    return Err(err);
                }
            };
            match response {
                HelperResponse::Transcript {
                    request_id: response_id,
                    text,
                    inference_ms,
                    error,
                } if response_id == request_id => {
                    if let Some(error) = error {
                        Err(error)
                    } else {
                        Ok(ShadowTranscriptResult { text, inference_ms })
                    }
                }
                response => {
                    *process_slot = None;
                    Err(format!(
                        "local shadow ASR returned mismatched response: {response:?}"
                    ))
                }
            }
        }

        fn ensure_process(
            process_slot: &mut Option<HelperProcess>,
        ) -> Result<&mut HelperProcess, String> {
            let restart = process_slot
                .as_mut()
                .is_some_and(|process| !process.is_running());
            if restart {
                *process_slot = None;
            }
            if process_slot.is_none() {
                *process_slot = Some(HelperProcess::spawn()?);
            }
            process_slot
                .as_mut()
                .ok_or_else(|| "local wake helper did not start".to_string())
        }
    }

    fn client() -> &'static WakeHelperClient {
        static CLIENT: OnceLock<WakeHelperClient> = OnceLock::new();
        CLIENT.get_or_init(WakeHelperClient::default)
    }

    pub fn preload() -> Result<(), String> {
        client().preload()
    }

    pub fn confirm(
        pcm: &[u8],
        phrase: &str,
        timeout: Duration,
    ) -> Result<WakeHelperResult, String> {
        client().confirm(pcm, phrase, timeout)
    }

    pub fn transcribe_if_ready(
        pcm: &[u8],
        timeout: Duration,
    ) -> Result<ShadowTranscriptResult, String> {
        client().transcribe_if_ready(pcm, timeout)
    }

    fn emit_response(response: &HelperResponse) -> Result<(), String> {
        let payload = serde_json::to_string(response)
            .map_err(|err| format!("encode local wake helper response: {err}"))?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{payload}")
            .and_then(|_| stdout.flush())
            .map_err(|err| format!("write local wake helper response: {err}"))
    }

    fn phrase_relation_matches(relation: crate::wake_phrase::LocalPhraseRelation) -> bool {
        // KWS 的 3M zipformer 在噪声下会漏掉"开始录音",而本地确认转写的是整段候选
        // (terminal 路径)或前 1.8~3s(bounded 路径)。连续环境音里唤醒词常出现在候选
        // 中段而非开头,只认 ExactStart/PhoneticStart 会漏掉这些。这里放宽到也接受
        // PresentLater:paraformer 在候选任意位置转出完整"开始录音"即视为命中。完整
        // 4 字短语在环境台词里极少出现,且声纹门(owner_match)会挡掉非机主的声音,
        // 误触发风险可控。
        matches!(
            relation,
            crate::wake_phrase::LocalPhraseRelation::ExactStart
                | crate::wake_phrase::LocalPhraseRelation::PhoneticStart
                | crate::wake_phrase::LocalPhraseRelation::PresentLater
        )
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct RedactedPhraseDiagnostics {
        prefix_units: usize,
        best_distance: usize,
        best_window_start: usize,
    }

    fn phonetic_units(text: &str) -> Vec<String> {
        text.chars()
            .filter(|ch| ch.is_alphanumeric())
            .map(|ch| {
                ch.to_pinyin()
                    .map(|value| value.plain().to_ascii_lowercase())
                    .unwrap_or_else(|| ch.to_lowercase().collect())
            })
            .collect()
    }

    fn unit_edit_distance(actual: &[String], expected: &[String]) -> usize {
        let mut previous: Vec<usize> = (0..=expected.len()).collect();
        let mut current = vec![0usize; expected.len() + 1];
        for (actual_index, actual_unit) in actual.iter().enumerate() {
            current[0] = actual_index + 1;
            for (expected_index, expected_unit) in expected.iter().enumerate() {
                current[expected_index + 1] = (previous[expected_index + 1] + 1)
                    .min(current[expected_index] + 1)
                    .min(previous[expected_index] + usize::from(actual_unit != expected_unit));
            }
            std::mem::swap(&mut previous, &mut current);
        }
        previous[expected.len()]
    }

    fn redacted_phrase_diagnostics(text: &str, phrase: &str) -> RedactedPhraseDiagnostics {
        let actual = phonetic_units(text);
        let expected = phonetic_units(phrase);
        let prefix_units = actual
            .iter()
            .zip(&expected)
            .take_while(|(actual, expected)| actual == expected)
            .count();
        if expected.is_empty() {
            return RedactedPhraseDiagnostics {
                prefix_units,
                best_distance: 0,
                best_window_start: 0,
            };
        }
        if actual.is_empty() {
            return RedactedPhraseDiagnostics {
                prefix_units,
                best_distance: expected.len(),
                best_window_start: 0,
            };
        }

        let mut best_distance = unit_edit_distance(&actual, &expected);
        let mut best_window_start = 0usize;
        let min_window_len = expected.len().saturating_sub(1).max(1);
        let max_window_len = expected.len().saturating_add(1).min(actual.len());
        for window_len in min_window_len..=max_window_len {
            for window_start in 0..=actual.len() - window_len {
                let distance =
                    unit_edit_distance(&actual[window_start..window_start + window_len], &expected);
                if distance < best_distance
                    || (distance == best_distance && window_start < best_window_start)
                {
                    best_distance = distance;
                    best_window_start = window_start;
                }
            }
        }
        RedactedPhraseDiagnostics {
            prefix_units,
            best_distance,
            best_window_start,
        }
    }

    struct WakeTranscriptEvidence {
        text: String,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation,
        transcript_chars: usize,
        diagnostics: RedactedPhraseDiagnostics,
    }

    fn transcript_evidence(text: &str, phrase: &str) -> WakeTranscriptEvidence {
        WakeTranscriptEvidence {
            text: text.to_string(),
            phrase_relation: crate::wake_phrase::local_transcript_phrase_relation(text, phrase),
            transcript_chars: text.chars().count(),
            diagnostics: redacted_phrase_diagnostics(text, phrase),
        }
    }

    /// Near-field owner speech normally needs no enhancement. Try the raw
    /// waveform first so an already-complete wake phrase does not pay the
    /// GTCRN pass before the capsule can appear. If raw audio misses, retain
    /// the established enhanced-first decision semantics: enhanced evidence
    /// owns relation/length while the better redacted phonetic diagnostic from
    /// either pass is kept. Thus this changes latency, not acceptance policy.
    fn transcribe_wake_evidence(
        paraformer: &ParaformerRuntime,
        wav_path: &std::path::Path,
        phrase: &str,
    ) -> Result<WakeTranscriptEvidence, String> {
        if !paraformer.has_denoiser() {
            return paraformer
                .transcribe_wav(wav_path)
                .map(|text| transcript_evidence(&text, phrase));
        }

        let raw = paraformer
            .transcribe_wav_raw(wav_path)
            .ok()
            .map(|text| transcript_evidence(&text, phrase));
        if raw
            .as_ref()
            .is_some_and(|evidence| phrase_relation_matches(evidence.phrase_relation))
        {
            return Ok(raw.expect("raw wake evidence exists after match check"));
        }

        let enhanced_text = paraformer.transcribe_wav(wav_path)?;
        let mut enhanced = transcript_evidence(&enhanced_text, phrase);
        if !phrase_relation_matches(enhanced.phrase_relation) {
            if let Some(raw) = raw {
                if raw.diagnostics.best_distance < enhanced.diagnostics.best_distance {
                    enhanced.diagnostics = raw.diagnostics;
                }
            }
        }
        Ok(enhanced)
    }

    pub fn run_helper() -> i32 {
        let warmup_started = Instant::now();
        let paraformer = match ParaformerRuntime::load_cached().and_then(|runtime| {
            runtime.warm_up()?;
            Ok(runtime)
        }) {
            Ok(runtime) => {
                if emit_response(&HelperResponse::Ready {
                    model_id: Some("sherpa-onnx-paraformer-zh-small-2024-03-09".to_string()),
                    warmup_ms: warmup_started.elapsed().as_millis() as u64,
                    error: None,
                })
                .is_err()
                {
                    return 2;
                }
                runtime
            }
            Err(err) => {
                let _ = emit_response(&HelperResponse::Ready {
                    model_id: None,
                    warmup_ms: warmup_started.elapsed().as_millis() as u64,
                    error: Some(err),
                });
                return 2;
            }
        };

        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let request = match serde_json::from_str::<HelperRequest>(&line) {
                Ok(request) => request,
                Err(_) => continue,
            };
            let response = match request {
                HelperRequest::Confirm {
                    request_id,
                    wav_path,
                    phrase,
                } => {
                    let started = Instant::now();
                    let transcript = transcribe_wake_evidence(
                        &paraformer,
                        std::path::Path::new(&wav_path),
                        &phrase,
                    );
                    match transcript {
                        Ok(evidence) => {
                            let transcript_text = std::env::var(
                                "LISTENER_WAKE_DIAGNOSTIC_DIR",
                            )
                            .ok()
                            .filter(|directory| !directory.trim().is_empty())
                            .map(|_| evidence.text);
                            HelperResponse::Result {
                                request_id,
                                matched: phrase_relation_matches(evidence.phrase_relation),
                                phrase_relation: evidence.phrase_relation,
                                transcript_chars: evidence.transcript_chars,
                                phonetic_prefix_units: evidence.diagnostics.prefix_units,
                                phonetic_best_distance: evidence.diagnostics.best_distance,
                                phonetic_best_window_start: evidence.diagnostics.best_window_start,
                                inference_ms: started.elapsed().as_millis() as u64,
                                transcript_text,
                                error: None,
                            }
                        }
                        Err(err) => HelperResponse::Result {
                            request_id,
                            matched: false,
                            phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
                            transcript_chars: 0,
                            phonetic_prefix_units: 0,
                            phonetic_best_distance: 0,
                            phonetic_best_window_start: 0,
                            inference_ms: started.elapsed().as_millis() as u64,
                            transcript_text: None,
                            error: Some(format!("{err:#}")),
                        },
                    }
                }
                HelperRequest::Transcribe {
                    request_id,
                    wav_path,
                } => {
                    let started = Instant::now();
                    match paraformer.transcribe_wav_raw(std::path::Path::new(&wav_path)) {
                        Ok(text) => HelperResponse::Transcript {
                            request_id,
                            text,
                            inference_ms: started.elapsed().as_millis() as u64,
                            error: None,
                        },
                        Err(err) => HelperResponse::Transcript {
                            request_id,
                            text: String::new(),
                            inference_ms: started.elapsed().as_millis() as u64,
                            error: Some(format!("{err:#}")),
                        },
                    }
                }
            };
            if emit_response(&response).is_err() {
                break;
            }
        }
        0
    }

    #[cfg(test)]
    mod tests {
        use std::collections::BTreeMap;
        use std::fs;
        use std::path::PathBuf;
        use std::time::Duration;

        use sha2::Digest;

        use super::{
            phrase_relation_matches, redacted_phrase_diagnostics, HelperRequest, HelperResponse,
            WakeHelperClient,
        };

        #[test]
        fn confirmation_returns_busy_instead_of_queueing_on_the_helper() {
            let client = WakeHelperClient::default();
            let _active_request = client.process.lock();
            let error = client
                .confirm(&[0, 0], "开始录音", Duration::from_millis(1))
                .expect_err("a second confirmation must not queue");
            assert!(super::super::is_busy_error(&error));
        }

        #[test]
        fn wake_confirmation_protocol_exposes_match_metadata_without_transcript_text() {
            let request = HelperRequest::Confirm {
                request_id: "request-1".into(),
                wav_path: "input.wav".into(),
                phrase: "开始录音".into(),
            };
            let response = HelperResponse::Result {
                request_id: "request-1".into(),
                matched: true,
                phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
                transcript_chars: 4,
                phonetic_prefix_units: 4,
                phonetic_best_distance: 0,
                phonetic_best_window_start: 0,
                inference_ms: 123,
                transcript_text: None,
                error: None,
            };

            let request_json = serde_json::to_string(&request).unwrap();
            let response_json = serde_json::to_string(&response).unwrap();
            assert!(request_json.contains("开始录音"));
            assert!(!response_json.contains("开始录音"));
            assert!(!response_json.contains("transcript_text"));
        }

        #[test]
        fn shadow_transcript_protocol_is_separate_from_wake_confirmation() {
            let request = HelperRequest::Transcribe {
                request_id: "request-2".into(),
                wav_path: "input.wav".into(),
            };
            let response = HelperResponse::Transcript {
                request_id: "request-2".into(),
                text: "只用于本机终稿漏字校验".into(),
                inference_ms: 321,
                error: None,
            };

            let request_json = serde_json::to_string(&request).unwrap();
            let response_json = serde_json::to_string(&response).unwrap();
            assert!(request_json.contains("transcribe"));
            assert!(response_json.contains("只用于本机终稿漏字校验"));
        }

        #[test]
        fn redacted_phrase_metrics_locate_crops_near_homophones_and_leading_speech() {
            let exact = redacted_phrase_diagnostics("开始录音", "开始录音");
            assert_eq!(
                (
                    exact.prefix_units,
                    exact.best_distance,
                    exact.best_window_start
                ),
                (4, 0, 0)
            );

            let leading = redacted_phrase_diagnostics("请说开使录因", "开始录音");
            assert_eq!(leading.prefix_units, 0);
            assert_eq!(leading.best_distance, 0);
            assert_eq!(leading.best_window_start, 2);

            let cropped = redacted_phrase_diagnostics("始录音", "开始录音");
            assert_eq!(cropped.prefix_units, 0);
            assert_eq!(cropped.best_distance, 1);
            assert_eq!(cropped.best_window_start, 0);

            let near = redacted_phrase_diagnostics("开始录像", "开始录音");
            assert_eq!(near.prefix_units, 3);
            assert_eq!(near.best_distance, 1);
        }

        #[test]
        fn near_field_wake_checks_raw_audio_before_denoiser_fallback() {
            let source = include_str!("wake_helper.rs");
            let helper = source
                .split("fn transcribe_wake_evidence")
                .nth(1)
                .and_then(|tail| tail.split("pub fn run_helper").next())
                .expect("wake evidence helper source");
            let raw = helper
                .find(".transcribe_wav_raw(wav_path)")
                .expect("raw pass");
            let enhanced = helper
                .find(".transcribe_wav(wav_path)?")
                .expect("enhanced pass");
            assert!(raw < enhanced, "near-field raw pass must stay first");
        }

        #[test]
        fn helper_protocol_accepts_phrase_present_anywhere_in_transcript() {
            use crate::wake_phrase::LocalPhraseRelation;

            for (relation, expected) in [
                (LocalPhraseRelation::ExactStart, true),
                (LocalPhraseRelation::PhoneticStart, true),
                (LocalPhraseRelation::PresentLater, true),
                (LocalPhraseRelation::Absent, false),
            ] {
                assert_eq!(phrase_relation_matches(relation), expected);
            }
        }

        #[test]
        #[ignore = "diagnostic: scan LISTENER_WAKE_DIAG_DIR WAV transcripts"]
        fn diagnostic_scan_local_wake_transcripts() {
            let dir = PathBuf::from(
                std::env::var("LISTENER_WAKE_DIAG_DIR").expect("LISTENER_WAKE_DIAG_DIR"),
            );
            let runtime =
                super::ParaformerRuntime::load_cached().expect("load cached Paraformer runtime");
            let mut paths = fs::read_dir(dir)
                .expect("diagnostic directory")
                .map(|entry| entry.expect("directory entry").path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("wav"))
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                let text = runtime
                    .transcribe_wav(&path)
                    .expect("transcribe diagnostic WAV");
                println!(
                    "local_transcript file={} text={text:?}",
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("<invalid>")
                );
            }
        }

        #[test]
        #[ignore = "diagnostic: verify LISTENER_WAKE_DIAG_DIR contains full-length negative WAVs"]
        fn diagnostic_secondary_negative_gate() {
            let phrase =
                std::env::var("LISTENER_WAKE_PHRASE").unwrap_or_else(|_| "开始录音".to_string());
            let dir = PathBuf::from(
                std::env::var("LISTENER_WAKE_DIAG_DIR").expect("LISTENER_WAKE_DIAG_DIR"),
            );
            let runtime =
                super::ParaformerRuntime::load_cached().expect("load cached Paraformer runtime");
            let mut paths = fs::read_dir(dir)
                .expect("diagnostic directory")
                .map(|entry| entry.expect("directory entry").path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("wav"))
                .collect::<Vec<_>>();
            paths.sort();
            assert!(
                !paths.is_empty(),
                "secondary negative gate needs WAV fixtures"
            );
            for path in paths {
                let text = runtime
                    .transcribe_wav(&path)
                    .expect("transcribe diagnostic WAV");
                let relation = denzic_voice_activation_v1_core::local_transcript_phrase_relation(
                    &text, &phrase,
                );
                let decision = denzic_voice_activation_v1_core::decide_completed_secondary(
                    denzic_voice_activation_v1_core::CompletedSecondaryInput {
                        relation,
                        transcript_chars: text.chars().filter(|ch| ch.is_alphanumeric()).count(),
                        phrase_chars: phrase.chars().filter(|ch| ch.is_alphanumeric()).count(),
                    },
                );
                println!(
                    "secondary_negative file={} text={text:?} relation={relation:?} decision={decision:?}",
                    path.file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("<invalid>")
                );
                assert_eq!(
                    decision,
                    denzic_voice_activation_v1_core::CompletedSecondaryDecision::RejectExplicitAbsent,
                    "secondary verifier must reject {}",
                    path.display()
                );
            }
        }

        fn wav_pcm_data(path: &std::path::Path) -> Vec<u8> {
            let wav = fs::read(path).expect("matrix WAV");
            assert!(wav.len() >= 12 && &wav[0..4] == b"RIFF" && &wav[8..12] == b"WAVE");
            let mut offset = 12usize;
            while offset + 8 <= wav.len() {
                let chunk_id = &wav[offset..offset + 4];
                let chunk_len = u32::from_le_bytes(
                    wav[offset + 4..offset + 8]
                        .try_into()
                        .expect("WAV chunk length"),
                ) as usize;
                let data_start = offset + 8;
                let data_end = data_start.saturating_add(chunk_len).min(wav.len());
                if chunk_id == b"data" {
                    return wav[data_start..data_end].to_vec();
                }
                offset = data_start.saturating_add(chunk_len + (chunk_len & 1));
            }
            panic!("WAV data chunk missing: {}", path.display());
        }

        fn product_confirmation_windows(pcm_ms: usize) -> Vec<(usize, usize)> {
            let mut windows = Vec::new();
            for end_ms in [800usize, 1_400, 1_600, 2_000] {
                if end_ms <= pcm_ms {
                    windows.push((0, end_ms));
                }
            }
            // Production rotates at about 2.4 s with 1.4 s overlap, then every
            // ~1.0 s. Each advanced origin owns exactly one focused attempt.
            for end_ms in [2_450usize, 3_470, 4_490] {
                if end_ms <= pcm_ms {
                    windows.push((end_ms - 1_400, end_ms));
                }
            }
            windows
        }

        fn evaluate_local_window(
            runtime: &super::ParaformerRuntime,
            pcm: &[u8],
            phrase: &str,
            phrase_chars: usize,
        ) -> (
            denzic_voice_activation_v1_core::CompletedSecondaryDecision,
            crate::wake_phrase::LocalPhraseRelation,
            usize,
            u64,
        ) {
            let boosted = crate::wake_phrase::gain_normalized_pcm16(pcm);
            let wav = super::TempWavFile::create_without_context_padding(&boosted)
                .expect("prepare matrix local-confirm WAV");
            let started = std::time::Instant::now();
            let evidence = super::transcribe_wake_evidence(runtime, wav.path(), phrase)
                .expect("matrix local ASR");
            let relation = evidence.phrase_relation;
            let transcript_chars = evidence.transcript_chars;
            let decision = denzic_voice_activation_v1_core::decide_completed_secondary(
                denzic_voice_activation_v1_core::CompletedSecondaryInput {
                    relation,
                    transcript_chars,
                    phrase_chars,
                },
            );
            (
                decision,
                relation,
                transcript_chars,
                started.elapsed().as_millis() as u64,
            )
        }

        fn percentile_95(values: &[u64]) -> u64 {
            let mut sorted = values.to_vec();
            sorted.sort_unstable();
            sorted[((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)]
        }

        const DIAGNOSTIC_KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 60;

        #[derive(Clone, Copy)]
        struct DiagnosticConfirmation {
            origin_ms: u64,
            end_ms: u64,
            relation: crate::wake_phrase::LocalPhraseRelation,
            transcript_chars: usize,
            inference_ms: u64,
            accepts: bool,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct DiagnosticGateDecision {
            decision_ms: u64,
            snapshot_audio_ms: u64,
            accounted_inference_ms: u64,
            path: &'static str,
            keyword_fallback_ms: Option<u64>,
        }

        fn model_product_gate_decision(
            confirmations: &[DiagnosticConfirmation],
            phrase_chars: usize,
            kws_observed_audio_ms: Option<u64>,
            kws_end_ms: Option<u64>,
        ) -> Option<DiagnosticGateDecision> {
            let mut worker_available_ms = 0u64;
            let scheduled = confirmations
                .iter()
                .map(|confirmation| {
                    let started_ms = confirmation.end_ms.max(worker_available_ms);
                    let completed_ms = started_ms.saturating_add(confirmation.inference_ms);
                    worker_available_ms = completed_ms;
                    (*confirmation, started_ms, completed_ms)
                })
                .collect::<Vec<_>>();

            let local = scheduled
                .iter()
                .filter(|(confirmation, _, _)| confirmation.accepts)
                .min_by_key(|(_, _, completed_ms)| *completed_ms)
                .map(|(confirmation, _, completed_ms)| DiagnosticGateDecision {
                    decision_ms: *completed_ms,
                    snapshot_audio_ms: confirmation.end_ms,
                    accounted_inference_ms: completed_ms.saturating_sub(confirmation.end_ms),
                    path: "LocalTranscript",
                    keyword_fallback_ms: None,
                });

            let keyword = kws_observed_audio_ms.zip(kws_end_ms).and_then(
                |(observed_ms, keyword_end_ms)| {
                    let in_flight_age_ms = scheduled
                        .iter()
                        .find(|(_, started_ms, completed_ms)| {
                            *started_ms <= observed_ms && observed_ms < *completed_ms
                        })
                        .map(|(_, started_ms, _)| observed_ms.saturating_sub(*started_ms))
                        .unwrap_or(0);
                    let remaining_budget_ms = DIAGNOSTIC_KWS_SECONDARY_CONFIRM_BUDGET_MS
                        .saturating_sub(
                            in_flight_age_ms.min(DIAGNOSTIC_KWS_SECONDARY_CONFIRM_BUDGET_MS),
                        );
                    let fallback_ms = observed_ms.saturating_add(remaining_budget_ms);
                    let explicit_absent_covers_hit = scheduled.iter().any(
                        |(confirmation, _, completed_ms)| {
                            *completed_ms <= fallback_ms
                                && confirmation.origin_ms == 0
                                && confirmation.end_ms >= keyword_end_ms
                                && matches!(
                                    denzic_voice_activation_v1_core::decide_completed_secondary(
                                        denzic_voice_activation_v1_core::CompletedSecondaryInput {
                                            relation: confirmation.relation,
                                            transcript_chars: confirmation.transcript_chars,
                                            phrase_chars,
                                        },
                                    ),
                                    denzic_voice_activation_v1_core::CompletedSecondaryDecision::RejectExplicitAbsent
                                )
                        },
                    );
                    (!explicit_absent_covers_hit).then_some(DiagnosticGateDecision {
                        decision_ms: fallback_ms,
                        snapshot_audio_ms: observed_ms,
                        accounted_inference_ms: remaining_budget_ms,
                        path: "KeywordModel",
                        keyword_fallback_ms: Some(fallback_ms),
                    })
                },
            );

            [local, keyword]
                .into_iter()
                .flatten()
                .min_by_key(|decision| decision.decision_ms)
        }

        #[test]
        fn diagnostic_gate_models_pre_hit_budget_and_non_authoritative_partial() {
            use crate::wake_phrase::LocalPhraseRelation::Absent;
            let confirmations = [
                DiagnosticConfirmation {
                    origin_ms: 0,
                    end_ms: 800,
                    relation: Absent,
                    transcript_chars: 4,
                    inference_ms: 160,
                    accepts: false,
                },
                DiagnosticConfirmation {
                    origin_ms: 0,
                    end_ms: 1_400,
                    relation: Absent,
                    transcript_chars: 1,
                    inference_ms: 205,
                    accepts: false,
                },
                DiagnosticConfirmation {
                    origin_ms: 0,
                    end_ms: 1_750,
                    relation: Absent,
                    transcript_chars: 3,
                    inference_ms: 261,
                    accepts: false,
                },
                DiagnosticConfirmation {
                    origin_ms: 0,
                    end_ms: 2_000,
                    relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
                    transcript_chars: 4,
                    inference_ms: 245,
                    accepts: true,
                },
            ];
            assert_eq!(
                model_product_gate_decision(&confirmations, 4, Some(2_070), Some(1_720)),
                Some(DiagnosticGateDecision {
                    decision_ms: 2_071,
                    snapshot_audio_ms: 2_070,
                    accounted_inference_ms: 1,
                    path: "KeywordModel",
                    keyword_fallback_ms: Some(2_071),
                }),
                "the 3/4-character window must not veto KWS; the in-flight task already spent 59 ms",
            );
        }

        #[test]
        #[ignore = "diagnostic: evaluate LISTENER_WAKE_MATRIX_DIR manifest and WAVs"]
        fn diagnostic_mature_wake_product_matrix() {
            let dir = PathBuf::from(
                std::env::var("LISTENER_WAKE_MATRIX_DIR").expect("LISTENER_WAKE_MATRIX_DIR"),
            );
            let manifest_path = dir.join("manifest.json");
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest_path).expect("wake matrix manifest"))
                    .expect("valid wake matrix manifest");
            assert_eq!(
                manifest["schema"], "listener.mature-wake-matrix.v1",
                "unexpected wake matrix schema"
            );
            let phrase = manifest["phrase"].as_str().expect("matrix phrase");
            let mut metadata = BTreeMap::<String, (String, String)>::new();
            let mut positive_boundaries = BTreeMap::<String, (f64, f64)>::new();
            let mut speeds = Vec::new();
            let mut crops = Vec::new();
            let mut distances = Vec::new();
            for case in manifest["cases"].as_array().expect("matrix cases") {
                let path = PathBuf::from(case["path"].as_str().expect("case path"));
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("case file name")
                    .to_string();
                let kind = case["kind"].as_str().expect("case kind").to_string();
                let category = case["category"]
                    .as_str()
                    .expect("case category")
                    .to_string();
                metadata.insert(name.clone(), (kind.clone(), category));
                if kind == "positive" {
                    speeds.push(case["speed"].as_f64().expect("positive speed"));
                    crops.push(case["start_crop_ms"].as_u64().expect("positive start crop"));
                    distances.push(case["distance_m"].as_f64().expect("positive distance"));
                    positive_boundaries.insert(
                        name,
                        (
                            case["wake_phrase_start_ms"]
                                .as_f64()
                                .expect("wake phrase start"),
                            case["wake_phrase_end_ms"]
                                .as_f64()
                                .expect("wake phrase end"),
                        ),
                    );
                }
            }
            assert!(speeds.iter().any(|value| *value <= 0.8));
            assert!(speeds.iter().any(|value| *value >= 1.25));
            assert!(crops.contains(&0) && crops.contains(&200));
            assert!(distances.iter().any(|value| *value <= 0.3));
            assert!(distances.iter().any(|value| *value >= 1.0));

            let runtime =
                super::ParaformerRuntime::load_cached().expect("load cached Paraformer runtime");
            let mut paths = fs::read_dir(&dir)
                .expect("matrix directory")
                .map(|entry| entry.expect("matrix entry").path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("wav"))
                .collect::<Vec<_>>();
            paths.sort();

            let phrase_chars = phrase.chars().filter(|ch| ch.is_alphanumeric()).count();
            let mut positive_total = 0usize;
            let mut positive_accept = 0usize;
            let mut negative_total = 0usize;
            let mut negative_accept = 0usize;
            let mut categories = BTreeMap::<String, (usize, usize)>::new();
            let mut wake_start_latencies = Vec::<u64>::new();
            let mut phrase_tail_latencies = Vec::<u64>::new();
            let mut rows = Vec::<serde_json::Value>::new();
            for path in paths {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("matrix WAV name");
                let (kind, category) = metadata.get(name).expect("manifest row for WAV");
                let pcm = wav_pcm_data(&path);
                let mut detector = crate::wake_phrase::StreamingDetector::new(phrase)
                    .expect("product-sensitive streaming detector");
                let mut kws_match = None;
                let mut kws_observed_audio_ms = None;
                for (index, chunk) in pcm.chunks(320).enumerate() {
                    if let Some(found) = detector.accept_pcm(chunk).expect("streaming KWS") {
                        kws_match = Some(found);
                        kws_observed_audio_ms = Some(((index * 320 + chunk.len()) / 32) as u64);
                        break;
                    }
                }
                if kws_match.is_none() {
                    kws_match = detector.finish().expect("finish streaming KWS");
                }
                let mut accepted_audio_ms = None;
                let mut accepted_inference_ms = None;
                let mut last_relation = crate::wake_phrase::LocalPhraseRelation::Absent;
                let mut last_transcript_chars = 0usize;
                let mut total_inference_ms = 0u64;
                let mut window_rows = Vec::new();
                let mut confirmations = Vec::new();
                for (origin_ms, end_ms) in product_confirmation_windows(pcm.len() / 32) {
                    let start = origin_ms * 32;
                    let end = (end_ms * 32).min(pcm.len());
                    let (decision, relation, transcript_chars, inference_ms) =
                        evaluate_local_window(&runtime, &pcm[start..end], phrase, phrase_chars);
                    total_inference_ms = total_inference_ms.saturating_add(inference_ms);
                    last_relation = relation;
                    last_transcript_chars = transcript_chars;
                    let window_accept = matches!(
                        decision,
                        denzic_voice_activation_v1_core::CompletedSecondaryDecision::AcceptLocalTranscript
                    );
                    confirmations.push(DiagnosticConfirmation {
                        origin_ms: origin_ms as u64,
                        end_ms: end_ms as u64,
                        relation,
                        transcript_chars,
                        inference_ms,
                        accepts: window_accept,
                    });
                    window_rows.push(serde_json::json!({
                        "origin_ms": origin_ms,
                        "end_ms": end_ms,
                        "relation": format!("{relation:?}"),
                        "transcript_chars": transcript_chars,
                        "inference_ms": inference_ms,
                        "decision": format!("{decision:?}"),
                    }));
                    if window_accept {
                        accepted_audio_ms = Some(end_ms as u64);
                        accepted_inference_ms = Some(inference_ms);
                        break;
                    }
                }
                // Production starts a focused stage-2 confirmation as soon as
                // streaming KWS is observed, even between fixed exploratory
                // snapshots. Include that path and select the earliest valid
                // completed decision. Explicit Absent is still authoritative;
                // this is a full-phrase verifier pass, not bare-KWS acceptance.
                let mut kws_confirmation = None;
                if let Some(kws_audio_ms) = kws_observed_audio_ms {
                    let end = (kws_audio_ms as usize * 32).min(pcm.len());
                    let (decision, relation, transcript_chars, inference_ms) =
                        evaluate_local_window(&runtime, &pcm[..end], phrase, phrase_chars);
                    let kws_accept = matches!(
                        decision,
                        denzic_voice_activation_v1_core::CompletedSecondaryDecision::AcceptLocalTranscript
                    );
                    kws_confirmation = Some(serde_json::json!({
                        "audio_ms": kws_audio_ms,
                        "relation": format!("{relation:?}"),
                        "transcript_chars": transcript_chars,
                        "inference_ms": inference_ms,
                        "decision": format!("{decision:?}"),
                    }));
                    if kws_accept
                        && accepted_audio_ms
                            .zip(accepted_inference_ms)
                            .map(|(audio_ms, local_ms)| {
                                kws_audio_ms.saturating_add(inference_ms)
                                    < audio_ms.saturating_add(local_ms)
                            })
                            .unwrap_or(true)
                    {
                        last_relation = relation;
                        last_transcript_chars = transcript_chars;
                    }
                }
                let kws_end_ms = kws_match
                    .as_ref()
                    .map(|value| (value.end_seconds * 1000.0).round() as u64);
                let modeled_gate = model_product_gate_decision(
                    &confirmations,
                    phrase_chars,
                    kws_observed_audio_ms,
                    kws_end_ms,
                );
                let accepted = modeled_gate.is_some();
                let accepted_audio_ms = modeled_gate.map(|decision| decision.snapshot_audio_ms);
                let accepted_inference_ms =
                    modeled_gate.map(|decision| decision.accounted_inference_ms);
                if kind == "positive" {
                    positive_total += 1;
                    positive_accept += usize::from(accepted);
                    let row = categories.entry(category.clone()).or_default();
                    row.0 += 1;
                    row.1 += usize::from(accepted);
                    if let (Some(audio_ms), Some(inference_ms), Some((phrase_start, phrase_end))) = (
                        accepted_audio_ms,
                        accepted_inference_ms,
                        positive_boundaries.get(name),
                    ) {
                        let decision_ms = modeled_gate
                            .map(|decision| decision.decision_ms)
                            .unwrap_or_else(|| audio_ms.saturating_add(inference_ms));
                        wake_start_latencies
                            .push((decision_ms as f64 - phrase_start).max(0.0).round() as u64);
                        phrase_tail_latencies
                            .push((decision_ms as f64 - phrase_end).max(0.0).round() as u64);
                    }
                } else {
                    negative_total += 1;
                    negative_accept += usize::from(accepted);
                }
                rows.push(serde_json::json!({
                    "file": name,
                    "kind": kind,
                    "category": category,
                    "kws_hit": kws_match.is_some(),
                    "kws_end_ms": kws_end_ms,
                    "kws_observed_audio_ms": kws_observed_audio_ms,
                    "local_relation": format!("{last_relation:?}"),
                    "local_transcript_chars": last_transcript_chars,
                    "local_total_inference_ms": total_inference_ms,
                    "accepted_audio_ms": accepted_audio_ms,
                    "accepted_inference_ms": accepted_inference_ms,
                    "gate_decision_ms": modeled_gate.map(|decision| decision.decision_ms),
                    "gate_path": modeled_gate.map(|decision| decision.path),
                    "keyword_fallback_ms": modeled_gate.and_then(|decision| decision.keyword_fallback_ms),
                    "confirmation_windows": window_rows,
                    "kws_confirmation": kws_confirmation,
                    "gate_decision": if accepted { "Accept" } else { "Reject" },
                    "owner_gate": "OpenUnenrolled",
                    "visible_side_effect": accepted,
                }));
            }

            assert!(
                positive_total >= 40,
                "positive matrix requires at least 40 cases"
            );
            assert!(
                negative_total >= 100,
                "negative matrix requires at least 100 cases"
            );
            let overall_recall = positive_accept as f64 / positive_total as f64;
            assert_eq!(wake_start_latencies.len(), positive_accept);
            assert_eq!(phrase_tail_latencies.len(), positive_accept);
            let wake_start_p95_ms = percentile_95(&wake_start_latencies);
            let wake_start_max_ms = *wake_start_latencies.iter().max().expect("wake latency");
            let phrase_tail_p95_ms = percentile_95(&phrase_tail_latencies);
            let phrase_tail_max_ms = *phrase_tail_latencies.iter().max().expect("tail latency");
            let mut category_report = serde_json::Map::new();
            for (category, (total, accepted)) in &categories {
                let recall = *accepted as f64 / *total as f64;
                category_report.insert(
                    category.clone(),
                    serde_json::json!({"accepted": accepted, "total": total, "recall": recall}),
                );
                assert!(
                    recall >= 0.90,
                    "category {category} recall {recall:.3} < 0.90"
                );
            }
            let report = serde_json::json!({
                "schema": "listener.mature-wake-report.v1",
                "manifest_sha256": format!("{:x}", sha2::Sha256::digest(fs::read(&manifest_path).expect("manifest bytes"))),
                "positive": {"accepted": positive_accept, "total": positive_total, "recall": overall_recall},
                "categories": category_report,
                "negative": {"accepted": negative_accept, "total": negative_total},
                "latency": {
                    "wake_start_to_decision_p95_ms": wake_start_p95_ms,
                    "wake_start_to_decision_max_ms": wake_start_max_ms,
                    "phrase_tail_to_decision_p95_ms": phrase_tail_p95_ms,
                    "phrase_tail_to_decision_max_ms": phrase_tail_max_ms,
                },
                "rows": rows,
            });
            fs::write(
                dir.join("evaluation-report.json"),
                serde_json::to_vec_pretty(&report).expect("serialize wake report"),
            )
            .expect("write wake report");
            assert!(
                overall_recall >= 0.95,
                "overall recall {overall_recall:.3} < 0.95"
            );
            assert_eq!(
                negative_accept, 0,
                "negative matrix produced visible accepts"
            );
            assert!(
                wake_start_p95_ms <= 1_000 && wake_start_max_ms <= 1_200,
                "wake-start latency p95={wake_start_p95_ms} max={wake_start_max_ms} exceeds 1000/1200 ms"
            );
            assert!(
                phrase_tail_p95_ms <= 350 && phrase_tail_max_ms <= 500,
                "phrase-tail latency p95={phrase_tail_p95_ms} max={phrase_tail_max_ms} exceeds 350/500 ms"
            );
            println!(
                "mature_wake_matrix positive={positive_accept}/{positive_total} recall={overall_recall:.3} negative_accept={negative_accept}/{negative_total} wake_start_p95_ms={wake_start_p95_ms} phrase_tail_p95_ms={phrase_tail_p95_ms} report={}",
                dir.join("evaluation-report.json").display()
            );
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub use imp::{
    confirm, preload, run_helper, transcribe_if_ready, ShadowTranscriptResult, WakeHelperResult,
};

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeHelperResult {
    pub request_id: String,
    pub matched: bool,
    pub phrase_relation: crate::wake_phrase::LocalPhraseRelation,
    pub transcript_chars: usize,
    pub phonetic_prefix_units: usize,
    pub phonetic_best_distance: usize,
    pub phonetic_best_window_start: usize,
    pub inference_ms: u64,
    pub transcript_text: Option<String>,
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowTranscriptResult {
    pub text: String,
    pub inference_ms: u64,
}

#[cfg(not(target_os = "windows"))]
pub fn preload() -> Result<(), String> {
    Err("local wake helper is only available on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn confirm(
    _pcm: &[u8],
    _phrase: &str,
    _timeout: std::time::Duration,
) -> Result<WakeHelperResult, String> {
    Err("local wake helper is only available on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn transcribe_if_ready(
    _pcm: &[u8],
    _timeout: std::time::Duration,
) -> Result<ShadowTranscriptResult, String> {
    Err("local shadow ASR is only available on Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn run_helper() -> i32 {
    2
}
