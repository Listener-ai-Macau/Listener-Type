//! Embedded BLE audio protocol helpers.
//!
//! This module mirrors the firmware VKA1 wire contract without owning BLE I/O.
//! Windows GATT code and replay tests can both feed notifications into the same
//! parser and session collector.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"VKA1";
pub const HEADER_LEN: usize = 20;
pub const PCM_SAMPLE_RATE_HZ: u32 = 16_000;
pub const PCM_CHANNELS: u16 = 1;
pub const PCM_SAMPLE_WIDTH_BITS: u16 = 16;
pub const PCM_BYTES_PER_SECOND: usize =
    PCM_SAMPLE_RATE_HZ as usize * PCM_CHANNELS as usize * (PCM_SAMPLE_WIDTH_BITS as usize / 8);
pub const DEFAULT_REPLAY_PAYLOAD_PCM_BYTES: usize = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmbeddedAudioInputFormat {
    #[serde(rename = "wav")]
    Wav,
    #[serde(rename = "pcm16le")]
    Pcm16Le,
}

impl EmbeddedAudioInputFormat {
    pub fn infer_from_path(path: &Path) -> Self {
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        {
            Self::Wav
        } else {
            Self::Pcm16Le
        }
    }
}

impl std::str::FromStr for EmbeddedAudioInputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "wav" => Ok(Self::Wav),
            "pcm" | "pcm16" | "pcm16le" | "s16le" => Ok(Self::Pcm16Le),
            other => Err(format!("unsupported embedded audio input format: {other}")),
        }
    }
}

#[derive(Debug, Error)]
pub enum InputAudioError {
    #[error("read embedded audio input failed: {0}")]
    Read(#[source] std::io::Error),
    #[error("pcm_s16le input byte length must be even")]
    OddPcmByteLength,
    #[error("input is not a RIFF/WAVE file")]
    NotRiffWave,
    #[error("wav chunk size overflow")]
    WavChunkSizeOverflow,
    #[error("wav chunk extends past end of file")]
    WavChunkOutOfBounds,
    #[error("wav fmt chunk missing")]
    WavFmtMissing,
    #[error("wav fmt chunk too short")]
    WavFmtTooShort,
    #[error(
        "wav must be PCM {expected_sample_rate_hz} Hz mono {expected_bits_per_sample}-bit; got format={audio_format} channels={channels} sampleRate={sample_rate_hz} bits={bits_per_sample}"
    )]
    UnsupportedWavFormat {
        expected_sample_rate_hz: u32,
        expected_bits_per_sample: u16,
        audio_format: u16,
        channels: u16,
        sample_rate_hz: u32,
        bits_per_sample: u16,
    },
    #[error("wav data chunk missing")]
    WavDataMissing,
}

pub fn read_input_pcm(
    path: &Path,
    format: Option<EmbeddedAudioInputFormat>,
) -> Result<Vec<u8>, InputAudioError> {
    let bytes = fs::read(path).map_err(InputAudioError::Read)?;
    let format = format.unwrap_or_else(|| EmbeddedAudioInputFormat::infer_from_path(path));
    decode_input_pcm(&bytes, format)
}

pub fn decode_input_pcm(
    bytes: &[u8],
    format: EmbeddedAudioInputFormat,
) -> Result<Vec<u8>, InputAudioError> {
    let pcm = match format {
        EmbeddedAudioInputFormat::Wav => read_wav_pcm16le(bytes)?,
        EmbeddedAudioInputFormat::Pcm16Le => bytes.to_vec(),
    };
    if pcm.len() % 2 != 0 {
        return Err(InputAudioError::OddPcmByteLength);
    }
    Ok(pcm)
}

pub fn read_wav_pcm16le(bytes: &[u8]) -> Result<Vec<u8>, InputAudioError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(InputAudioError::NotRiffWave);
    }

    let mut fmt = None;
    let mut data = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = read_u32_le(bytes, offset + 4) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(chunk_size)
            .ok_or(InputAudioError::WavChunkSizeOverflow)?;
        if chunk_end > bytes.len() {
            return Err(InputAudioError::WavChunkOutOfBounds);
        }

        match chunk_id {
            b"fmt " => {
                if chunk_size < 16 {
                    return Err(InputAudioError::WavFmtTooShort);
                }
                fmt = Some(WavFormat {
                    audio_format: read_u16_le(bytes, chunk_start),
                    channels: read_u16_le(bytes, chunk_start + 2),
                    sample_rate_hz: read_u32_le(bytes, chunk_start + 4),
                    bits_per_sample: read_u16_le(bytes, chunk_start + 14),
                });
            }
            b"data" => data = Some(bytes[chunk_start..chunk_end].to_vec()),
            _ => {}
        }

        offset = chunk_end + (chunk_size % 2);
    }

    let fmt = fmt.ok_or(InputAudioError::WavFmtMissing)?;
    if fmt.audio_format != 1
        || fmt.channels != PCM_CHANNELS
        || fmt.sample_rate_hz != PCM_SAMPLE_RATE_HZ
        || fmt.bits_per_sample != PCM_SAMPLE_WIDTH_BITS
    {
        return Err(InputAudioError::UnsupportedWavFormat {
            expected_sample_rate_hz: PCM_SAMPLE_RATE_HZ,
            expected_bits_per_sample: PCM_SAMPLE_WIDTH_BITS,
            audio_format: fmt.audio_format,
            channels: fmt.channels,
            sample_rate_hz: fmt.sample_rate_hz,
            bits_per_sample: fmt.bits_per_sample,
        });
    }

    data.ok_or(InputAudioError::WavDataMissing)
}

