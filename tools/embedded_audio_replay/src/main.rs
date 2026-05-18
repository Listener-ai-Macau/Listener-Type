#![allow(dead_code)]

#[path = "../../../src-tauri/src/embedded_audio.rs"]
mod embedded_audio;

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use embedded_audio::{
    build_session_replay_notifications, collect_notifications, parse_packet, read_input_pcm,
    EmbeddedAudioInputFormat, PacketType, ReplayConfig, SessionEndReason,
    DEFAULT_REPLAY_PAYLOAD_PCM_BYTES, PCM_CHANNELS, PCM_SAMPLE_RATE_HZ, PCM_SAMPLE_WIDTH_BITS,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

const USAGE: &str = "\
Usage:
  listener-embedded-audio-replay --input <file> [options]

Options:
  --format <wav|pcm16le>      Input format. Defaults to wav for .wav, otherwise pcm16le.
  --session-id <u32>          VKA1 session id. Defaults to 1.
  --payload-bytes <usize>     Audio payload bytes per VKA1 audio_data packet. Defaults to 480.
  --drop-packet <u16>         Drop one audio_data packet sequence. Can be repeated.
  --json-out <file>           Write pretty JSON report to this file.
  --help                      Show this help.
";

#[derive(Debug)]
struct Args {
    input: PathBuf,
    format: Option<EmbeddedAudioInputFormat>,
    session_id: u32,
    payload_bytes: usize,
    drop_packets: BTreeSet<u16>,
    json_out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayReport {
    status: &'static str,
    reason: Option<String>,
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioFormatReport {
    encoding: &'static str,
    sample_rate_hz: u32,
    channels: u16,
    sample_width_bits: u16,
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
    let report = ReplayReport {
        status: replay_status(&stats, reconstructed_matches_input),
        reason: replay_reason(&stats, reconstructed_matches_input),
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
    let mut session_id = 1u32;
    let mut payload_bytes = DEFAULT_REPLAY_PAYLOAD_PCM_BYTES;
    let mut drop_packets = BTreeSet::new();
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
            "--json-out" => {
                json_out = Some(PathBuf::from(next_value(&mut args, "--json-out")?));
            }
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }

    Ok(Args {
        input: input.ok_or_else(|| format!("missing --input\n\n{USAGE}"))?,
        format,
        session_id,
        payload_bytes,
        drop_packets,
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
