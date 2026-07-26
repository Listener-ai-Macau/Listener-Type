#[cfg(target_os = "windows")]
mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    use super::super::foundry_provider::TempWavFile;
    use super::super::paraformer::ParaformerRuntime;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const HELPER_READY_TIMEOUT: Duration = Duration::from_secs(30);
    const HELPER_AUDIO_TIMEOUT: Duration = Duration::from_secs(4);

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct WakeHelperResult {
        pub matched: bool,
        pub phrase_relation: crate::wake_phrase::LocalPhraseRelation,
        pub transcript_chars: usize,
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
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum HelperResponse {
        Ready {
            model_id: Option<String>,
            error: Option<String>,
        },
        Result {
            request_id: String,
            matched: bool,
            phrase_relation: crate::wake_phrase::LocalPhraseRelation,
            transcript_chars: usize,
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
            let executable = std::env::current_exe()
                .map_err(|err| format!("resolve Listener Type executable: {err}"))?;
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
                    model_id: Some(_),
                    error: None,
                } => Ok(process),
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
            let wav = TempWavFile::create_without_context_padding(pcm)
                .map_err(|err| format!("prepare local wake helper WAV: {err:#}"))?;
            let request_id = Uuid::new_v4().to_string();
            let request = HelperRequest::Confirm {
                request_id: request_id.clone(),
                wav_path: wav.path().to_string_lossy().into_owned(),
                phrase: phrase.to_string(),
            };

            let mut process_slot = self.process.lock();
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
                    inference_ms,
                    error,
                } if response_id == request_id => {
                    if let Some(error) = error {
                        Err(error)
                    } else {
                        Ok(WakeHelperResult {
                            matched,
                            phrase_relation,
                            transcript_chars,
                            inference_ms,
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

    pub fn run_helper() -> i32 {
        let paraformer = match ParaformerRuntime::load_cached() {
            Ok(runtime) => {
                if emit_response(&HelperResponse::Ready {
                    model_id: Some("sherpa-onnx-paraformer-zh-small-2024-03-09".to_string()),
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
            let HelperRequest::Confirm {
                request_id,
                wav_path,
                phrase,
            } = request;
            let started = Instant::now();
            let transcript = paraformer.transcribe_wav(std::path::Path::new(&wav_path));
            let response = match transcript {
                Ok(text) => {
                    let phrase_relation =
                        crate::wake_phrase::local_transcript_phrase_relation(&text, &phrase);
                    HelperResponse::Result {
                        request_id,
                        matched: phrase_relation_matches(phrase_relation),
                        phrase_relation,
                        transcript_chars: text.chars().count(),
                        inference_ms: started.elapsed().as_millis() as u64,
                        error: None,
                    }
                }
                Err(err) => HelperResponse::Result {
                    request_id,
                    matched: false,
                    phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
                    transcript_chars: 0,
                    inference_ms: started.elapsed().as_millis() as u64,
                    error: Some(format!("{err:#}")),
                },
            };
            if emit_response(&response).is_err() {
                break;
            }
        }
        0
    }

    #[cfg(test)]
    mod tests {
        use super::{phrase_relation_matches, HelperRequest, HelperResponse};

        #[test]
        fn helper_protocol_exposes_match_metadata_without_transcript_text() {
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
                inference_ms: 123,
                error: None,
            };

            let request_json = serde_json::to_string(&request).unwrap();
            let response_json = serde_json::to_string(&response).unwrap();
            assert!(request_json.contains("开始录音"));
            assert!(!response_json.contains("开始录音"));
            assert!(!response_json.contains("transcript_text"));
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
    }
}

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub use imp::{confirm, preload, run_helper, WakeHelperResult};

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeHelperResult {
    pub matched: bool,
    pub phrase_relation: crate::wake_phrase::LocalPhraseRelation,
    pub transcript_chars: usize,
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
pub fn run_helper() -> i32 {
    2
}