#[derive(Debug, Clone, Copy)]
struct WavFormat {
    audio_format: u16,
    channels: u16,
    sample_rate_hz: u32,
    bits_per_sample: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketType {
    SessionStart,
    AudioData,
    SessionStop,
    SessionCancel,
    SessionError,
}

impl PacketType {
    pub const fn wire_value(self) -> u8 {
        match self {
            Self::SessionStart => 1,
            Self::AudioData => 2,
            Self::SessionStop => 3,
            Self::SessionCancel => 4,
            Self::SessionError => 5,
        }
    }
}

impl TryFrom<u8> for PacketType {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::SessionStart),
            2 => Ok(Self::AudioData),
            3 => Ok(Self::SessionStop),
            4 => Ok(Self::SessionCancel),
            5 => Ok(Self::SessionError),
            other => Err(ParseError::UnknownPacketType(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    None,
    QueueFull,
    NotifyTimeout,
    LinkLost,
    SequenceOverflow,
    InvalidState,
    NoMemory,
    PacketTooLarge,
    Transport,
    Unknown(u16),
}

impl SessionErrorCode {
    pub const fn from_wire(value: u16) -> Self {
        match value {
            0 => Self::None,
            1 => Self::QueueFull,
            2 => Self::NotifyTimeout,
            3 => Self::LinkLost,
            4 => Self::SequenceOverflow,
            5 => Self::InvalidState,
            6 => Self::NoMemory,
            7 => Self::PacketTooLarge,
            8 => Self::Transport,
            other => Self::Unknown(other),
        }
    }

    pub const fn wire_value(self) -> u16 {
        match self {
            Self::None => 0,
            Self::QueueFull => 1,
            Self::NotifyTimeout => 2,
            Self::LinkLost => 3,
            Self::SequenceOverflow => 4,
            Self::InvalidState => 5,
            Self::NoMemory => 6,
            Self::PacketTooLarge => 7,
            Self::Transport => 8,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    Stop,
    Cancel,
    Error(SessionErrorCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PacketHeader {
    pub packet_type: PacketType,
    pub flags: u8,
    pub header_len: u16,
    pub session_id: u32,
    pub packet_sequence: u16,
    pub expected_packet_count: u16,
    pub fragment_index: u8,
    pub fragment_count: u8,
    pub payload_len: u16,
    pub packet_pcm_bytes: u16,
    pub reserved: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub header: PacketHeader,
    pub payload: &'a [u8],
}

impl Packet<'_> {
    pub fn payload_pcm(&self) -> &[u8] {
        let declared = usize::from(self.header.packet_pcm_bytes);
        let wanted = if declared == 0 {
            self.payload.len()
        } else {
            declared.min(self.payload.len())
        };
        &self.payload[..wanted]
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("VKA1 notification too short: {actual} bytes")]
    TooShort { actual: usize },
    #[error("invalid VKA1 magic: {0:?}")]
    InvalidMagic([u8; 4]),
    #[error("unknown VKA1 packet type: {0}")]
    UnknownPacketType(u8),
    #[error("invalid VKA1 header length: {0}")]
    InvalidHeaderLen(u16),
    #[error("VKA1 payload is truncated: expected at least {expected} bytes, got {actual}")]
    TruncatedPayload { expected: usize, actual: usize },
    #[error("unexpected VKA1 fragment fields: index={fragment_index}, count={fragment_count}")]
    UnexpectedFragmentFields {
        fragment_index: u8,
        fragment_count: u8,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplayBuildError {
    #[error("VKA1 replay payload size must be greater than zero")]
    EmptyPayloadSize,
    #[error("VKA1 replay payload size is too large: {payload_pcm_bytes} bytes")]
    PayloadTooLarge { payload_pcm_bytes: usize },
    #[error("VKA1 replay has too many audio packets: {packet_count}")]
    PacketSequenceOverflow { packet_count: usize },
    #[error("VKA1 replay packet payload is too large: {pcm_bytes} bytes")]
    PacketPayloadTooLarge { pcm_bytes: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayConfig {
    pub session_id: u32,
    pub payload_pcm_bytes: usize,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            session_id: 1,
            payload_pcm_bytes: DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
        }
    }
}

pub fn parse_packet(notification: &[u8]) -> Result<Packet<'_>, ParseError> {
    if notification.len() < HEADER_LEN {
        return Err(ParseError::TooShort {
            actual: notification.len(),
        });
    }

    let magic: [u8; 4] = [
        notification[0],
        notification[1],
        notification[2],
        notification[3],
    ];
    if &magic != MAGIC {
        return Err(ParseError::InvalidMagic(magic));
    }

    let packet_type = PacketType::try_from(notification[4])?;
    let flags = notification[5];
    let header_len = read_u16_le(notification, 6);
    if usize::from(header_len) < HEADER_LEN || usize::from(header_len) > notification.len() {
        return Err(ParseError::InvalidHeaderLen(header_len));
    }

    let session_id = read_u32_le(notification, 8);
    let sequence_or_expected = read_u16_le(notification, 12);
    let fragment_index = notification[14];
    let fragment_count = notification[15];
    if fragment_index != 0 || fragment_count != 1 {
        return Err(ParseError::UnexpectedFragmentFields {
            fragment_index,
            fragment_count,
        });
    }

    let payload_len = read_u16_le(notification, 16);
    let packet_pcm_bytes = read_u16_le(notification, 18);
    let reserved = if usize::from(header_len) >= HEADER_LEN + 4 {
        read_u32_le(notification, HEADER_LEN)
    } else {
        0
    };
    let payload_start = usize::from(header_len);
    let payload_end = payload_start + usize::from(payload_len);
    if payload_end > notification.len() {
        return Err(ParseError::TruncatedPayload {
            expected: payload_end,
            actual: notification.len(),
        });
    }

    Ok(Packet {
        header: PacketHeader {
            packet_type,
            flags,
            header_len,
            session_id,
            packet_sequence: sequence_or_expected,
            expected_packet_count: sequence_or_expected,
            fragment_index,
            fragment_count,
            payload_len,
            packet_pcm_bytes,
            reserved,
        },
        payload: &notification[payload_start..payload_end],
    })
}

pub fn build_session_replay_notifications(
    config: ReplayConfig,
    pcm_bytes: &[u8],
) -> Result<Vec<Vec<u8>>, ReplayBuildError> {
    validate_payload_size(config.payload_pcm_bytes)?;

    let packet_count = pcm_bytes.len().div_ceil(config.payload_pcm_bytes);
    if packet_count > u16::MAX as usize {
        return Err(ReplayBuildError::PacketSequenceOverflow { packet_count });
    }

    let mut notifications = Vec::with_capacity(packet_count + 2);
    notifications.push(build_session_start_notification(config.session_id));

    for (packet_sequence, payload) in pcm_bytes.chunks(config.payload_pcm_bytes).enumerate() {
        notifications.push(build_audio_data_notification(
            config.session_id,
            packet_sequence as u16,
            payload,
        )?);
    }

    notifications.push(build_session_stop_notification(
        config.session_id,
        packet_count as u16,
    ));
    Ok(notifications)
}

pub fn build_session_start_notification(session_id: u32) -> Vec<u8> {
    build_notification(PacketType::SessionStart, session_id, 0, &[], 0)
        .expect("empty control packet fits VKA1 header")
}

pub fn build_audio_data_notification(
    session_id: u32,
    packet_sequence: u16,
    pcm_payload: &[u8],
) -> Result<Vec<u8>, ReplayBuildError> {
    build_notification(
        PacketType::AudioData,
        session_id,
        packet_sequence,
        pcm_payload,
        pcm_payload.len() as u16,
    )
}

pub fn build_session_stop_notification(session_id: u32, expected_packet_count: u16) -> Vec<u8> {
    build_notification(
        PacketType::SessionStop,
        session_id,
        expected_packet_count,
        &[],
        0,
    )
    .expect("empty control packet fits VKA1 header")
}

pub fn build_session_cancel_notification(session_id: u32, expected_packet_count: u16) -> Vec<u8> {
    build_notification(
        PacketType::SessionCancel,
        session_id,
        expected_packet_count,
        &[],
        0,
    )
    .expect("empty control packet fits VKA1 header")
}

pub fn build_session_error_notification(
    session_id: u32,
    expected_packet_count: u16,
    error_code: SessionErrorCode,
) -> Vec<u8> {
    build_notification(
        PacketType::SessionError,
        session_id,
        expected_packet_count,
        &[],
        error_code.wire_value(),
    )
    .expect("empty control packet fits VKA1 header")
}

pub fn collect_notifications<'a, I>(notifications: I) -> Result<SessionCollector, ParseError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut collector = SessionCollector::default();
    for notification in notifications {
        collector.handle_notification(notification)?;
    }
    Ok(collector)
}

fn validate_payload_size(payload_pcm_bytes: usize) -> Result<(), ReplayBuildError> {
    if payload_pcm_bytes == 0 {
        return Err(ReplayBuildError::EmptyPayloadSize);
    }
    if payload_pcm_bytes > u16::MAX as usize {
        return Err(ReplayBuildError::PayloadTooLarge { payload_pcm_bytes });
    }
    Ok(())
}

fn build_notification(
    packet_type: PacketType,
    session_id: u32,
    sequence_or_expected: u16,
    payload: &[u8],
    packet_pcm_bytes: u16,
) -> Result<Vec<u8>, ReplayBuildError> {
    if payload.len() > u16::MAX as usize {
        return Err(ReplayBuildError::PacketPayloadTooLarge {
            pcm_bytes: payload.len(),
        });
    }

    let mut notification = Vec::with_capacity(HEADER_LEN + payload.len());
    notification.extend_from_slice(MAGIC);
    notification.push(packet_type.wire_value());
    notification.push(0);
    notification.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    notification.extend_from_slice(&session_id.to_le_bytes());
    notification.extend_from_slice(&sequence_or_expected.to_le_bytes());
    notification.push(0);
    notification.push(1);
    notification.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    notification.extend_from_slice(&packet_pcm_bytes.to_le_bytes());
    notification.extend_from_slice(payload);
    Ok(notification)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Started {
        session_id: u32,
    },
    AudioData {
        session_id: u32,
        packet_sequence: u16,
        pcm_bytes: usize,
    },
    Stopped {
        session_id: u32,
        expected_packet_count: u16,
    },
    Cancelled {
        session_id: u32,
        expected_packet_count: u16,
    },
    Error {
        session_id: u32,
        expected_packet_count: u16,
        error_code: SessionErrorCode,
    },
    Ignored(IgnoredPacketReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoredPacketReason {
    NonAudioBeforeStart,
    ForeignSession,
    EmptyAudioPayload,
    DuplicateOrShorterPacket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub session_id: Option<u32>,
    pub explicit_start_received: bool,
    pub start_inferred_from_audio: bool,
    pub terminal_received: bool,
    pub end_reason: Option<SessionEndReason>,
    pub expected_packet_count: Option<usize>,
    pub received_packet_count: usize,
    pub missing_packet_count: usize,
    pub missing_packet_indices: Vec<u16>,
    pub received_pcm_bytes: usize,
    pub reconstructed_pcm_bytes: usize,
    pub silence_filled_bytes: usize,
    pub duplicate_packet_count: usize,
    pub replaced_packet_count: usize,
    pub ignored_foreign_packet_count: usize,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedAudioSubmissionResult {
    pub stats: SessionStats,
    pub reconstructed_pcm_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingPcmChunk {
    pub session_id: u32,
    pub packet_sequence: u16,
    pub pcm: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingSessionEvent {
    Started {
        session_id: u32,
    },
    PcmChunk(StreamingPcmChunk),
    Stopped {
        session_id: u32,
        expected_packet_count: u16,
    },
    Cancelled {
        session_id: u32,
        expected_packet_count: u16,
    },
    Error {
        session_id: u32,
        expected_packet_count: u16,
        error_code: SessionErrorCode,
    },
    Ignored(IgnoredPacketReason),
}

#[derive(Debug, Default)]
pub struct StreamingSessionCollector {
    inner: SessionCollector,
}

impl StreamingSessionCollector {
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    pub fn handle_notification(
        &mut self,
        notification: &[u8],
    ) -> Result<StreamingSessionEvent, ParseError> {
        let packet = parse_packet(notification)?;
        Ok(self.handle_packet(packet))
    }

    pub fn handle_packet(&mut self, packet: Packet<'_>) -> StreamingSessionEvent {
        let header = packet.header;
        let pcm = (header.packet_type == PacketType::AudioData)
            .then(|| packet.payload_pcm().to_vec())
            .unwrap_or_default();
        let event = self.inner.handle_packet(packet);
        match event {
            SessionEvent::Started { session_id } => StreamingSessionEvent::Started { session_id },
            SessionEvent::AudioData {
                session_id,
                packet_sequence,
                ..
            } => StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id,
                packet_sequence,
                pcm,
            }),
            SessionEvent::Stopped {
                session_id,
                expected_packet_count,
            } => StreamingSessionEvent::Stopped {
                session_id,
                expected_packet_count,
            },
            SessionEvent::Cancelled {
                session_id,
                expected_packet_count,
            } => StreamingSessionEvent::Cancelled {
                session_id,
                expected_packet_count,
            },
            SessionEvent::Error {
                session_id,
                expected_packet_count,
                error_code,
            } => StreamingSessionEvent::Error {
                session_id,
                expected_packet_count,
                error_code,
            },
            SessionEvent::Ignored(reason) => StreamingSessionEvent::Ignored(reason),
        }
    }

    pub fn inner(&self) -> &SessionCollector {
        &self.inner
    }

    pub fn into_inner(self) -> SessionCollector {
        self.inner
    }
}

#[derive(Debug, Default)]
pub struct SessionCollector {
    session_id: Option<u32>,
    explicit_start_received: bool,
    start_inferred_from_audio: bool,
    terminal_received: bool,
    expected_packet_count: Option<u16>,
    end_reason: Option<SessionEndReason>,
    audio_packets: BTreeMap<u16, Vec<u8>>,
    packet_pcm_bytes: BTreeMap<u16, usize>,
    duplicate_packet_count: usize,
    replaced_packet_count: usize,
    ignored_foreign_packet_count: usize,
}

impl SessionCollector {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn handle_notification(&mut self, notification: &[u8]) -> Result<SessionEvent, ParseError> {
        let packet = parse_packet(notification)?;
        Ok(self.handle_packet(packet))
    }

    pub fn handle_packet(&mut self, packet: Packet<'_>) -> SessionEvent {
        let header = packet.header;
        match header.packet_type {
            PacketType::SessionStart => {
                self.reset_for_session(header.session_id);
                self.explicit_start_received = true;
                self.start_inferred_from_audio = false;
                SessionEvent::Started {
                    session_id: header.session_id,
                }
            }
            PacketType::AudioData => self.handle_audio_packet(packet),
            PacketType::SessionStop => {
                if !self.accepts_terminal_packet(header.session_id) {
                    return SessionEvent::Ignored(self.last_ignored_reason());
                }
                self.terminal_received = true;
                self.expected_packet_count = Some(header.expected_packet_count);
                self.end_reason = Some(SessionEndReason::Stop);
                SessionEvent::Stopped {
                    session_id: header.session_id,
                    expected_packet_count: header.expected_packet_count,
                }
            }
            PacketType::SessionCancel => {
                if !self.accepts_terminal_packet(header.session_id) {
                    return SessionEvent::Ignored(self.last_ignored_reason());
                }
                self.terminal_received = true;
                self.expected_packet_count = Some(header.expected_packet_count);
                self.end_reason = Some(SessionEndReason::Cancel);
                SessionEvent::Cancelled {
                    session_id: header.session_id,
                    expected_packet_count: header.expected_packet_count,
                }
            }
            PacketType::SessionError => {
                if !self.accepts_terminal_packet(header.session_id) {
                    return SessionEvent::Ignored(self.last_ignored_reason());
                }
                let error_code = SessionErrorCode::from_wire(header.packet_pcm_bytes);
                self.terminal_received = true;
                self.expected_packet_count = Some(header.expected_packet_count);
                self.end_reason = Some(SessionEndReason::Error(error_code));
                SessionEvent::Error {
                    session_id: header.session_id,
                    expected_packet_count: header.expected_packet_count,
                    error_code,
                }
            }
        }
    }

    pub fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    pub fn has_successful_complete_session(&self) -> bool {
        self.session_id.is_some()
            && self.terminal_received
            && self.end_reason == Some(SessionEndReason::Stop)
            && self.expected_packet_count.is_some()
            && self.expected_packet_count != Some(0)
            && self.missing_packet_indices().is_empty()
    }

    pub fn terminal_received(&self) -> bool {
        self.terminal_received
    }

    pub fn received_packet_count(&self) -> usize {
        self.audio_packets.len()
    }

    pub fn inferred_expected_packet_count_from_received(&self) -> Option<u16> {
        self.audio_packets
            .keys()
            .next_back()
            .map(|sequence| sequence.saturating_add(1))
    }

    pub fn received_pcm_bytes(&self) -> usize {
        self.packet_pcm_bytes.values().sum()
    }

    pub fn missing_packet_indices(&self) -> Vec<u16> {
        let Some(expected_packet_count) = self.expected_packet_count else {
            return Vec::new();
        };
        (0..expected_packet_count)
            .filter(|sequence| !self.audio_packets.contains_key(sequence))
            .collect()
    }

    pub fn reconstructed_pcm(&self) -> Vec<u8> {
        let Some(expected_packet_count) = self.expected_packet_count else {
            return self
                .audio_packets
                .values()
                .flat_map(|payload| payload.iter().copied())
                .collect();
        };

        let mut pcm = Vec::new();
        for sequence in 0..expected_packet_count {
            if let Some(payload) = self.audio_packets.get(&sequence) {
                pcm.extend_from_slice(payload);
            } else {
                let silence_bytes = self.inferred_packet_pcm_bytes(sequence);
                pcm.resize(pcm.len() + silence_bytes, 0);
            }
        }
        pcm
    }

    pub fn stats(&self) -> SessionStats {
        let missing_packet_indices = self.missing_packet_indices();
        let silence_filled_bytes = missing_packet_indices
            .iter()
            .map(|sequence| self.inferred_packet_pcm_bytes(*sequence))
            .sum();
        let reconstructed_pcm_bytes = self.reconstructed_pcm_len();

        SessionStats {
            session_id: self.session_id,
            explicit_start_received: self.explicit_start_received,
            start_inferred_from_audio: self.start_inferred_from_audio,
            terminal_received: self.terminal_received,
            end_reason: self.end_reason,
            expected_packet_count: self.expected_packet_count.map(usize::from),
            received_packet_count: self.received_packet_count(),
            missing_packet_count: missing_packet_indices.len(),
            missing_packet_indices,
            received_pcm_bytes: self.received_pcm_bytes(),
            reconstructed_pcm_bytes,
            silence_filled_bytes,
            duplicate_packet_count: self.duplicate_packet_count,
            replaced_packet_count: self.replaced_packet_count,
            ignored_foreign_packet_count: self.ignored_foreign_packet_count,
            duration_seconds: reconstructed_pcm_bytes as f64 / PCM_BYTES_PER_SECOND as f64,
        }
    }

    fn handle_audio_packet(&mut self, packet: Packet<'_>) -> SessionEvent {
        let header = packet.header;
        if self.session_id.is_none() {
            self.reset_for_session(header.session_id);
            self.start_inferred_from_audio = true;
        } else if self.session_id != Some(header.session_id) {
            self.ignored_foreign_packet_count += 1;
            return SessionEvent::Ignored(IgnoredPacketReason::ForeignSession);
        }

        let payload = packet.payload_pcm();
        if payload.is_empty() {
            return SessionEvent::Ignored(IgnoredPacketReason::EmptyAudioPayload);
        }

        let packet_sequence = header.packet_sequence;
        if self
            .audio_packets
            .get(&packet_sequence)
            .is_some_and(|existing| existing.len() >= payload.len())
        {
            self.duplicate_packet_count += 1;
            return SessionEvent::Ignored(IgnoredPacketReason::DuplicateOrShorterPacket);
        }

        if self.audio_packets.contains_key(&packet_sequence) {
            self.replaced_packet_count += 1;
        }

        self.audio_packets.insert(packet_sequence, payload.to_vec());
        self.packet_pcm_bytes.insert(packet_sequence, payload.len());

        SessionEvent::AudioData {
            session_id: header.session_id,
            packet_sequence,
            pcm_bytes: payload.len(),
        }
    }

    fn reset_for_session(&mut self, session_id: u32) {
        if self.session_id == Some(session_id) {
            return;
        }
        self.reset();
        self.session_id = Some(session_id);
    }

    fn accepts_terminal_packet(&mut self, session_id: u32) -> bool {
        if self.session_id.is_none() {
            return false;
        }
        if self.session_id != Some(session_id) {
            self.ignored_foreign_packet_count += 1;
            return false;
        }
        true
    }

    fn last_ignored_reason(&self) -> IgnoredPacketReason {
        if self.session_id.is_none() {
            IgnoredPacketReason::NonAudioBeforeStart
        } else {
            IgnoredPacketReason::ForeignSession
        }
    }

    fn reconstructed_pcm_len(&self) -> usize {
        let Some(expected_packet_count) = self.expected_packet_count else {
            return self.received_pcm_bytes();
        };

        (0..expected_packet_count)
            .map(|sequence| {
                self.packet_pcm_bytes
                    .get(&sequence)
                    .copied()
                    .unwrap_or_else(|| self.inferred_packet_pcm_bytes(sequence))
            })
            .sum()
    }

    fn inferred_packet_pcm_bytes(&self, packet_sequence: u16) -> usize {
        if let Some(actual) = self.packet_pcm_bytes.get(&packet_sequence) {
            return *actual;
        }
        if self.packet_pcm_bytes.is_empty() {
            return 0;
        }

        let cycle_length = self.inferred_cycle_length();
        if let Some(size) = self
            .build_cycle_size_map(cycle_length)
            .get(&(usize::from(packet_sequence) % cycle_length))
        {
            return *size;
        }

        most_common_size(self.packet_size_counts()).unwrap_or(0)
    }

    fn inferred_cycle_length(&self) -> usize {
        if self.packet_pcm_bytes.len() < 6 {
            return 1;
        }

        let mut best_cycle_length = 1;
        let mut best_match_count = 0;
        let mut best_score = -1.0f64;
        for cycle_length in 1..=16 {
            let buckets = self.build_cycle_size_map(cycle_length);
            let match_count = self
                .packet_pcm_bytes
                .iter()
                .filter(|(sequence, size)| {
                    buckets
                        .get(&(usize::from(**sequence) % cycle_length))
                        .is_some_and(|predicted| predicted == *size)
                })
                .count();
            let score = match_count as f64 / self.packet_pcm_bytes.len() as f64;
            if score > best_score
                || ((score - best_score).abs() < f64::EPSILON && match_count > best_match_count)
            {
                best_cycle_length = cycle_length;
                best_match_count = match_count;
                best_score = score;
            }
        }
        best_cycle_length
    }

    fn build_cycle_size_map(&self, cycle_length: usize) -> BTreeMap<usize, usize> {
        if cycle_length == 0 {
            return BTreeMap::new();
        }

        let mut buckets: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
        for (sequence, size) in &self.packet_pcm_bytes {
            *buckets
                .entry(usize::from(*sequence) % cycle_length)
                .or_default()
                .entry(*size)
                .or_default() += 1;
        }

        buckets
            .into_iter()
            .filter_map(|(offset, counts)| most_common_size(counts).map(|size| (offset, size)))
            .collect()
    }

    fn packet_size_counts(&self) -> BTreeMap<usize, usize> {
        let mut counts = BTreeMap::new();
        for size in self.packet_pcm_bytes.values() {
            *counts.entry(*size).or_default() += 1;
        }
        counts
    }
}

fn most_common_size(counts: BTreeMap<usize, usize>) -> Option<usize> {
    counts
        .into_iter()
        .max_by(|(left_size, left_count), (right_size, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_size.cmp(left_size))
        })
        .map(|(size, _)| size)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_maps_legacy_chunk_field_to_packet_sequence() {
        let notification = packet(PacketType::AudioData, 7, 42, &[1, 2, 3, 4], Some(2));

        let parsed = parse_packet(&notification).expect("parse packet");

        assert_eq!(parsed.header.packet_type, PacketType::AudioData);
        assert_eq!(parsed.header.session_id, 7);
        assert_eq!(parsed.header.packet_sequence, 42);
        assert_eq!(parsed.header.expected_packet_count, 42);
        assert_eq!(parsed.payload, &[1, 2, 3, 4]);
        assert_eq!(parsed.payload_pcm(), &[1, 2]);
    }

    #[test]
    fn parser_rejects_fragmented_legacy_packets() {
        let mut notification = packet(PacketType::AudioData, 7, 0, &[1, 2], None);
        notification[15] = 2;

        assert_eq!(
            parse_packet(&notification),
            Err(ParseError::UnexpectedFragmentFields {
                fragment_index: 0,
                fragment_count: 2,
            })
        );
    }

    #[test]
    fn collector_reconstructs_out_of_order_success_session() {
        let mut collector = SessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::SessionStart, 100, 0, &[], Some(0)))
            .expect("start");
        collector
            .handle_notification(&packet(PacketType::AudioData, 100, 1, &[3, 4], None))
            .expect("audio 1");
        collector
            .handle_notification(&packet(PacketType::AudioData, 100, 0, &[1, 2], None))
            .expect("audio 0");
        collector
            .handle_notification(&packet(PacketType::SessionStop, 100, 2, &[], Some(0)))
            .expect("stop");

        assert!(collector.has_successful_complete_session());
        assert_eq!(collector.reconstructed_pcm(), vec![1, 2, 3, 4]);

        let stats = collector.stats();
        assert_eq!(stats.session_id, Some(100));
        assert_eq!(stats.expected_packet_count, Some(2));
        assert_eq!(stats.received_packet_count, 2);
        assert_eq!(stats.missing_packet_count, 0);
        assert_eq!(stats.received_pcm_bytes, 4);
        assert_eq!(stats.reconstructed_pcm_bytes, 4);
        assert_eq!(stats.silence_filled_bytes, 0);
    }

    #[test]
    fn collector_fills_missing_packets_with_inferred_silence() {
        let mut collector = SessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::SessionStart, 101, 0, &[], Some(0)))
            .expect("start");
        collector
            .handle_notification(&packet(PacketType::AudioData, 101, 0, &[1, 2, 3, 4], None))
            .expect("audio 0");
        collector
            .handle_notification(&packet(
                PacketType::AudioData,
                101,
                2,
                &[9, 10, 11, 12],
                None,
            ))
            .expect("audio 2");
        collector
            .handle_notification(&packet(PacketType::SessionStop, 101, 3, &[], Some(0)))
            .expect("stop");

        assert!(!collector.has_successful_complete_session());
        assert_eq!(collector.missing_packet_indices(), vec![1]);
        assert_eq!(
            collector.reconstructed_pcm(),
            vec![1, 2, 3, 4, 0, 0, 0, 0, 9, 10, 11, 12]
        );

        let stats = collector.stats();
        assert_eq!(stats.missing_packet_count, 1);
        assert_eq!(stats.silence_filled_bytes, 4);
        assert_eq!(stats.reconstructed_pcm_bytes, 12);
    }

    #[test]
    fn collector_keeps_larger_duplicate_packet() {
        let mut collector = SessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::AudioData, 102, 0, &[1, 2], None))
            .expect("audio small");
        collector
            .handle_notification(&packet(PacketType::AudioData, 102, 0, &[1, 2, 3, 4], None))
            .expect("audio large");
        let duplicate = collector
            .handle_notification(&packet(PacketType::AudioData, 102, 0, &[9], None))
            .expect("audio duplicate");

        assert_eq!(
            duplicate,
            SessionEvent::Ignored(IgnoredPacketReason::DuplicateOrShorterPacket)
        );
        assert_eq!(collector.reconstructed_pcm(), vec![1, 2, 3, 4]);

        let stats = collector.stats();
        assert_eq!(stats.start_inferred_from_audio, true);
        assert_eq!(stats.replaced_packet_count, 1);
        assert_eq!(stats.duplicate_packet_count, 1);
    }

    #[test]
    fn collector_records_session_error_code() {
        let mut collector = SessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::SessionStart, 103, 0, &[], Some(0)))
            .expect("start");
        collector
            .handle_notification(&packet(PacketType::SessionError, 103, 5, &[], Some(3)))
            .expect("error");

