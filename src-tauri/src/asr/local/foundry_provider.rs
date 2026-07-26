#![allow(dead_code)] // Task 6 接入 coordinator 后这些路径会变成运行时路径。

#[cfg(target_os = "windows")]
use std::fs::{self, OpenOptions};
#[cfg(target_os = "windows")]
use std::io::Write;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use anyhow::Context;
use anyhow::Result;
use parking_lot::Mutex;
#[cfg(target_os = "windows")]
use uuid::Uuid;

use crate::asr::wav::{append_tail_silence_16k_mono, encode_wav_16k_mono};
use crate::asr::RawTranscript;

#[cfg(target_os = "windows")]
use super::foundry_runtime::FoundryLocalRuntime;

const FOUNDRY_LEAD_SILENCE_PADDING_MS: usize = 250;
const FOUNDRY_LEAD_SILENCE_PADDING_SAMPLES: usize =
    16_000 * FOUNDRY_LEAD_SILENCE_PADDING_MS / 1_000;
const FOUNDRY_MIN_AUDIO_CONTEXT_MS: usize = 4_000;
const FOUNDRY_MIN_AUDIO_CONTEXT_SAMPLES: usize = 16_000 * FOUNDRY_MIN_AUDIO_CONTEXT_MS / 1_000;

pub struct FoundryLocalWhisperAsr {
    #[cfg(target_os = "windows")]
    runtime: Arc<FoundryLocalRuntime>,
    model_alias: String,
    runtime_source: String,
    language_hint: Option<String>,
    buffer: Mutex<Vec<u8>>,
    cancel_generation: AtomicU64,
}

