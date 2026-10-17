// DOVZ is negotiated on the freshly opened OTA target, never from a cached
// device name, shared UUID, manifest field or a previous connection's receipt.
// Disabled by product request: normal OTA measures and transfers the canonical
// raw image. Keep the isolated codec evidence without enabling it implicitly
// merely because firmware advertises support.
const LISTENER_OTA_BLOCK_DEFLATE_ENABLED: bool = false;
const LISTENER_OTA_WIRE_BLOCK_BYTES: usize = 16_384;
const LISTENER_OTA_WIRE_IMAGE_MAX_BYTES: usize = 0x60_0000;

fn listener_ota_v1_supports_block_deflate(readiness: &[u8]) -> bool {
    if readiness.len() > 512 {
        return false;
    }
    let Ok(text) = std::str::from_utf8(readiness) else {
        return false;
    };
    let mut declarations = text
        .split(';')
        .filter_map(|part| part.split_once('='))
        .filter(|(key, _)| *key == "ota_deflate_blocks_v1");
    matches!(declarations.next(), Some((_, "1"))) && declarations.next().is_none()
}

fn listener_ota_v1_encode_block_deflate(raw: &[u8]) -> Result<Vec<u8>, String> {
    use sha2::Digest;
    use std::io::Write;

    if raw.is_empty() || raw.len() > LISTENER_OTA_WIRE_IMAGE_MAX_BYTES {
        return Err("Listener OTA canonical image is outside the DOVZ size bound".into());
    }
    let mut wire =
        Vec::with_capacity(raw.len() + 48 + raw.len().div_ceil(LISTENER_OTA_WIRE_BLOCK_BYTES) * 8);
    wire.extend_from_slice(b"DOVZ");
    wire.extend_from_slice(&[1, 1]);
    wire.extend_from_slice(&48u16.to_le_bytes());
    wire.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    wire.extend_from_slice(&(LISTENER_OTA_WIRE_BLOCK_BYTES as u32).to_le_bytes());
    wire.extend_from_slice(&sha2::Sha256::digest(raw));
    for block in raw.chunks(LISTENER_OTA_WIRE_BLOCK_BYTES) {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        encoder
            .write_all(block)
            .map_err(|err| format!("Listener OTA block encoding failed: {err}"))?;
        let compressed = encoder
            .finish()
            .map_err(|err| format!("Listener OTA block encoding finish failed: {err}"))?;
        let (body, kind) = if compressed.len() < block.len() {
            (compressed.as_slice(), 1)
        } else {
            (block, 0)
        };
        wire.extend_from_slice(&(body.len() as u16).to_le_bytes());
        wire.extend_from_slice(&(block.len() as u16).to_le_bytes());
        wire.extend_from_slice(&[kind, 0, 0, 0]);
        wire.extend_from_slice(body);
    }
    Ok(wire)
}

fn listener_ota_v1_control_wire_packet(
    packet: &[u8; denzic_ota_core::CONTROL_BYTES],
    encoded_raw_size: Option<u32>,
) -> Result<Vec<u8>, String> {
    let mut wire = packet.to_vec();
    if packet[4] == denzic_ota_core::OP_BEGIN {
        if let Some(raw_size) = encoded_raw_size {
            if raw_size == 0
                || raw_size as usize > LISTENER_OTA_WIRE_IMAGE_MAX_BYTES
                || packet[6] != 0
                || packet[7] != 0
            {
                return Err("Listener OTA invalid encoded BEGIN contract".into());
            }
            wire[6] = 1;
            wire.extend_from_slice(&raw_size.to_le_bytes());
        }
    }
    Ok(wire)
}
