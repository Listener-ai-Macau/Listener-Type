#[path = "../../../../src-tauri/src/asr/frame.rs"]
pub mod frame;

pub trait AudioConsumer: Send + Sync {
    fn consume_pcm_chunk(&self, pcm: &[u8]);
}

#[derive(Debug, Clone)]
pub struct RawTranscript {
    pub text: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct DictionaryHotword {
    pub phrase: String,
    pub enabled: bool,
}

#[path = "../../../../src-tauri/src/asr/volcengine.rs"]
pub mod volcengine;
