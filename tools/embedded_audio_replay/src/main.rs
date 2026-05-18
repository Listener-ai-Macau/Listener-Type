#![allow(dead_code)]

#[path = "../../../src-tauri/src/embedded_audio.rs"]
mod embedded_audio;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use embedded_audio::{
    build_session_cancel_notification, build_session_error_notification,
    build_session_replay_notifications, collect_notifications, parse_packet, read_input_pcm,
    EmbeddedAudioInputFormat, PacketType, ReplayConfig, SessionEndReason, SessionErrorCode,
    StreamingSessionCollector, StreamingSessionEvent, DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
    PCM_CHANNELS, PCM_SAMPLE_RATE_HZ, PCM_SAMPLE_WIDTH_BITS,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

const USAGE: &str = "\
Usage:
  listener-embedded-audio-replay --input <file> [options]

Options:
  --format <wav|pcm16le>      Input format. Defaults to wav for .wav, otherwise pcm16le.
  --mode <batch|stream>       Replay collector mode. Defaults to batch.
  --session-id <u32>          VKA1 session id. Defaults to 1.
  --payload-bytes <usize>     Audio payload bytes per VKA1 audio_data packet. Defaults to 480.
  --drop-packet <u16>         Drop one audio_data packet sequence. Can be repeated.
  --terminal <stop|cancel|error>
                              Terminal packet to emit. Defaults to stop.
  --json-out <file>           Write pretty JSON report to this file.
  --help                      Show this help.
";

#[derive(Debug)]
struct Args {
    input: PathBuf,
    format: Option<EmbeddedAudioInputFormat>,
    mode: ReplayMode,
    session_id: u32,
    payload_bytes: usize,
    drop_packets: BTreeSet<u16>,
    terminal: TerminalMode,
    json_out: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReplayMode {
    Batch,
    Stream,
}

impl std::str::FromStr for ReplayMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "batch" => Ok(Self::Batch),
            "stream" | "streaming" => Ok(Self::Stream),
            other => Err(format!("unsupported replay mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TerminalMode {
    Stop,
    Cancel,
    Error,
}

impl std::str::FromStr for TerminalMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "stop" => Ok(Self::Stop),
            "cancel" | "cancelled" => Ok(Self::Cancel),
            "error" => Ok(Self::Error),
            other => Err(format!("unsupported terminal mode: {other}")),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayReport {
    status: &'static str,
    reason: Option<String>,
    mode: ReplayMode,
    terminal: TerminalMode,
    input_path: String,
    input_format: EmbeddedAudioInputFormat,
    session_id: u32,
    payload_pcm_bytes: usize,
    dropped_packet_sequences: Vec<u16>,
    input_pcm_bytes: usize,
    input_pcm_sha256: String,
    notification_count: usize,
    audio_notification_count: usize,
    reconstructed_matches_input: bool,
    reconstructed_pcm_sha256: String,
    audio_format: AudioFormatReport,
    stats: embedded_audio::SessionStats,
    streaming: Option<StreamingReplayReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioFormatReport {
    encoding: &'static str,
    sample_rate_hz: u32,
    channels: u16,
    sample_width_bits: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamingReplayReport {
    status: &'static str,
    reason: Option<String>,
    event_count: usize,
    pcm_chunk_count: usize,
    chunk_packet_sequences: Vec<u16>,
    chunk_pcm_bytes: Vec<usize>,
    chunk_sequence_is_strictly_increasing: bool,
    streamed_pcm_bytes: usize,
    streamed_pcm_sha256: String,
    streamed_pcm_matches_input: bool,
    terminal_event: Option<StreamingEventReport>,
    events: Vec<StreamingEventReport>,
    stats: embedded_audio::SessionStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamingEventReport {
    kind: &'static str,
    session_id: Option<u32>,
    packet_sequence: Option<u16>,
    pcm_bytes: Option<usize>,
    expected_packet_count: Option<u16>,
    error_code: Option<SessionErrorCode>,
    ignored_reason: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args().skip(1))?;
    let format = args
        .format
        .unwrap_or_else(|| EmbeddedAudioInputFormat::infer_from_path(&args.input));
    let pcm = read_input_pcm(&args.input, Some(format)).map_err(|err| err.to_string())?;
    let mut notifications = build_session_replay_notifications(
        ReplayConfig {
            session_id: args.session_id,
            payload_pcm_bytes: args.payload_bytes,
        },
        &pcm,
    )
    .map_err(|err| err.to_string())?;

    apply_terminal_mode(&mut notifications, args.session_id, args.terminal)?;

    let original_notification_count = notifications.len();
    notifications.retain(|notification| {
        parse_packet(notification)
            .map(|packet| {
                packet.header.packet_type != PacketType::AudioData
                    || !args.drop_packets.contains(&packet.header.packet_sequence)
            })
            .unwrap_or(true)
    });

    let collector = collect_notifications(notifications.iter().map(Vec::as_slice))
        .map_err(|err| err.to_string())?;
    let reconstructed_pcm = collector.reconstructed_pcm();
    let reconstructed_matches_input = reconstructed_pcm == pcm;
    let stats = collector.stats();
    let streaming = if args.mode == ReplayMode::Stream {
        Some(run_streaming_replay(&notifications, &pcm, args.terminal)?)
    } else {
        None
    };
    let status = report_status(
        args.mode,
        &stats,
        reconstructed_matches_input,
        streaming.as_ref(),
    );
    let report = ReplayReport {
        status,
        reason: report_reason(
            args.mode,
            &stats,
            reconstructed_matches_input,
            streaming.as_ref(),
        ),
        mode: args.mode,
        terminal: args.terminal,
        input_path: args.input.display().to_string(),
        input_format: format,
        session_id: args.session_id,
        payload_pcm_bytes: args.payload_bytes,
        dropped_packet_sequences: args.drop_packets.iter().copied().collect(),
        input_pcm_bytes: pcm.len(),
        input_pcm_sha256: sha256_hex(&pcm),
        notification_count: notifications.len(),
        audio_notification_count: original_notification_count.saturating_sub(2),
        reconstructed_matches_input,
        reconstructed_pcm_sha256: sha256_hex(&reconstructed_pcm),
        audio_format: AudioFormatReport {
            encoding: "pcm_s16le",
            sample_rate_hz: PCM_SAMPLE_RATE_HZ,
            channels: PCM_CHANNELS,
            sample_width_bits: PCM_SAMPLE_WIDTH_BITS,
        },
        stats,
        streaming,
    };

    let pretty = serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?;
    if let Some(json_out) = args.json_out {
        if let Some(parent) = json_out.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create json output dir: {err}"))?;
        }
        fs::write(&json_out, pretty).map_err(|err| format!("write json output: {err}"))?;
    } else {
        println!("{pretty}");
    }

    let compact = serde_json::to_string(&report).map_err(|err| err.to_string())?;
    println!("replay_result_json={compact}");
    Ok(())
}

fn parse_args<I>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = String>,
{
    let mut input = None;
    let mut format = None;
    let mut mode = ReplayMode::Batch;
    let mut session_id = 1u32;
    let mut payload_bytes = DEFAULT_REPLAY_PAYLOAD_PCM_BYTES;
    let mut drop_packets = BTreeSet::new();
    let mut terminal = TerminalMode::Stop;
    let mut json_out = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--input" => {
                input = Some(PathBuf::from(next_value(&mut args, "--input")?));
            }
            "--format" => {
                format = Some(next_value(&mut args, "--format")?.parse()?);
            }
            "--mode" => {
                mode = next_value(&mut args, "--mode")?.parse()?;
            }
            "--session-id" => {
                session_id = parse_value(&next_value(&mut args, "--session-id")?, "--session-id")?;
            }
            "--payload-bytes" => {
                payload_bytes = parse_value(
                    &next_value(&mut args, "--payload-bytes")?,
                    "--payload-bytes",
                )?;
            }
            "--drop-packet" => {
                drop_packets.insert(parse_value(
                    &next_value(&mut args, "--drop-packet")?,
                    "--drop-packet",
                )?);
            }
            "--terminal" => {
                terminal = next_value(&mut args, "--terminal")?.parse()?;
            }
            "--json-out" => {
                json_out = Some(PathBuf::from(next_value(&mut args, "--json-out")?));
            }
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }

    Ok(Args {
        input: input.ok_or_else(|| format!("missing --input\n\n{USAGE}"))?,
        format,
        mode,
        session_id,
        payload_bytes,
        drop_packets,
        terminal,
        json_out,
    })
}

fn next_value<I>(args: &mut I, name: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("missing value for {name}"))
}

fn parse_value<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|err| format!("invalid value for {name}: {err}"))
}

