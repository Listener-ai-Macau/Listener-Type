//! Listener-owned lossless audio framing layered on the VKA1 packet flags.
//!
//! The shared platform parser deliberately treats flags as opaque. Type
//! inflates this optional frame before handing it to that parser, so the
//! shared PCM/session contract remains byte-for-byte unchanged.

use crate::embedded_audio::{parse_packet, PacketType};

pub const LOSSLESS_PREDICTIVE_RICE_FLAG: u8 = 0x01;
const VKA1_MAGIC: &[u8; 4] = b"VKA1";
const HEADER_BYTES: usize = 20;
const PAYLOAD_LEN_OFFSET: usize = 16;
const RICE_VERSION_V1: u8 = 1;
const RICE_VERSION_V2: u8 = 2;
const RICE_VERSION_V3: u8 = 3;
const RICE_HEADER_BYTES: usize = 6;
const RICE_MAX_K: u8 = 15;
const MAX_ZIGZAG_RESIDUAL: u32 = 1_048_560;
const RICE_PREDICTOR_FIRST_ORDER: u8 = 1;
const RICE_PREDICTOR_SECOND_ORDER: u8 = 2;
const RICE_PREDICTOR_THIRD_ORDER: u8 = 3;
const RICE_PREDICTOR_FOURTH_ORDER: u8 = 4;

pub fn normalize_listener_audio_notification(notification: &[u8]) -> Result<Vec<u8>, String> {
    if notification.len() < 6
        || &notification[..VKA1_MAGIC.len()] != VKA1_MAGIC
        || notification[5] & LOSSLESS_PREDICTIVE_RICE_FLAG == 0
    {
        return Ok(notification.to_vec());
    }

    let packet = parse_packet(notification)
        .map_err(|err| format!("lossless audio frame header rejected: {err}"))?;
    if packet.header.packet_type != PacketType::AudioData {
        return Err(format!(
            "lossless audio flag is only valid on audio data, got {:?}",
            packet.header.packet_type
        ));
    }

    let pcm = decode_lossless_predictive_rice(
        packet.payload,
        usize::from(packet.header.packet_pcm_bytes),
    )?;
    let header_len = usize::from(packet.header.header_len);
    if header_len < HEADER_BYTES || header_len > notification.len() {
        return Err(format!(
            "lossless audio frame has invalid header length {header_len}"
        ));
    }
    if pcm.len() > u16::MAX as usize {
        return Err(format!(
            "lossless PCM payload too large: {} bytes",
            pcm.len()
        ));
    }

    let mut normalized = notification[..header_len].to_vec();
    normalized[5] &= !LOSSLESS_PREDICTIVE_RICE_FLAG;
    normalized[PAYLOAD_LEN_OFFSET..PAYLOAD_LEN_OFFSET + 2]
        .copy_from_slice(&(pcm.len() as u16).to_le_bytes());
    normalized.extend_from_slice(&pcm);
    Ok(normalized)
}

