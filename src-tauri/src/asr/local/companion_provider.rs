use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::Result;
use parking_lot::Mutex;

use crate::asr::RawTranscript;

#[derive(Clone, Default)]
pub(crate) struct LocalPhrasePrefix {
    pub end_ms: u64,
    pub raw_text: String,
}

pub(crate) fn join_local_phrases(prefix: &str, phrase: &str) -> String {
    if prefix.is_empty() || phrase.is_empty() {
        return format!("{prefix}{phrase}");
    }
    let left = prefix.chars().next_back().expect("nonempty");
    let right = phrase.chars().next().expect("nonempty");
    let separator = if left.is_ascii() && right.is_ascii_alphanumeric() && !left.is_whitespace() {
        " "
    } else {
        ""
    };
    format!("{prefix}{separator}{phrase}")
}

pub(crate) struct CompanionDictationAsr {
    pub(crate) local_phrase_prefix: tokio::sync::Mutex<LocalPhrasePrefix>,
    automatic_stop_speech_end_ms: AtomicU64,
    buffer: Mutex<Vec<u8>>,
    cancel_generation: AtomicU64,
    confirmed_no_speech: AtomicBool,
}

impl CompanionDictationAsr {
    pub(crate) fn new() -> Self {
        Self {
            local_phrase_prefix: tokio::sync::Mutex::new(LocalPhrasePrefix::default()),
            automatic_stop_speech_end_ms: AtomicU64::new(u64::MAX),
            buffer: Mutex::new(Vec::new()),
            cancel_generation: AtomicU64::new(0),
            confirmed_no_speech: AtomicBool::new(false),
        }
    }

    pub async fn transcribe(&self, audio_timeout: std::time::Duration) -> Result<RawTranscript> {
        let cancel_generation = self.cancel_generation.load(Ordering::SeqCst);
        if self.confirmed_no_speech.load(Ordering::Acquire) {
            let mut pcm = self.buffer.lock();
            let duration_ms = pcm_duration_ms(&pcm);
            pcm.clear();
            return Ok(RawTranscript { text: String::new(), duration_ms });
        }
        let pcm = self.buffer.lock().clone();
        if pcm.is_empty() {
            return Ok(RawTranscript {
                text: String::new(),
                duration_ms: 0,
            });
        }

        let result = self.transcribe_with_local_phrase_prefix(pcm, audio_timeout).await;
        if self.cancel_generation.load(Ordering::SeqCst) != cancel_generation {
            anyhow::bail!("Local transcription cancelled");
        }
        self.buffer.lock().clear();
        result
    }

    pub(crate) fn mark_automatic_stop_speech_end(&self, last_speech_end: Option<u64>) {
        if let Some(end_ms) = last_speech_end {
            self.automatic_stop_speech_end_ms.store(end_ms, Ordering::Release);
        }
    }

    async fn transcribe_with_local_phrase_prefix(
        &self, mut pcm: Vec<u8>, audio_timeout: std::time::Duration,
    ) -> Result<RawTranscript> {
        // A paste and checkpoint share this lock. The final take either sees
        // both or sees neither, including a STOP arriving mid-dispatch.
        let prefix = self.local_phrase_prefix.lock().await.clone();
        if prefix.end_ms == 0 {
            return self.transcribe_final_pcm(pcm, audio_timeout).await;
        }
        let duration_ms = pcm_duration_ms(&pcm);
        let last_speech = self.automatic_stop_speech_end_ms.load(Ordering::Acquire);
        let text = if last_speech <= prefix.end_ms || prefix.end_ms >= duration_ms {
            prefix.raw_text
        } else {
            let tail = pcm.split_off(prefix.end_ms as usize * 32);
            let result = self.transcribe_final_pcm(tail, audio_timeout).await?;
            join_local_phrases(&prefix.raw_text, &result.text)
        };
        Ok(RawTranscript { text, duration_ms })
    }

    async fn transcribe_final_pcm(
        &self,
        pcm: Vec<u8>,
        audio_timeout: std::time::Duration,
    ) -> Result<RawTranscript> {
        let duration_ms = pcm_duration_ms(&pcm);
        let text = super::companion_paraformer::transcribe(pcm.into(), audio_timeout)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(RawTranscript { text: text.trim().to_owned(), duration_ms })
    }

    pub(crate) fn preview_pcm(&self, end_ms: u64, window_ms: usize) -> Vec<u8> {
        const BYTES_PER_MS: usize = 32;
        let buffer = self.buffer.lock();
        let end = (end_ms as usize).saturating_mul(BYTES_PER_MS).min(buffer.len());
        let start = end.saturating_sub(window_ms * BYTES_PER_MS);
        buffer[start..end].to_vec()
    }

    pub(crate) fn mark_confirmed_no_speech(&self) {
        self.confirmed_no_speech.store(true, Ordering::Release);
    }

    pub fn cancel(&self) {
        self.cancel_generation.fetch_add(1, Ordering::SeqCst);
        self.buffer.lock().clear();
    }
}

impl crate::recorder::AudioConsumer for CompanionDictationAsr {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.buffer.lock().extend_from_slice(pcm);
    }
}

fn pcm_duration_ms(pcm: &[u8]) -> u64 {
    (pcm.len() as u64 / 2) * 1000 / 16_000
}