fn apply_terminal_mode(
    notifications: &mut [Vec<u8>],
    session_id: u32,
    terminal: TerminalMode,
) -> Result<(), String> {
    let terminal_index = notifications
        .len()
        .checked_sub(1)
        .ok_or_else(|| "replay notification list is empty".to_string())?;
    let expected_packet_count = notifications
        .get(terminal_index)
        .and_then(|notification| parse_packet(notification).ok())
        .map(|packet| packet.header.expected_packet_count)
        .ok_or_else(|| "failed to read replay terminal packet".to_string())?;

    notifications[terminal_index] = match terminal {
        TerminalMode::Stop => build_terminal_stop(session_id, expected_packet_count),
        TerminalMode::Cancel => {
            build_session_cancel_notification(session_id, expected_packet_count)
        }
        TerminalMode::Error => build_session_error_notification(
            session_id,
            expected_packet_count,
            SessionErrorCode::LinkLost,
        ),
    };
    Ok(())
}

fn build_terminal_stop(session_id: u32, expected_packet_count: u16) -> Vec<u8> {
    embedded_audio::build_session_stop_notification(session_id, expected_packet_count)
}

fn run_streaming_replay(
    notifications: &[Vec<u8>],
    input_pcm: &[u8],
    terminal: TerminalMode,
) -> Result<StreamingReplayReport, String> {
    let mut collector = StreamingSessionCollector::default();
    let mut events = Vec::new();
    let mut streamed_pcm = Vec::new();
    let mut chunk_packet_sequences = Vec::new();
    let mut chunk_pcm_bytes = Vec::new();
    let mut terminal_event = None;

    for notification in notifications {
        let event = collector
            .handle_notification(notification)
            .map_err(|err| err.to_string())?;
        let report = streaming_event_report(&event);
        if let StreamingSessionEvent::PcmChunk(chunk) = &event {
            chunk_packet_sequences.push(chunk.packet_sequence);
            chunk_pcm_bytes.push(chunk.pcm.len());
            streamed_pcm.extend_from_slice(&chunk.pcm);
        }
        if matches!(
            event,
            StreamingSessionEvent::Stopped { .. }
                | StreamingSessionEvent::Cancelled { .. }
                | StreamingSessionEvent::Error { .. }
        ) {
            terminal_event = Some(report.clone());
        }
        events.push(report);
    }

    let streamed_pcm_matches_input = streamed_pcm == input_pcm;
    let chunk_sequence_is_strictly_increasing = is_strictly_increasing(&chunk_packet_sequences);
    let stats = collector.inner().stats();
    let status = streaming_status(
        terminal,
        terminal_event.as_ref(),
        streamed_pcm_matches_input,
        chunk_sequence_is_strictly_increasing,
    );
    let reason = streaming_reason(
        terminal,
        terminal_event.as_ref(),
        streamed_pcm_matches_input,
        chunk_sequence_is_strictly_increasing,
    );
    Ok(StreamingReplayReport {
        status,
        reason,
        event_count: events.len(),
        pcm_chunk_count: chunk_packet_sequences.len(),
        chunk_packet_sequences,
        chunk_pcm_bytes,
        chunk_sequence_is_strictly_increasing,
        streamed_pcm_bytes: streamed_pcm.len(),
        streamed_pcm_sha256: sha256_hex(&streamed_pcm),
        streamed_pcm_matches_input,
        terminal_event,
        events,
        stats,
    })
}

