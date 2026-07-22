//! Listener-owned lossless audio framing layered on the VKA1 packet flags.
//!
//! The codec itself lives in the shared platform crate
//! (`denzic_audio_v1_core::lossless_v1`); this module only keeps the
//! Listener-facing names so call sites stay unchanged. The shared PCM/session
//! contract remains byte-for-byte unchanged.

// LOSSLESS_PREDICTIVE_RICE_FLAG stays part of the Listener-facing surface even
// though current call sites only need the normalizer.
#[allow(unused_imports)]
pub use denzic_audio_v1_core::lossless_v1::{
    normalize_lossless_audio_notification as normalize_listener_audio_notification,
    LOSSLESS_RICE_FLAG as LOSSLESS_PREDICTIVE_RICE_FLAG,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_audio::{parse_packet, PacketType};

    const HEADER_BYTES: usize = 20;

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn compressed_notification_is_normalized_before_platform_parser() {
        let mut notification = vec![0u8; HEADER_BYTES];
        notification[..4].copy_from_slice(b"VKA1");
        notification[4] = PacketType::AudioData.wire_value();
        notification[5] = LOSSLESS_PREDICTIVE_RICE_FLAG;
        notification[6..8].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
        notification[14] = 0;
        notification[15] = 1;
        notification[16..18].copy_from_slice(&(7u16).to_le_bytes());
        notification[18..20].copy_from_slice(&(8u16).to_le_bytes());
        notification.extend_from_slice(&[1, 0, 0, 0, 1, 0, 0x03]);

        let normalized = normalize_listener_audio_notification(&notification).expect("normalize");
        assert_eq!(normalized[5], 0);
        assert_eq!(&normalized[HEADER_BYTES..], pcm(&[0, 1, 2, 3]));
        assert_eq!(
            parse_packet(&normalized).expect("platform packet").payload,
            pcm(&[0, 1, 2, 3])
        );
    }
}
