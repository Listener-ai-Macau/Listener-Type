mod asr;
#[path = "../../../src-tauri/src/embedded_audio.rs"]
mod embedded_audio;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use asr::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
use asr::AudioConsumer;
use embedded_audio::{read_input_pcm, EmbeddedAudioInputFormat};
use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "com.listener.type";
const CREDENTIALS_ACCOUNT: &str = "credentials.v1";
const CREDENTIALS_CHUNK_PREFIX: &str = "credentials.v1.chunk.";
const DEFAULT_CHUNK_BYTES: usize = 3_200;
const USAGE: &str = "\
Usage:
  listener-volcengine-asr-probe <status|transcribe> [options]

Options:
  --audio <path>              WAV/PCM path for transcribe.
  --format <wav|pcm16le>      Optional input format. Defaults to path inference.
  --timeout-seconds <n>       Final result timeout. Defaults to 60.
  --chunk-bytes <n>           Feed chunk size. Defaults to 3200.
  --pace-audio                Sleep between chunks to mimic live 16 kHz capture.
  --json-out <path>           Optional report JSON file path for pipeline metadata.
  --help                      Show this help.
";

#[derive(Debug)]
enum Command {
    Status,
    Transcribe,
}

#[derive(Debug)]
struct Args {
    command: Command,
    audio: Option<PathBuf>,
    format: Option<EmbeddedAudioInputFormat>,
    timeout_seconds: u64,
    chunk_bytes: usize,
    pace_audio: bool,
    json_out: Option<PathBuf>,
}

