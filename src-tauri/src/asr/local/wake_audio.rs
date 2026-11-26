use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::asr::wav::encode_wav_16k_mono;

pub(crate) struct TempWavFile {
    path: PathBuf,
}

impl TempWavFile {
    // The wake recognizer needs the candidate PCM without added context padding.
    pub(crate) fn create_without_context_padding(pcm: &[u8]) -> Result<Self> {
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let wav = encode_wav_16k_mono(&samples);
        Self::write_temp(wav)
    }

    fn write_temp(wav: Vec<u8>) -> Result<Self> {
        let dir = wake_temp_dir();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join(format!("wake-phrase-{}.wav", Uuid::new_v4()));
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

impl Drop for TempWavFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!(
                    "[wake-phrase] 清理临时 WAV 失败 {}: {err}",
                    self.path.display()
                );
            }
        }
    }
}

fn remove_partial_temp_wav(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            log::warn!(
                "[wake-phrase] 清理未完成的临时 WAV 失败 {}: {err}",
                path.display()
            );
        }
    }
}

fn wake_temp_dir() -> PathBuf {
    std::env::temp_dir()
        .join("Listener Type")
        .join("wake-audio")
}

#[cfg(test)]
mod tests {
    #[test]
    fn wake_temp_wav_drop_removes_file() {
        let pcm = [0x01, 0x00, 0xff, 0x7f];
        let path = {
            let temp = super::TempWavFile::create_without_context_padding(&pcm).unwrap();
            let path = temp.path().to_path_buf();

            assert!(path.exists());

            path
        };

        assert!(!path.exists());
    }

    #[test]
    fn wake_temp_wav_without_context_padding_keeps_raw_samples() {
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
}