fn streaming_event_report(event: &StreamingSessionEvent) -> StreamingEventReport {
    match event {
        StreamingSessionEvent::Started { session_id } => StreamingEventReport {
            kind: "started",
            session_id: Some(*session_id),
            packet_sequence: None,
            pcm_bytes: None,
            expected_packet_count: None,
            error_code: None,
            ignored_reason: None,
        },
        StreamingSessionEvent::PcmChunk(chunk) => StreamingEventReport {
            kind: "pcm_chunk",
            session_id: Some(chunk.session_id),
            packet_sequence: Some(chunk.packet_sequence),
            pcm_bytes: Some(chunk.pcm.len()),
            expected_packet_count: None,
            error_code: None,
            ignored_reason: None,
        },
        StreamingSessionEvent::Stopped {
            session_id,
            expected_packet_count,
        } => StreamingEventReport {
            kind: "stopped",
            session_id: Some(*session_id),
            packet_sequence: None,
            pcm_bytes: None,
            expected_packet_count: Some(*expected_packet_count),
            error_code: None,
            ignored_reason: None,
        },
        StreamingSessionEvent::Cancelled {
            session_id,
            expected_packet_count,
        } => StreamingEventReport {
            kind: "cancelled",
            session_id: Some(*session_id),
            packet_sequence: None,
            pcm_bytes: None,
            expected_packet_count: Some(*expected_packet_count),
            error_code: None,
            ignored_reason: None,
        },
        StreamingSessionEvent::Error {
            session_id,
            expected_packet_count,
            error_code,
        } => StreamingEventReport {
            kind: "error",
            session_id: Some(*session_id),
            packet_sequence: None,
            pcm_bytes: None,
            expected_packet_count: Some(*expected_packet_count),
            error_code: Some(*error_code),
            ignored_reason: None,
        },
        StreamingSessionEvent::Ignored(reason) => StreamingEventReport {
            kind: "ignored",
            session_id: None,
            packet_sequence: None,
            pcm_bytes: None,
            expected_packet_count: None,
            error_code: None,
            ignored_reason: Some(format!("{reason:?}")),
        },
    }
}