#[derive(Debug)]
struct RunOutput {
    report: ProbeReport,
    json_out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeReport {
    status: &'static str,
    command: &'static str,
    active_asr: Option<String>,
    volcengine_configured: bool,
    resource_id: Option<String>,
    audio_path: Option<String>,
    input_format: Option<EmbeddedAudioInputFormat>,
    pcm_bytes: Option<usize>,
    audio_duration_seconds: Option<f64>,
    transcript: Option<String>,
    transcript_duration_ms: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CredsChunkManifest {
    listener_type_credentials_storage: String,
    version: u32,
    #[serde(default)]
    generation: Option<String>,
    chunks: usize,
}

#[derive(Debug, Default, Deserialize)]
struct CredsRoot {
    #[serde(default)]
    active: CredsActive,
    #[serde(default)]
    providers: CredsProviders,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct CredsActive {
    asr: String,
    llm: String,
}

impl Default for CredsActive {
    fn default() -> Self {
        Self {
            asr: "volcengine".to_string(),
            llm: "ark".to_string(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct CredsProviders {
    #[serde(default)]
    asr: BTreeMap<String, CredsAsrEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CredsAsrEntry {
    app_key: Option<String>,
    api_key: Option<String>,
    access_key: Option<String>,
    resource_id: Option<String>,
}

#[tokio::main]
async fn main() {
    install_rustls_crypto_provider();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let result = run().await;
    match result {
        Ok(output) => {
            print_report(&output.report, output.json_out.as_ref());
            if output.report.status != "PASS" {
                std::process::exit(1);
            }
        }
        Err(error) => {
            let report = ProbeReport {
                status: "FAIL",
                command: "unknown",
                active_asr: None,
                volcengine_configured: false,
                resource_id: None,
                audio_path: None,
                input_format: None,
                pcm_bytes: None,
                audio_duration_seconds: None,
                transcript: None,
                transcript_duration_ms: None,
                error: Some(format!("{error:#}")),
            };
            print_report(&report, None);
            std::process::exit(2);
        }
    }
}

async fn run() -> Result<RunOutput> {
    let args = parse_args(env::args().skip(1))?;
    let root = load_credentials().context("load Listener Type credentials")?;
    let active_asr = root.active.asr.clone();
    let creds = read_volcengine_credentials(&root);
    let configured = credentials_configured(&creds);

    let report = match args.command {
        Command::Status => ProbeReport {
            status: if configured { "PASS" } else { "FAIL" },
            command: "status",
            active_asr: Some(active_asr),
            volcengine_configured: configured,
            resource_id: nonempty(creds.resource_id),
            audio_path: None,
            input_format: None,
            pcm_bytes: None,
            audio_duration_seconds: None,
            transcript: None,
            transcript_duration_ms: None,
            error: if configured {
                None
            } else {
                Some("volcengine credentials missing".to_string())
            },
        },
        Command::Transcribe => {
            if !configured {
                return Ok(RunOutput {
                    report: ProbeReport {
                        status: "FAIL",
                        command: "transcribe",
                        active_asr: Some(active_asr),
                        volcengine_configured: false,
                        resource_id: nonempty(creds.resource_id),
                        audio_path: args.audio.as_ref().map(|path| path.display().to_string()),
                        input_format: args.format,
                        pcm_bytes: None,
                        audio_duration_seconds: None,
                        transcript: None,
                        transcript_duration_ms: None,
                        error: Some("volcengine credentials missing".to_string()),
                    },
                    json_out: args.json_out,
                });
            }
            let audio = args
                .audio
                .as_deref()
                .context("--audio is required for transcribe")?;
            let input_format = args
                .format
                .unwrap_or_else(|| EmbeddedAudioInputFormat::infer_from_path(audio));
            match read_input_pcm(audio, Some(input_format))
                .with_context(|| format!("read {}", audio.display()))
            {
                Ok(pcm) => {
                    match transcribe_pcm(
                        &creds,
                        &pcm,
                        args.chunk_bytes,
                        args.timeout_seconds,
                        args.pace_audio,
                    )
                    .await
                    .with_context(|| format!("transcribe {}", audio.display()))
                    {
                        Ok(transcript) => {
                            let text_empty = transcript.text.trim().is_empty();
                            ProbeReport {
                                status: if text_empty { "FAIL" } else { "PASS" },
                                command: "transcribe",
                                active_asr: Some(active_asr),
                                volcengine_configured: true,
                                resource_id: nonempty(creds.resource_id),
                                audio_path: Some(audio.display().to_string()),
                                input_format: Some(input_format),
                                pcm_bytes: Some(pcm.len()),
                                audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                                transcript: Some(transcript.text),
                                transcript_duration_ms: Some(transcript.duration_ms),
                                error: if text_empty {
                                    Some("empty transcript".to_string())
                                } else {
                                    None
                                },
                            }
                        }
                        Err(error) => ProbeReport {
                            status: "FAIL",
                            command: "transcribe",
                            active_asr: Some(active_asr),
                            volcengine_configured: true,
                            resource_id: nonempty(creds.resource_id),
                            audio_path: Some(audio.display().to_string()),
                            input_format: Some(input_format),
                            pcm_bytes: Some(pcm.len()),
                            audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                            transcript: None,
                            transcript_duration_ms: None,
                            error: Some(format!("{error:#}")),
                        },
                    }
                }
                Err(error) => ProbeReport {
                    status: "FAIL",
                    command: "transcribe",
                    active_asr: Some(active_asr),
                    volcengine_configured: true,
                    resource_id: nonempty(creds.resource_id),
                    audio_path: Some(audio.display().to_string()),
                    input_format: Some(input_format),
                    pcm_bytes: None,
                    audio_duration_seconds: None,
                    transcript: None,
                    transcript_duration_ms: None,
                    error: Some(format!("{error:#}")),
                },
            }
        }
    };

    Ok(RunOutput {
        report,
        json_out: args.json_out,
    })
}

async fn transcribe_pcm(
    creds: &VolcengineCredentials,
    pcm: &[u8],
    chunk_bytes: usize,
    timeout_seconds: u64,
    pace_audio: bool,
) -> Result<asr::RawTranscript> {
    let chunk_bytes = chunk_bytes.max(640).min(64 * 1024);
    let asr = Arc::new(VolcengineStreamingASR::new(creds.clone(), Vec::new()));
    asr.open_session()
        .await
        .context("open Volcengine ASR session")?;
    for chunk in pcm.chunks(chunk_bytes) {
        asr.consume_pcm_chunk(chunk);
        if pace_audio {
            let duration_ms = ((chunk.len() as f64) / 32.0).ceil().max(1.0) as u64;
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
    asr.send_last_frame()
        .await
        .context("send final audio frame")?;
    asr.await_final_result_with_timeout(Duration::from_secs(timeout_seconds))
        .await
        .context("await final transcript")
}

fn parse_args<I>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        anyhow::bail!("{USAGE}");
    };
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        std::process::exit(0);
    }

    let command = match command.as_str() {
        "status" => Command::Status,
        "transcribe" => Command::Transcribe,
        other => anyhow::bail!("unknown command: {other}\n\n{USAGE}"),
    };

    let mut parsed = Args {
        command,
        audio: None,
        format: None,
        timeout_seconds: 60,
        chunk_bytes: DEFAULT_CHUNK_BYTES,
        pace_audio: false,
        json_out: None,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--audio" => parsed.audio = Some(PathBuf::from(next_value(&mut args, "--audio")?)),
            "--format" => {
                parsed.format = Some(
                    next_value(&mut args, "--format")?
                        .parse()
                        .map_err(|e| anyhow!("{e}"))?,
                )
            }
            "--timeout-seconds" => {
                parsed.timeout_seconds = next_value(&mut args, "--timeout-seconds")?.parse()?
            }
            "--chunk-bytes" => {
                parsed.chunk_bytes = next_value(&mut args, "--chunk-bytes")?.parse()?
            }
            "--pace-audio" => parsed.pace_audio = true,
            "--json-out" => {
                parsed.json_out = Some(PathBuf::from(next_value(&mut args, "--json-out")?))
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}\n\n{USAGE}"),
        }
    }

    Ok(parsed)
}

fn next_value<I>(args: &mut I, name: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .with_context(|| format!("missing value for {name}"))
}

fn load_credentials() -> Result<CredsRoot> {
    let Some(json_or_manifest) = get_keyring_password(CREDENTIALS_ACCOUNT)? else {
        return Ok(CredsRoot::default());
    };
    let manifest: CredsChunkManifest =
        serde_json::from_str(&json_or_manifest).context("decode system credential manifest")?;
    if manifest.listener_type_credentials_storage != "chunked" || manifest.version != 1 {
        anyhow::bail!("unsupported system credential manifest");
    }

    let mut json = String::new();
    for index in 0..manifest.chunks {
        let account = chunk_account(manifest.generation.as_deref(), index);
        let chunk = get_keyring_password(&account)?
            .ok_or_else(|| anyhow!("missing system credential chunk {index}"))?;
        json.push_str(&chunk);
    }
    serde_json::from_str(&json).context("decode system credential payload")
}

fn get_keyring_password(account: &str) -> Result<Option<String>> {
    let entry =
        keyring::Entry::new(SERVICE_NAME, account).context("open system credential vault")?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => {
            Err(anyhow!(error)).with_context(|| format!("read credential account {account}"))
        }
    }
}

fn chunk_account(generation: Option<&str>, index: usize) -> String {
    match generation {
        Some(gen) => format!("{CREDENTIALS_CHUNK_PREFIX}{gen}.{index}"),
        None => format!("{CREDENTIALS_CHUNK_PREFIX}{index}"),
    }
}

fn read_volcengine_credentials(root: &CredsRoot) -> VolcengineCredentials {
    let entry = root.providers.asr.get(&root.active.asr);
    let app_id = entry
        .and_then(|entry| pick(&entry.app_key).or_else(|| pick(&entry.api_key)))
        .unwrap_or_default();
    let access_token = entry
        .and_then(|entry| pick(&entry.access_key))
        .unwrap_or_default();
    let resource_id = entry
        .and_then(|entry| pick(&entry.resource_id))
        .unwrap_or_else(|| VolcengineCredentials::default_resource_id().to_string());
    VolcengineCredentials {
        app_id,
        access_token,
        resource_id,
    }
}

fn credentials_configured(creds: &VolcengineCredentials) -> bool {
    !creds.app_id.trim().is_empty()
        && !creds.access_token.trim().is_empty()
        && !creds.resource_id.trim().is_empty()
}

fn pick(value: &Option<String>) -> Option<String> {
    value.as_ref().filter(|v| !v.trim().is_empty()).cloned()
}

fn nonempty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn print_report(report: &ProbeReport, json_out: Option<&PathBuf>) {
    let pretty = serde_json::to_string_pretty(report).expect("serialize pretty report");
    println!("{pretty}");
    let compact = serde_json::to_string(report).expect("serialize compact report");
    if let Some(path) = json_out {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(path, &compact).expect("write report JSON");
        println!("volcengine_probe_result_json={}", path.display());
    } else {
        println!("volcengine_probe_result_json={compact}");
    }
}