impl FoundryLocalWhisperAsr {
    #[cfg(target_os = "windows")]
    pub fn new(
        runtime: Arc<FoundryLocalRuntime>,
        model_alias: String,
        runtime_source: String,
        language_hint: Option<String>,
    ) -> Self {
        Self {
            runtime,
            model_alias,
            runtime_source,
            language_hint: normalize_language_hint(language_hint),
            buffer: Mutex::new(Vec::new()),
            cancel_generation: AtomicU64::new(0),
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn new(model_alias: String, language_hint: Option<String>) -> Self {
        Self {
            model_alias,
            runtime_source: "auto".into(),
            language_hint: normalize_language_hint(language_hint),
            buffer: Mutex::new(Vec::new()),
            cancel_generation: AtomicU64::new(0),
        }
    }

    pub fn model_alias(&self) -> &str {
        &self.model_alias
    }

    pub fn language_hint(&self) -> Option<&str> {
        self.language_hint.as_deref()
    }

    pub async fn transcribe(&self, audio_timeout: std::time::Duration) -> Result<RawTranscript> {
        let cancel_generation = self.cancel_generation.load(Ordering::SeqCst);
        let pcm = self.buffer.lock().clone();
        if pcm.is_empty() {
            return Ok(RawTranscript {
                text: String::new(),
                duration_ms: 0,
            });
        }

        let result = self.transcribe_inner(&pcm, audio_timeout).await;
        if self.cancel_generation.load(Ordering::SeqCst) != cancel_generation {
            anyhow::bail!("Foundry Local Whisper transcription cancelled");
        }
        if foundry_transcribe_attempt_consumes_buffer(&result) {
            self.buffer.lock().clear();
        }
        result
    }

    async fn transcribe_inner(
        &self,
        pcm: &[u8],
        audio_timeout: std::time::Duration,
    ) -> Result<RawTranscript> {
        #[cfg(target_os = "windows")]
        let duration_ms = pcm_duration_ms(pcm);

        #[cfg(not(target_os = "windows"))]
        {
            let _ = pcm;
            let _ = audio_timeout;
            anyhow::bail!(
                "Foundry Local Whisper is only available on Windows: {}",
                self.model_alias
            );
        }

        #[cfg(target_os = "windows")]
        {
            let wav_file = TempWavFile::create(pcm)?;
            let text = self
                .runtime
                .transcribe_audio_file(
                    &self.model_alias,
                    &self.runtime_source,
                    self.language_hint(),
                    wav_file.path(),
                    audio_timeout,
                )
                .await
                .with_context(|| {
                    format!(
                        "transcribe audio file with Foundry Local Whisper model {}",
                        self.model_alias
                    )
                })?;

            Ok(RawTranscript {
                text: trim_transcript_text(&text),
                duration_ms,
            })
        }
    }

    pub fn cancel(&self) {
        self.cancel_generation.fetch_add(1, Ordering::SeqCst);
        #[cfg(target_os = "windows")]
        self.runtime.request_cancel_prepare();
        self.buffer.lock().clear();
    }
}

impl crate::recorder::AudioConsumer for FoundryLocalWhisperAsr {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.buffer.lock().extend_from_slice(pcm);
    }
}

#[cfg(target_os = "windows")]
pub(crate) async fn transcribe_cached_pcm(
    runtime: Arc<FoundryLocalRuntime>,
    model_alias: &str,
    language_hint: Option<&str>,
    pcm: &[u8],
    audio_timeout: std::time::Duration,
) -> Result<RawTranscript> {
    if pcm.is_empty() {
        return Ok(RawTranscript {
            text: String::new(),
            duration_ms: 0,
        });
    }
    let wav_file = TempWavFile::create(pcm)?;
    let text = runtime
        .transcribe_cached_audio_file(model_alias, language_hint, wav_file.path(), audio_timeout)
        .await
        .with_context(|| format!("transcribe cached PCM with Foundry Local model {model_alias}"))?;
    Ok(RawTranscript {
        text: trim_transcript_text(&text),
        duration_ms: pcm_duration_ms(pcm),
    })
}

fn pcm_duration_ms(pcm: &[u8]) -> u64 {
    (pcm.len() as u64 / 2) * 1000 / 16_000
}

fn pcm_to_wav_with_foundry_context_padding(pcm: &[u8]) -> Vec<u8> {
    let pcm_samples = pcm.chunks_exact(2).len();
    let mut samples: Vec<i16> = Vec::with_capacity(
        FOUNDRY_MIN_AUDIO_CONTEXT_SAMPLES.max(FOUNDRY_LEAD_SILENCE_PADDING_SAMPLES + pcm_samples),
    );
    samples.extend(std::iter::repeat(0).take(FOUNDRY_LEAD_SILENCE_PADDING_SAMPLES));
    samples.extend(
        pcm.chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]])),
    );
    append_tail_silence_16k_mono(&mut samples);
    if samples.len() < FOUNDRY_MIN_AUDIO_CONTEXT_SAMPLES {
        samples.resize(FOUNDRY_MIN_AUDIO_CONTEXT_SAMPLES, 0);
    }
    encode_wav_16k_mono(&samples)
}

#[cfg(target_os = "windows")]
pub(crate) struct TempWavFile {
    path: PathBuf,
}

#[cfg(target_os = "windows")]
impl TempWavFile {
    pub(crate) fn create(pcm: &[u8]) -> Result<Self> {
        let wav = pcm_to_wav_with_foundry_context_padding(pcm);
        Self::write_temp(wav)
    }

    /// Encode the candidate PCM as a plain 16 kHz / mono / 16-bit WAV **without**
    /// the lead-silence / minimum-context padding used by the Foundry Whisper
    /// path.
    ///
    /// The local wake confirmation transcribes with Paraformer (an offline
    /// attention model). Prepending 250 ms of lead silence and padding short
    /// utterances up to a 4 s context destabilises its transcript of short wake
    /// phrases: an exact "开始录音" captured cleanly turns into near-misses such
    /// as "拍始录音", which then fails the ExactStart/PhoneticStart gate. The
    /// wake helper therefore must transcribe the raw candidate audio as-is.
    pub(crate) fn create_without_context_padding(pcm: &[u8]) -> Result<Self> {
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let wav = encode_wav_16k_mono(&samples);
        Self::write_temp(wav)
    }

