//! WAV helpers for ASR providers that accept complete audio files.
//!
//! The container layout itself lives in the platform `denzic-host-audio-v1-core`
//! crate (16 kHz / mono / 16-bit little-endian); this module keeps the
//! Listener-specific tail-silence padding helpers.

pub const WAV_TAIL_SILENCE_PADDING_MS: usize = 1_000;
pub const WAV_TAIL_SILENCE_PADDING_SAMPLES: usize = 16_000 * WAV_TAIL_SILENCE_PADDING_MS / 1_000;

/// Encode 16 kHz / mono / 16-bit little-endian PCM samples as a RIFF WAV file.
pub fn encode_wav_16k_mono(samples: &[i16]) -> Vec<u8> {
    denzic_host_audio_v1_core::wav::encode_wav(samples)
}

pub fn append_tail_silence_16k_mono(samples: &mut Vec<i16>) {
    samples.extend(std::iter::repeat(0).take(WAV_TAIL_SILENCE_PADDING_SAMPLES));
}

#[cfg(test)]
mod tests {
    use super::{
        append_tail_silence_16k_mono, encode_wav_16k_mono, WAV_TAIL_SILENCE_PADDING_MS,
        WAV_TAIL_SILENCE_PADDING_SAMPLES,
    };

    #[test]
    fn wav_header_matches_16k_mono_pcm() {
        let samples = [1i16, i16::MAX, i16::MIN, -2i16];
        let wav = encode_wav_16k_mono(&samples);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 44);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(
            &wav[44..],
            &[0x01, 0x00, 0xff, 0x7f, 0x00, 0x80, 0xfe, 0xff]
        );
    }

    #[test]
    fn tail_silence_padding_is_one_second_at_16k() {
        let mut samples = vec![123i16, -456i16];

        append_tail_silence_16k_mono(&mut samples);

        assert_eq!(WAV_TAIL_SILENCE_PADDING_MS, 1_000);
        assert_eq!(WAV_TAIL_SILENCE_PADDING_SAMPLES, 16_000);
        assert_eq!(samples.len(), 16_002);
        assert_eq!(&samples[..2], &[123, -456]);
        assert!(samples[2..].iter().all(|sample| *sample == 0));
    }
}