fn decode_lossless_predictive_rice(
    payload: &[u8],
    expected_pcm_bytes: usize,
) -> Result<Vec<u8>, String> {
    if expected_pcm_bytes < 4 || expected_pcm_bytes % 2 != 0 {
        return Err(format!(
            "lossless PCM byte count must be an even value of at least four, got {expected_pcm_bytes}"
        ));
    }
    let version = payload
        .first()
        .copied()
        .ok_or_else(|| "lossless Rice payload is empty".to_string())?;
    let control = payload
        .get(1)
        .copied()
        .ok_or_else(|| "lossless Rice payload missing parameter".to_string())?;
    let (predictor, rice_k, seed_count) = match version {
        RICE_VERSION_V1 => (RICE_PREDICTOR_SECOND_ORDER, control, 2),
        RICE_VERSION_V2 => (control >> 4, control & RICE_MAX_K, 2),
        RICE_VERSION_V3 => (
            control >> 4,
            control & RICE_MAX_K,
            usize::from(control >> 4),
        ),
        value => return Err(format!("unsupported lossless Rice version {value}")),
    };
    if !(RICE_PREDICTOR_FIRST_ORDER..=RICE_PREDICTOR_FOURTH_ORDER).contains(&predictor)
        || (version != RICE_VERSION_V3 && predictor > RICE_PREDICTOR_SECOND_ORDER)
    {
        return Err(format!("invalid lossless Rice predictor {predictor}"));
    }
    if rice_k > RICE_MAX_K {
        return Err(format!("invalid lossless Rice parameter {rice_k}"));
    }
    let header_bytes = if version == RICE_VERSION_V3 {
        2 + seed_count * 2
    } else {
        RICE_HEADER_BYTES
    };
    if payload.len() < header_bytes || expected_pcm_bytes / 2 < seed_count {
        return Err(format!(
            "lossless Rice payload too short for predictor {predictor}: {} bytes",
            payload.len()
        ));
    }

    let mut pcm = Vec::with_capacity(expected_pcm_bytes);
    let mut samples = Vec::with_capacity(expected_pcm_bytes / 2);
    for seed_index in 0..seed_count {
        let offset = 2 + seed_index * 2;
        let sample = i16::from_le_bytes([payload[offset], payload[offset + 1]]);
        samples.push(sample);
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let mut bit_offset = header_bytes * 8;

    for _ in seed_count..expected_pcm_bytes / 2 {
        let mut quotient = 0u32;
        loop {
            let bit = read_bit(payload, bit_offset)
                .ok_or_else(|| "truncated lossless Rice unary residual".to_string())?;
            bit_offset += 1;
            if bit {
                break;
            }
            quotient = quotient.saturating_add(1);
            if quotient > (MAX_ZIGZAG_RESIDUAL >> rice_k) {
                return Err("lossless Rice residual exceeds signed 16-bit range".to_string());
            }
        }

        let mut remainder = 0u32;
        for bit_index in 0..rice_k {
            let bit = read_bit(payload, bit_offset)
                .ok_or_else(|| "truncated lossless Rice remainder".to_string())?;
            bit_offset += 1;
            if bit {
                remainder |= 1u32 << bit_index;
            }
        }

        let zigzag = (quotient << rice_k) | remainder;
        if zigzag > MAX_ZIGZAG_RESIDUAL {
            return Err("lossless Rice residual exceeds signed 16-bit range".to_string());
        }
        let residual = ((zigzag >> 1) as i32) ^ -((zigzag & 1) as i32);
        let sample = match predictor {
            RICE_PREDICTOR_FIRST_ORDER => {
                i32::from(*samples.last().expect("seeded predictor")) + residual
            }
            RICE_PREDICTOR_SECOND_ORDER => {
                2 * i32::from(samples[samples.len() - 1]) - i32::from(samples[samples.len() - 2])
                    + residual
            }
            RICE_PREDICTOR_THIRD_ORDER => {
                3 * i32::from(samples[samples.len() - 1])
                    - 3 * i32::from(samples[samples.len() - 2])
                    + i32::from(samples[samples.len() - 3])
                    + residual
            }
            RICE_PREDICTOR_FOURTH_ORDER => {
                4 * i32::from(samples[samples.len() - 1])
                    - 6 * i32::from(samples[samples.len() - 2])
                    + 4 * i32::from(samples[samples.len() - 3])
                    - i32::from(samples[samples.len() - 4])
                    + residual
            }
            _ => unreachable!("validated Rice predictor"),
        };
        if !(i32::from(i16::MIN)..=i32::from(i16::MAX)).contains(&sample) {
            return Err("lossless Rice sample reconstruction overflowed i16".to_string());
        }
        let sample = sample as i16;
        samples.push(sample);
        pcm.extend_from_slice(&sample.to_le_bytes());
    }

    Ok(pcm)
}

fn read_bit(bytes: &[u8], bit_offset: usize) -> Option<bool> {
    let byte = *bytes.get(bit_offset / 8)?;
    Some(byte & (1 << (bit_offset % 8)) != 0)
}

#[cfg(test)]
fn encode_lossless_predictive_rice(pcm: &[u8], version: u8) -> Vec<u8> {
    assert!(pcm.len() >= 4 && pcm.len() % 2 == 0);
    assert!(matches!(version, RICE_VERSION_V1 | RICE_VERSION_V2));
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect();
    let mut best_k = 0u8;
    let mut best_predictor = RICE_PREDICTOR_SECOND_ORDER;
    let mut best_bits = usize::MAX;
    let predictors: &[u8] = if version == RICE_VERSION_V2 {
        &[RICE_PREDICTOR_FIRST_ORDER, RICE_PREDICTOR_SECOND_ORDER]
    } else {
        &[RICE_PREDICTOR_SECOND_ORDER]
    };
    for &predictor in predictors {
        for rice_k in 0..=RICE_MAX_K {
            let mut bits = RICE_HEADER_BYTES * 8;
            for window in samples.windows(3) {
                let residual = match predictor {
                    RICE_PREDICTOR_FIRST_ORDER => i32::from(window[2]) - i32::from(window[1]),
                    RICE_PREDICTOR_SECOND_ORDER => {
                        i32::from(window[2]) - 2 * i32::from(window[1]) + i32::from(window[0])
                    }
                    _ => unreachable!("configured Rice predictor"),
                };
                let zigzag = zigzag(residual);
                bits += usize::try_from((zigzag >> rice_k) + 1 + u32::from(rice_k)).unwrap();
            }
            if bits < best_bits {
                best_bits = bits;
                best_k = rice_k;
                best_predictor = predictor;
            }
        }
    }

    let mut output = vec![0u8; best_bits.div_ceil(8)];
    output[0] = version;
    output[1] = if version == RICE_VERSION_V2 {
        (best_predictor << 4) | best_k
    } else {
        best_k
    };
    output[2..4].copy_from_slice(&samples[0].to_le_bytes());
    output[4..6].copy_from_slice(&samples[1].to_le_bytes());
    let mut bit_offset = RICE_HEADER_BYTES * 8;
    for window in samples.windows(3) {
        let residual = match best_predictor {
            RICE_PREDICTOR_FIRST_ORDER => i32::from(window[2]) - i32::from(window[1]),
            RICE_PREDICTOR_SECOND_ORDER => {
                i32::from(window[2]) - 2 * i32::from(window[1]) + i32::from(window[0])
            }
            _ => unreachable!("selected Rice predictor"),
        };
        let zigzag = zigzag(residual);
        bit_offset += usize::try_from(zigzag >> best_k).unwrap();
        write_bit(&mut output, bit_offset);
        bit_offset += 1;
        for bit_index in 0..best_k {
            if zigzag & (1u32 << bit_index) != 0 {
                write_bit(&mut output, bit_offset);
            }
            bit_offset += 1;
        }
    }
    output
}

#[cfg(test)]
fn zigzag(residual: i32) -> u32 {
    if residual >= 0 {
        (residual as u32) * 2
    } else {
        ((-residual) as u32) * 2 - 1
    }
}

#[cfg(test)]
fn write_bit(bytes: &mut [u8], bit_offset: usize) {
    bytes[bit_offset / 8] |= 1 << (bit_offset % 8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn rice_fixture_decodes_exact_pcm() {
        let compressed = [RICE_VERSION_V1, 0, 0, 0, 1, 0, 0x03];
        assert_eq!(
            decode_lossless_predictive_rice(&compressed, 8).expect("fixture decodes"),
            pcm(&[0, 1, 2, 3])
        );
    }

    #[test]
    fn codec_round_trips_speech_like_pcm_without_loss() {
        let samples: Vec<i16> = (0..112)
            .map(|index| {
                let index = index as i32;
                ((index * 137 + (index * index * 19) % 1200) - 7000) as i16
            })
            .collect();
        let raw = pcm(&samples);
        let compressed = encode_lossless_predictive_rice(&raw, RICE_VERSION_V2);
        assert!(compressed.len() < raw.len());
        assert_eq!(
            decode_lossless_predictive_rice(&compressed, raw.len()).expect("round trip"),
            raw
        );
    }

    #[test]
    fn codec_preserves_i16_extremes() {
        let raw = pcm(&[i16::MAX, i16::MIN, i16::MAX, i16::MIN]);
        let compressed = encode_lossless_predictive_rice(&raw, RICE_VERSION_V2);
        assert_eq!(
            decode_lossless_predictive_rice(&compressed, raw.len()).expect("round trip"),
            raw
        );
    }

    #[test]
    fn compressed_notification_is_normalized_before_platform_parser() {
        let mut notification = vec![0u8; HEADER_BYTES];
        notification[..4].copy_from_slice(VKA1_MAGIC);
        notification[4] = PacketType::AudioData.wire_value();
        notification[5] = LOSSLESS_PREDICTIVE_RICE_FLAG;
        notification[6..8].copy_from_slice(&(HEADER_BYTES as u16).to_le_bytes());
        notification[14] = 0;
        notification[15] = 1;
        notification[16..18].copy_from_slice(&(7u16).to_le_bytes());
        notification[18..20].copy_from_slice(&(8u16).to_le_bytes());
        notification.extend_from_slice(&[RICE_VERSION_V1, 0, 0, 0, 1, 0, 0x03]);

        let normalized = normalize_listener_audio_notification(&notification).expect("normalize");
        assert_eq!(normalized[5], 0);
        assert_eq!(&normalized[HEADER_BYTES..], pcm(&[0, 1, 2, 3]));
        assert_eq!(
            parse_packet(&normalized).expect("platform packet").payload,
            pcm(&[0, 1, 2, 3])
        );
    }

    #[test]
    fn v2_first_order_predictor_fits_a_packet_that_v1_cannot() {
        let mut state = 1u32;
        let mut sample = 0i16;
        let samples: Vec<i16> = (0..200)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let delta = ((state >> 24) as i16) - 128;
                sample = sample.saturating_add(delta);
                sample
            })
            .collect();
        let raw = pcm(&samples);
        let v1 = encode_lossless_predictive_rice(&raw, RICE_VERSION_V1);
        let v2 = encode_lossless_predictive_rice(&raw, RICE_VERSION_V2);

        assert!(v1.len() > 224);
        assert!(v2.len() <= 224);
        assert_eq!(
            decode_lossless_predictive_rice(&v2, raw.len()).expect("v2 round trip"),
            raw
        );
    }

    #[test]
    fn v3_fourth_order_predictor_decodes_exact_pcm() {
        // n^3 samples have a zero fourth-order residual. V3 carries the four
        // initial samples so this remains byte-for-byte PCM on decode.
        let compressed = [
            RICE_VERSION_V3,
            RICE_PREDICTOR_FOURTH_ORDER << 4,
            0,
            0,
            1,
            0,
            8,
            0,
            27,
            0,
            0x01,
        ];
        assert_eq!(
            decode_lossless_predictive_rice(&compressed, 10).expect("v3 round trip"),
            pcm(&[0, 1, 8, 27, 64])
        );
    }
}