    fn write_temp(wav: Vec<u8>) -> Result<Self> {
        let dir = foundry_temp_dir();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join(format!("foundry-whisper-{}.wav", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("create {}", path.display()))?;

        if let Err(err) = file.write_all(&wav) {
            drop(file);
            remove_partial_temp_wav(&path);
            return Err(err).with_context(|| format!("write {}", path.display()));
        }
        if let Err(err) = file.sync_all() {
            drop(file);
            remove_partial_temp_wav(&path);
            return Err(err).with_context(|| format!("sync {}", path.display()));
        }

        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(target_os = "windows")]
impl Drop for TempWavFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!(
                    "[foundry-asr] 清理临时 WAV 失败 {}: {err}",
                    self.path.display()
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn remove_partial_temp_wav(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            log::warn!(
                "[foundry-asr] 清理未完成的临时 WAV 失败 {}: {err}",
                path.display()
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn foundry_temp_dir() -> PathBuf {
    std::env::temp_dir()
        .join("Listener Type")
        .join("foundry-local-asr")
}

fn normalize_language_hint(language_hint: Option<String>) -> Option<String> {
    language_hint
        .map(|hint| hint.trim().to_string())
        .filter(|hint| !hint.is_empty())
}

fn trim_transcript_text(text: &str) -> String {
    text.trim().to_string()
}

fn foundry_transcribe_attempt_consumes_buffer<T>(result: &Result<T>) -> bool {
    let _ = result;
    true
}

#[cfg(test)]
mod tests {
    use crate::recorder::AudioConsumer;

    #[cfg(target_os = "windows")]
    fn test_provider() -> (
        super::FoundryLocalWhisperAsr,
        std::sync::Arc<super::FoundryLocalRuntime>,
    ) {
        use std::sync::Arc;

        let runtime = Arc::new(super::FoundryLocalRuntime::new());
        (
            super::FoundryLocalWhisperAsr::new(
                Arc::clone(&runtime),
                "whisper-small".into(),
                "auto".into(),
                Some(" zh ".into()),
            ),
            runtime,
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn test_provider() -> super::FoundryLocalWhisperAsr {
        super::FoundryLocalWhisperAsr::new("whisper-small".into(), Some(" zh ".into()))
    }

    #[test]
    fn foundry_provider_duration_uses_16k_i16_pcm() {
        let pcm = vec![0u8; 32_000];

        assert_eq!(super::pcm_duration_ms(&pcm), 1000);
    }

    #[test]
    fn foundry_provider_wav_adds_context_padding_and_ignores_odd_trailing_byte() {
        let pcm = [0x01, 0x00, 0xff, 0x7f, 0xee];
        let wav = super::pcm_to_wav_with_foundry_context_padding(&pcm);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()),
            super::FOUNDRY_MIN_AUDIO_CONTEXT_SAMPLES as u32 * 2
        );
        let data = &wav[44..];
        let lead_bytes = super::FOUNDRY_LEAD_SILENCE_PADDING_SAMPLES * 2;
        assert!(data[..lead_bytes].iter().all(|byte| *byte == 0));
        assert_eq!(&data[lead_bytes..lead_bytes + 4], &[0x01, 0x00, 0xff, 0x7f]);
        assert!(data[lead_bytes + 4..].iter().all(|byte| *byte == 0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_provider_temp_wav_drop_removes_file() {
        let pcm = [0x01, 0x00, 0xff, 0x7f];
        let path = {
            let temp = super::TempWavFile::create(&pcm).unwrap();
            let path = temp.path().to_path_buf();

            assert!(path.exists());

            path
        };

        assert!(!path.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_provider_temp_wav_without_context_padding_keeps_raw_samples() {
        // The local wake helper transcribes with Paraformer, which must receive
        // the candidate PCM verbatim: no lead silence and no minimum-context
        // padding. Padding the utterance destabilises Paraformer on short wake
        // phrases (an exact "开始录音" degrades to near-misses such as "拍始录音"),
        // so the padding-free writer has to round-trip the input samples exactly.
        let samples = [1234i16, -5678, 9, -10];
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let temp = super::TempWavFile::create_without_context_padding(&pcm).unwrap();
        let wav = std::fs::read(temp.path()).unwrap();
        let decoded = denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
        let decoded_samples: Vec<i16> = decoded
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        assert_eq!(decoded_samples, samples);
    }

    #[test]
    fn foundry_provider_normalizes_language_hint_and_text() {
        assert_eq!(
            super::normalize_language_hint(Some(" zh ".into())),
            Some("zh".into())
        );
        assert_eq!(super::normalize_language_hint(Some(" ".into())), None);
        assert_eq!(super::trim_transcript_text("  hello\r\n"), "hello");
    }

    #[test]
    fn foundry_transcribe_attempt_consumes_buffer_even_on_error() {
        let result: anyhow::Result<()> = Err(anyhow::anyhow!("transient runtime error"));

        assert!(super::foundry_transcribe_attempt_consumes_buffer(&result));
    }

    #[test]
    fn foundry_provider_cancel_clears_buffer() {
        #[cfg(target_os = "windows")]
        let (provider, _) = test_provider();
        #[cfg(not(target_os = "windows"))]
        let provider = test_provider();

        provider.consume_pcm_chunk(&[1, 0, 2, 0]);
        provider.cancel();

        assert!(provider.buffer.lock().is_empty());
        assert_eq!(
            provider
                .cancel_generation
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(provider.model_alias(), "whisper-small");
        assert_eq!(provider.language_hint(), Some("zh"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn foundry_provider_cancel_requests_runtime_prepare_cancel() {
        let (provider, runtime) = test_provider();

        provider.cancel();

        assert!(runtime.cancel_prepare_requested_for_tests());
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    #[ignore = "requires cached Foundry Local Whisper and consented local WAV fixtures"]
    async fn cached_local_wake_confirmation_replays_positive_and_negative_fixtures() {
        let positive_path =
            std::env::var("LISTENER_WAKE_PHRASE_WAV").expect("positive fixture path");
        let negative_paths =
            std::env::var("LISTENER_NON_WAKE_PHRASE_WAVS").expect("negative fixture paths");
        let phrase =
            std::env::var("LISTENER_WAKE_PHRASE").unwrap_or_else(|_| "开始录音".to_string());
        let runtime = std::sync::Arc::new(super::FoundryLocalRuntime::new());

        let positive_wav = std::fs::read(positive_path).expect("read positive fixture");
        let positive_pcm =
            denzic_audio_v1_core::read_wav_pcm16le(&positive_wav).expect("decode positive fixture");
        let positive = super::transcribe_cached_pcm(
            std::sync::Arc::clone(&runtime),
            crate::asr::local::foundry::DEFAULT_MODEL_ALIAS,
            Some("zh"),
            &positive_pcm,
            std::time::Duration::from_secs(8),
        )
        .await
        .expect("transcribe positive fixture");
        assert!(
            crate::wake_phrase::local_transcript_matches_phrase(&positive.text, &phrase),
            "positive transcript did not start with configured phrase: {:?}",
            positive.text
        );

        let mut negative_count = 0usize;
        for path in negative_paths.split(';').filter(|path| !path.is_empty()) {
            let wav = std::fs::read(path).expect("read negative fixture");
            let pcm =
                denzic_audio_v1_core::read_wav_pcm16le(&wav).expect("decode negative fixture");
            let transcript = super::transcribe_cached_pcm(
                std::sync::Arc::clone(&runtime),
                crate::asr::local::foundry::DEFAULT_MODEL_ALIAS,
                Some("zh"),
                &pcm,
                std::time::Duration::from_secs(8),
            )
            .await
            .expect("transcribe negative fixture");
            assert!(
                !crate::wake_phrase::local_transcript_matches_phrase(&transcript.text, &phrase),
                "negative fixture false-triggered: {path}"
            );
            negative_count += 1;
        }
        assert!(negative_count > 0);
        runtime
            .release_now()
            .await
            .expect("release cached Foundry model");
    }
}