fn report_status(
    mode: ReplayMode,
    stats: &embedded_audio::SessionStats,
    reconstructed_matches_input: bool,
    streaming: Option<&StreamingReplayReport>,
) -> &'static str {
    match mode {
        ReplayMode::Batch => replay_status(stats, reconstructed_matches_input),
        ReplayMode::Stream => streaming.map(|report| report.status).unwrap_or("FAIL"),
    }
}

fn report_reason(
    mode: ReplayMode,
    stats: &embedded_audio::SessionStats,
    reconstructed_matches_input: bool,
    streaming: Option<&StreamingReplayReport>,
) -> Option<String> {
    match mode {
        ReplayMode::Batch => replay_reason(stats, reconstructed_matches_input),
        ReplayMode::Stream => match streaming {
            Some(report) => report.reason.clone(),
            None => Some("streaming report missing".to_string()),
        },
    }
}

fn replay_status(
    stats: &embedded_audio::SessionStats,
    reconstructed_matches_input: bool,
) -> &'static str {
    if stats.end_reason == Some(SessionEndReason::Stop)
        && stats.missing_packet_count == 0
        && reconstructed_matches_input
    {
        "PASS"
    } else if stats.terminal_received {
        "WARNING"
    } else {
        "FAIL"
    }
}

fn replay_reason(
    stats: &embedded_audio::SessionStats,
    reconstructed_matches_input: bool,
) -> Option<String> {
    if stats.end_reason != Some(SessionEndReason::Stop) {
        return Some(format!("session ended with {:?}", stats.end_reason));
    }
    if stats.missing_packet_count > 0 {
        return Some(format!(
            "missing {} packet(s): {:?}",
            stats.missing_packet_count, stats.missing_packet_indices
        ));
    }
    if !reconstructed_matches_input {
        return Some("reconstructed PCM does not match input PCM".to_string());
    }
    None
}