        assert!(collector.terminal_received());
        assert_eq!(
            collector.stats().end_reason,
            Some(SessionEndReason::Error(SessionErrorCode::LinkLost))
        );
    }

    #[test]
    fn replay_builder_roundtrips_pcm_fixture_through_collector() {
        let pcm = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let notifications = build_session_replay_notifications(
            ReplayConfig {
                session_id: 200,
                payload_pcm_bytes: 3,
            },
            &pcm,
        )
        .expect("build replay");

        assert_eq!(notifications.len(), 5);
        assert_eq!(
            parse_packet(&notifications[0])
                .expect("start")
                .header
                .packet_type,
            PacketType::SessionStart
        );
        assert_eq!(
            parse_packet(&notifications[4])
                .expect("stop")
                .header
                .expected_packet_count,
            3
        );

        let notification_slices = notifications.iter().map(Vec::as_slice);
        let collector = collect_notifications(notification_slices).expect("collect replay");

        assert!(collector.has_successful_complete_session());
        assert_eq!(collector.reconstructed_pcm(), pcm);
        assert_eq!(collector.stats().expected_packet_count, Some(3));
    }

    #[test]
    fn streaming_collector_emits_pcm_chunk_per_audio_packet() {
        let mut collector = StreamingSessionCollector::default();

        let started = collector
            .handle_notification(&packet(PacketType::SessionStart, 205, 0, &[], Some(0)))
            .expect("start");
        let chunk_0 = collector
            .handle_notification(&packet(PacketType::AudioData, 205, 0, &[1, 2], None))
            .expect("audio 0");
        let chunk_1 = collector
            .handle_notification(&packet(PacketType::AudioData, 205, 1, &[3, 4], None))
            .expect("audio 1");
        let stopped = collector
            .handle_notification(&packet(PacketType::SessionStop, 205, 2, &[], Some(0)))
            .expect("stop");

        assert_eq!(started, StreamingSessionEvent::Started { session_id: 205 });
        assert_eq!(
            chunk_0,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 205,
                packet_sequence: 0,
                pcm: vec![1, 2],
            })
        );
        assert_eq!(
            chunk_1,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 205,
                packet_sequence: 1,
                pcm: vec![3, 4],
            })
        );
        assert_eq!(
            stopped,
            StreamingSessionEvent::Stopped {
                session_id: 205,
                expected_packet_count: 2,
            }
        );
        assert!(collector.inner().has_successful_complete_session());
        assert_eq!(collector.inner().reconstructed_pcm(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn streaming_collector_accepts_tail_audio_after_stop() {
        let mut collector = StreamingSessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::SessionStart, 209, 0, &[], Some(0)))
            .expect("start");
        collector
            .handle_notification(&packet(PacketType::AudioData, 209, 0, &[1, 2], None))
            .expect("audio 0");
        let stopped = collector
            .handle_notification(&packet(PacketType::SessionStop, 209, 2, &[], Some(0)))
            .expect("stop");

        assert_eq!(
            stopped,
            StreamingSessionEvent::Stopped {
                session_id: 209,
                expected_packet_count: 2,
            }
        );
        assert!(!collector.inner().has_successful_complete_session());
        assert_eq!(collector.inner().missing_packet_indices(), vec![1]);

        let tail = collector
            .handle_notification(&packet(PacketType::AudioData, 209, 1, &[3, 4], None))
            .expect("tail audio");

        assert_eq!(
            tail,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 209,
                packet_sequence: 1,
                pcm: vec![3, 4],
            })
        );
        assert!(collector.inner().has_successful_complete_session());
        assert_eq!(collector.inner().reconstructed_pcm(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn streaming_collector_preserves_batch_collector_behavior_for_duplicates() {
        let mut collector = StreamingSessionCollector::default();

        collector
            .handle_notification(&packet(PacketType::AudioData, 206, 0, &[1, 2], None))
            .expect("audio small");
        let replacement = collector
            .handle_notification(&packet(PacketType::AudioData, 206, 0, &[1, 2, 3, 4], None))
            .expect("audio larger");
        let duplicate = collector
            .handle_notification(&packet(PacketType::AudioData, 206, 0, &[9], None))
            .expect("audio duplicate");

        assert_eq!(
            replacement,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 206,
                packet_sequence: 0,
                pcm: vec![1, 2, 3, 4],
            })
        );
        assert_eq!(
            duplicate,
            StreamingSessionEvent::Ignored(IgnoredPacketReason::DuplicateOrShorterPacket)
        );
        assert_eq!(collector.inner().reconstructed_pcm(), vec![1, 2, 3, 4]);
        assert_eq!(collector.inner().stats().replaced_packet_count, 1);
        assert_eq!(collector.inner().stats().duplicate_packet_count, 1);
    }

    #[test]
    fn streaming_collector_surfaces_cancel_and_error_terminal_events() {
        let mut cancel_collector = StreamingSessionCollector::default();
        cancel_collector
            .handle_notification(&build_session_start_notification(207))
            .expect("cancel start");
        let cancelled = cancel_collector
            .handle_notification(&build_session_cancel_notification(207, 0))
            .expect("cancel");

        assert_eq!(
            cancelled,
            StreamingSessionEvent::Cancelled {
                session_id: 207,
                expected_packet_count: 0,
            }
        );
        assert_eq!(
            cancel_collector.inner().stats().end_reason,
            Some(SessionEndReason::Cancel)
        );

        let mut error_collector = StreamingSessionCollector::default();
        error_collector
            .handle_notification(&build_session_start_notification(208))
            .expect("error start");
        let errored = error_collector
            .handle_notification(&build_session_error_notification(
                208,
                1,
                SessionErrorCode::LinkLost,
            ))
            .expect("error");

        assert_eq!(
            errored,
            StreamingSessionEvent::Error {
                session_id: 208,
                expected_packet_count: 1,
                error_code: SessionErrorCode::LinkLost,
            }
        );
        assert_eq!(
            error_collector.inner().stats().end_reason,
            Some(SessionEndReason::Error(SessionErrorCode::LinkLost))
        );
    }

    #[test]
    fn replay_builder_missing_packet_is_visible_in_stats() {
        let pcm = vec![10, 11, 12, 13, 14, 15, 16, 17];
        let mut notifications = build_session_replay_notifications(
            ReplayConfig {
                session_id: 201,
                payload_pcm_bytes: 2,
            },
            &pcm,
        )
        .expect("build replay");

        notifications.remove(2);

        let collector =
            collect_notifications(notifications.iter().map(Vec::as_slice)).expect("collect replay");

        assert!(!collector.has_successful_complete_session());
        assert_eq!(collector.missing_packet_indices(), vec![1]);
        assert_eq!(
            collector.reconstructed_pcm(),
            vec![10, 11, 0, 0, 14, 15, 16, 17]
        );

        let stats = collector.stats();
        assert_eq!(stats.missing_packet_count, 1);
        assert_eq!(stats.silence_filled_bytes, 2);
        assert_eq!(stats.received_packet_count, 3);
    }

    #[test]
    fn replay_builder_rejects_unbounded_packet_sequences() {
        let pcm = vec![0; u16::MAX as usize + 1];

        assert_eq!(
            build_session_replay_notifications(
                ReplayConfig {
                    session_id: 202,
                    payload_pcm_bytes: 1,
                },
                &pcm,
            ),
            Err(ReplayBuildError::PacketSequenceOverflow {
                packet_count: u16::MAX as usize + 1
            })
        );
    }

    #[test]
    fn replay_error_notification_maps_session_error_code() {
        let notifications = [
            build_session_start_notification(203),
            build_session_error_notification(203, 0, SessionErrorCode::NotifyTimeout),
        ];

        let collector =
            collect_notifications(notifications.iter().map(Vec::as_slice)).expect("collect replay");

        assert_eq!(
            collector.stats().end_reason,
            Some(SessionEndReason::Error(SessionErrorCode::NotifyTimeout))
        );
    }

    #[test]
    fn wav_decoder_accepts_16k_mono_pcm() {
        let pcm = vec![1, 0, 2, 0, 3, 0, 4, 0];
        let wav = test_wav(
            &pcm,
            PCM_SAMPLE_RATE_HZ,
            PCM_CHANNELS,
            PCM_SAMPLE_WIDTH_BITS,
        );

        assert_eq!(
            decode_input_pcm(&wav, EmbeddedAudioInputFormat::Wav).expect("decode wav"),
            pcm
        );
    }

    #[test]
    fn wav_decoder_rejects_wrong_sample_rate() {
        let wav = test_wav(&[0, 0, 1, 0], 8_000, PCM_CHANNELS, PCM_SAMPLE_WIDTH_BITS);

        assert!(matches!(
            decode_input_pcm(&wav, EmbeddedAudioInputFormat::Wav),
            Err(InputAudioError::UnsupportedWavFormat { .. })
        ));
    }

    #[test]
    fn replay_cancel_notification_maps_cancel_reason() {
        let notifications = [
            build_session_start_notification(204),
            build_session_cancel_notification(204, 0),
        ];

        let collector =
            collect_notifications(notifications.iter().map(Vec::as_slice)).expect("collect replay");

        assert_eq!(collector.session_id(), Some(204));
        assert_eq!(collector.stats().end_reason, Some(SessionEndReason::Cancel));
    }

    fn test_wav(pcm: &[u8], sample_rate_hz: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
        let data_size = pcm.len() as u32;
        let byte_rate = sample_rate_hz * channels as u32 * bits_per_sample as u32 / 8;
        let block_align = channels * bits_per_sample / 8;
        let mut wav = Vec::with_capacity(44 + pcm.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm);
        wav
    }

    fn packet(
        packet_type: PacketType,
        session_id: u32,
        sequence_or_expected: u16,
        payload: &[u8],
        packet_pcm_bytes: Option<u16>,
    ) -> Vec<u8> {
        let packet_pcm_bytes = packet_pcm_bytes.unwrap_or(payload.len() as u16);
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.push(packet_type.wire_value());
        bytes.push(0);
        bytes.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        bytes.extend_from_slice(&session_id.to_le_bytes());
        bytes.extend_from_slice(&sequence_or_expected.to_le_bytes());
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&packet_pcm_bytes.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }
}