fn streaming_status(
    terminal: TerminalMode,
    terminal_event: Option<&StreamingEventReport>,
    streamed_pcm_matches_input: bool,
    chunk_sequence_is_strictly_increasing: bool,
) -> &'static str {
    if streaming_reason(
        terminal,
        terminal_event,
        streamed_pcm_matches_input,
        chunk_sequence_is_strictly_increasing,
    )
    .is_none()
    {
        "PASS"
    } else if terminal_event.is_some() {
        "WARNING"
    } else {
        "FAIL"
    }
}

fn streaming_reason(
    terminal: TerminalMode,
    terminal_event: Option<&StreamingEventReport>,
    streamed_pcm_matches_input: bool,
    chunk_sequence_is_strictly_increasing: bool,
) -> Option<String> {
    let Some(terminal_event) = terminal_event else {
        return Some("terminal event missing".to_string());
    };
    if !terminal_event_matches(terminal, terminal_event) {
        return Some(format!(
            "terminal event {:?} did not match requested {:?}",
            terminal_event.kind, terminal
        ));
    }
    if !chunk_sequence_is_strictly_increasing {
        return Some("streamed packet sequences are not strictly increasing".to_string());
    }
    if !streamed_pcm_matches_input {
        return Some("streamed PCM chunks do not reconstruct input PCM".to_string());
    }
    None
}

fn terminal_event_matches(terminal: TerminalMode, event: &StreamingEventReport) -> bool {
    matches!(
        (terminal, event.kind),
        (TerminalMode::Stop, "stopped")
            | (TerminalMode::Cancel, "cancelled")
            | (TerminalMode::Error, "error")
    )
}

fn is_strictly_increasing(values: &[u16]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_replay_reports_stop_chunks_in_order() {
        let input_pcm = vec![1, 2, 3, 4, 5, 6];
        let notifications = build_session_replay_notifications(
            ReplayConfig {
                session_id: 900,
                payload_pcm_bytes: 2,
            },
            &input_pcm,
        )
        .expect("build replay");

        let report =
            run_streaming_replay(&notifications, &input_pcm, TerminalMode::Stop).expect("stream");

        assert_eq!(report.status, "PASS");
        assert_eq!(report.chunk_packet_sequences, vec![0, 1, 2]);
        assert_eq!(report.chunk_pcm_bytes, vec![2, 2, 2]);
        assert!(report.streamed_pcm_matches_input);
        assert_eq!(report.terminal_event.expect("terminal").kind, "stopped");
    }

    #[test]
    fn streaming_replay_reports_cancel_terminal_event() {
        let input_pcm = vec![1, 2, 3, 4];
        let mut notifications = build_session_replay_notifications(
            ReplayConfig {
                session_id: 901,
                payload_pcm_bytes: 2,
            },
            &input_pcm,
        )
        .expect("build replay");
        apply_terminal_mode(&mut notifications, 901, TerminalMode::Cancel).expect("cancel");

        let report =
            run_streaming_replay(&notifications, &input_pcm, TerminalMode::Cancel).expect("stream");

        assert_eq!(report.status, "PASS");
        assert_eq!(report.terminal_event.expect("terminal").kind, "cancelled");
        assert!(report.streamed_pcm_matches_input);
    }

    #[test]
    fn streaming_replay_reports_error_terminal_event() {
        let input_pcm = vec![1, 2, 3, 4];
        let mut notifications = build_session_replay_notifications(
            ReplayConfig {
                session_id: 902,
                payload_pcm_bytes: 2,
            },
            &input_pcm,
        )
        .expect("build replay");
        apply_terminal_mode(&mut notifications, 902, TerminalMode::Error).expect("error");

        let report =
            run_streaming_replay(&notifications, &input_pcm, TerminalMode::Error).expect("stream");

        assert_eq!(report.status, "PASS");
        let terminal = report.terminal_event.expect("terminal");
        assert_eq!(terminal.kind, "error");
        assert_eq!(terminal.error_code, Some(SessionErrorCode::LinkLost));
    }
}
