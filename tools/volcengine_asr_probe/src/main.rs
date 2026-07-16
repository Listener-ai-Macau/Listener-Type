mod asr;
use denzic_audio_v1_core as embedded_audio;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use asr::volcengine::{
    VolcengineCredentials, VolcengineResultType, VolcengineSessionEndpoint,
    VolcengineSessionOptions, VolcengineStreamingASR,
};
use asr::AudioConsumer;
use embedded_audio::{read_input_pcm, EmbeddedAudioInputFormat};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "com.listener.type";
const CREDENTIALS_ACCOUNT: &str = "credentials.v1";
const CREDENTIALS_CHUNK_PREFIX: &str = "credentials.v1.chunk.";
const DEFAULT_CHUNK_BYTES: usize = 3_200;
const USAGE: &str = "\
Usage:
  listener-volcengine-asr-probe <status|transcribe> [options]
  listener-volcengine-asr-probe [transcribe options] <audio-path>

Options:
  --audio <path>              WAV/PCM path for transcribe.
  --format <wav|pcm16le>      Optional input format. Defaults to path inference.
  --timeout-seconds <n>       Final result timeout. Defaults to 60.
  --chunk-bytes <n>           Feed chunk size. Defaults to 3200.
  --preview-only              Report first authoritative streaming preview timing.
  --resource-id <id>          Validation-only resource ID override; does not save credentials.
  --endpoint <bigmodel_async|bigmodel>
                              Validation-only endpoint override. Defaults to bigmodel_async.
  --disable-nonstream         Disable the optimized endpoint's second recognition pass.
  --result-type <full|single> Validation-only result delivery format. Defaults to full.
  --end-window-size <ms>      Validation-only VAD end window override.
  --force-to-speech-time <ms> Validation-only forced-speech interval override.
  --preroll-ms <n>            Add leading 16 kHz silence before feeding audio. Defaults to 0.
  --post-feed-wait-ms <n>     Wait after feeding preview audio before cancel. Defaults to 3000.
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
    preview_only: bool,
    preroll_ms: u64,
    post_feed_wait_ms: u64,
    pace_audio: bool,
    resource_id_override: Option<String>,
    endpoint: String,
    disable_nonstream: bool,
    result_type: String,
    end_window_size_override: Option<u32>,
    force_to_speech_time_override: Option<u32>,
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
    session: Option<ProbeSession>,
    audio_path: Option<String>,
    input_format: Option<EmbeddedAudioInputFormat>,
    pcm_bytes: Option<usize>,
    audio_duration_seconds: Option<f64>,
    transcript: Option<String>,
    transcript_duration_ms: Option<u64>,
    preview_first_partial: Option<String>,
    preview_first_partial_elapsed_ms: Option<u64>,
    preview_partial_updates: Option<usize>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeSession {
    endpoint: String,
    enable_nonstream: bool,
    result_type: String,
    end_window_size_ms: Option<u32>,
    force_to_speech_time_ms: Option<u32>,
}

impl From<VolcengineSessionOptions> for ProbeSession {
    fn from(options: VolcengineSessionOptions) -> Self {
        Self {
            endpoint: options.endpoint.label().to_string(),
            enable_nonstream: options.enable_nonstream,
            result_type: options.result_type.as_str().to_string(),
            end_window_size_ms: options.end_window_size_ms,
            force_to_speech_time_ms: options.force_to_speech_time_ms,
        }
    }
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
                session: None,
                audio_path: None,
                input_format: None,
                pcm_bytes: None,
                audio_duration_seconds: None,
                transcript: None,
                transcript_duration_ms: None,
                preview_first_partial: None,
                preview_first_partial_elapsed_ms: None,
                preview_partial_updates: None,
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
    let mut creds = read_volcengine_credentials(&root);
    if let Some(resource_id) = args.resource_id_override.as_deref() {
        creds.resource_id = resource_id.to_string();
    }
    let session_options = probe_session_options(&args)?;
    let configured = credentials_configured(&creds);

    let report = match args.command {
        Command::Status => ProbeReport {
            status: if configured { "PASS" } else { "FAIL" },
            command: "status",
            active_asr: Some(active_asr),
            volcengine_configured: configured,
            resource_id: nonempty(creds.resource_id),
            session: Some(session_options.into()),
            audio_path: None,
            input_format: None,
            pcm_bytes: None,
            audio_duration_seconds: None,
            transcript: None,
            transcript_duration_ms: None,
            preview_first_partial: None,
            preview_first_partial_elapsed_ms: None,
            preview_partial_updates: None,
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
                        session: Some(session_options.into()),
                        audio_path: args.audio.as_ref().map(|path| path.display().to_string()),
                        input_format: args.format,
                        pcm_bytes: None,
                        audio_duration_seconds: None,
                        transcript: None,
                        transcript_duration_ms: None,
                        preview_first_partial: None,
                        preview_first_partial_elapsed_ms: None,
                        preview_partial_updates: None,
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
                    if args.preview_only {
                        match preview_pcm(
                            &creds,
                            &pcm,
                            args.chunk_bytes,
                            args.preroll_ms,
                            args.post_feed_wait_ms,
                            args.pace_audio,
                            session_options,
                        )
                        .await
                        .with_context(|| format!("preview {}", audio.display()))
                        {
                            Ok(preview) => {
                                let text_empty = preview
                                    .first_partial
                                    .as_deref()
                                    .unwrap_or_default()
                                    .trim()
                                    .is_empty();
                                ProbeReport {
                                    status: if text_empty { "FAIL" } else { "PASS" },
                                    command: "transcribe",
                                    active_asr: Some(active_asr),
                                    volcengine_configured: true,
                                    resource_id: nonempty(creds.resource_id),
                                    session: Some(session_options.into()),
                                    audio_path: Some(audio.display().to_string()),
                                    input_format: Some(input_format),
                                    pcm_bytes: Some(pcm.len()),
                                    audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                                    transcript: None,
                                    transcript_duration_ms: None,
                                    preview_first_partial: preview.first_partial,
                                    preview_first_partial_elapsed_ms: preview
                                        .first_partial_elapsed_ms,
                                    preview_partial_updates: Some(preview.partial_updates),
                                    error: if text_empty {
                                        Some("no preview partial".to_string())
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
                                session: Some(session_options.into()),
                                audio_path: Some(audio.display().to_string()),
                                input_format: Some(input_format),
                                pcm_bytes: Some(pcm.len()),
                                audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                                transcript: None,
                                transcript_duration_ms: None,
                                preview_first_partial: None,
                                preview_first_partial_elapsed_ms: None,
                                preview_partial_updates: None,
                                error: Some(format!("{error:#}")),
                            },
                        }
                    } else {
                        match transcribe_pcm(
                            &creds,
                            &pcm,
                            args.chunk_bytes,
                            args.preroll_ms,
                            args.timeout_seconds,
                            args.pace_audio,
                            session_options,
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
                                    session: Some(session_options.into()),
                                    audio_path: Some(audio.display().to_string()),
                                    input_format: Some(input_format),
                                    pcm_bytes: Some(pcm.len()),
                                    audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                                    transcript: Some(transcript.text),
                                    transcript_duration_ms: Some(transcript.duration_ms),
                                    preview_first_partial: None,
                                    preview_first_partial_elapsed_ms: None,
                                    preview_partial_updates: None,
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
                                session: Some(session_options.into()),
                                audio_path: Some(audio.display().to_string()),
                                input_format: Some(input_format),
                                pcm_bytes: Some(pcm.len()),
                                audio_duration_seconds: Some(pcm.len() as f64 / 32_000.0),
                                transcript: None,
                                transcript_duration_ms: None,
                                preview_first_partial: None,
                                preview_first_partial_elapsed_ms: None,
                                preview_partial_updates: None,
                                error: Some(format!("{error:#}")),
                            },
                        }
                    }
                }
                Err(error) => ProbeReport {
                    status: "FAIL",
                    command: "transcribe",
                    active_asr: Some(active_asr),
                    volcengine_configured: true,
                    resource_id: nonempty(creds.resource_id),
                    session: Some(session_options.into()),
                    audio_path: Some(audio.display().to_string()),
                    input_format: Some(input_format),
                    pcm_bytes: None,
                    audio_duration_seconds: None,
                    transcript: None,
                    transcript_duration_ms: None,
                    preview_first_partial: None,
                    preview_first_partial_elapsed_ms: None,
                    preview_partial_updates: None,
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
    preroll_ms: u64,
    timeout_seconds: u64,
    pace_audio: bool,
    session_options: VolcengineSessionOptions,
) -> Result<asr::RawTranscript> {
    let chunk_bytes = chunk_bytes.max(640).min(64 * 1024);
    let asr = Arc::new(VolcengineStreamingASR::new_with_session_options(
        creds.clone(),
        Vec::new(),
        session_options,
    ));
    asr.open_session()
        .await
        .context("open Volcengine ASR session")?;
    asr.mark_audio_delivery_ready();
    feed_preroll(&asr, chunk_bytes, preroll_ms, pace_audio).await;
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

struct PreviewProbeOutput {
    first_partial: Option<String>,
    first_partial_elapsed_ms: Option<u64>,
    partial_updates: usize,
}

async fn preview_pcm(
    creds: &VolcengineCredentials,
    pcm: &[u8],
    chunk_bytes: usize,
    preroll_ms: u64,
    post_feed_wait_ms: u64,
    pace_audio: bool,
    session_options: VolcengineSessionOptions,
) -> Result<PreviewProbeOutput> {
    let chunk_bytes = chunk_bytes.max(640).min(64 * 1024);
    let asr = Arc::new(VolcengineStreamingASR::new_with_session_options(
        creds.clone(),
        Vec::new(),
        session_options,
    ));
    let started = Instant::now();
    let partials = Arc::new(ParkingMutex::new(Vec::<(u64, String)>::new()));
    let partials_for_intermediate = Arc::clone(&partials);
    asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |update| {
        let text = update.text;
        if text.trim().is_empty() {
            return;
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        partials_for_intermediate.lock().push((elapsed_ms, text));
    })));
    let partials_for_generic = Arc::clone(&partials);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        if text.trim().is_empty() {
            return;
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        partials_for_generic.lock().push((elapsed_ms, text));
    })));
    asr.open_session()
        .await
        .context("open Volcengine preview ASR session")?;
    asr.mark_audio_delivery_ready();
    feed_preroll(&asr, chunk_bytes, preroll_ms, pace_audio).await;
    for chunk in pcm.chunks(chunk_bytes) {
        asr.consume_pcm_chunk(chunk);
        if pace_audio {
            let duration_ms = ((chunk.len() as f64) / 32.0).ceil().max(1.0) as u64;
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
    if post_feed_wait_ms > 0 {
        tokio::time::sleep(Duration::from_millis(post_feed_wait_ms)).await;
    }
    asr.cancel();
    let partials = partials.lock();
    let first = partials.first().cloned();
    Ok(PreviewProbeOutput {
        first_partial: first.as_ref().map(|(_, text)| text.clone()),
        first_partial_elapsed_ms: first.map(|(elapsed_ms, _)| elapsed_ms),
        partial_updates: partials.len(),
    })
}

async fn feed_preroll(
    asr: &Arc<VolcengineStreamingASR>,
    chunk_bytes: usize,
    preroll_ms: u64,
    pace_audio: bool,
) {
    let preroll_bytes =
        ((preroll_ms as usize) * 32).saturating_sub(((preroll_ms as usize) * 32) % 2);
    if preroll_bytes == 0 {
        return;
    }
    let silence = vec![0u8; preroll_bytes];
    for chunk in silence.chunks(chunk_bytes) {
        asr.consume_pcm_chunk(chunk);
        if pace_audio {
            let duration_ms = ((chunk.len() as f64) / 32.0).ceil().max(1.0) as u64;
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}

fn parse_args<I>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = String>,
{
    let mut raw_args: Vec<String> = args.into_iter().collect();
    let Some(first_arg) = raw_args.first() else {
        anyhow::bail!("{USAGE}");
    };
    if first_arg == "--help" || first_arg == "-h" {
        println!("{USAGE}");
        std::process::exit(0);
    }

    let command = match first_arg.as_str() {
        "status" => {
            raw_args.remove(0);
            Command::Status
        }
        "transcribe" => {
            raw_args.remove(0);
            Command::Transcribe
        }
        other if other.starts_with("--") || !other.trim().is_empty() => Command::Transcribe,
        other => anyhow::bail!("unknown command: {other}\n\n{USAGE}"),
    };

    let mut parsed = Args {
        command,
        audio: None,
        format: None,
        timeout_seconds: 60,
        chunk_bytes: DEFAULT_CHUNK_BYTES,
        preview_only: false,
        preroll_ms: 0,
        post_feed_wait_ms: 3_000,
        pace_audio: false,
        resource_id_override: None,
        endpoint: "bigmodel_async".to_string(),
        disable_nonstream: false,
        result_type: "full".to_string(),
        end_window_size_override: None,
        force_to_speech_time_override: None,
        json_out: None,
    };

    let mut args = raw_args.into_iter();
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
            "--preview-only" => parsed.preview_only = true,
            "--preroll-ms" => parsed.preroll_ms = next_value(&mut args, "--preroll-ms")?.parse()?,
            "--post-feed-wait-ms" => {
                parsed.post_feed_wait_ms = next_value(&mut args, "--post-feed-wait-ms")?.parse()?
            }
            "--pace-audio" => parsed.pace_audio = true,
            "--resource-id" => {
                parsed.resource_id_override = Some(next_value(&mut args, "--resource-id")?)
            }
            "--endpoint" => parsed.endpoint = next_value(&mut args, "--endpoint")?,
            "--disable-nonstream" => parsed.disable_nonstream = true,
            "--result-type" => parsed.result_type = next_value(&mut args, "--result-type")?,
            "--end-window-size" => {
                parsed.end_window_size_override =
                    Some(next_value(&mut args, "--end-window-size")?.parse()?)
            }
            "--force-to-speech-time" => {
                parsed.force_to_speech_time_override =
                    Some(next_value(&mut args, "--force-to-speech-time")?.parse()?)
            }
            "--json-out" => {
                parsed.json_out = Some(PathBuf::from(next_value(&mut args, "--json-out")?))
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other
                if !other.starts_with("--")
                    && matches!(parsed.command, Command::Transcribe)
                    && parsed.audio.is_none() =>
            {
                parsed.audio = Some(PathBuf::from(other));
            }
            other => anyhow::bail!("unknown argument: {other}\n\n{USAGE}"),
        }
    }

    Ok(parsed)
}

fn probe_session_options(args: &Args) -> Result<VolcengineSessionOptions> {
    let mut options = match args.endpoint.as_str() {
        "bigmodel_async" => VolcengineSessionOptions {
            enable_nonstream: !args.disable_nonstream,
            ..VolcengineSessionOptions::default()
        },
        "bigmodel" if args.disable_nonstream => VolcengineSessionOptions {
            endpoint: VolcengineSessionEndpoint::Bidirectional,
            enable_nonstream: false,
            result_type: VolcengineResultType::Full,
            end_window_size_ms: None,
            force_to_speech_time_ms: None,
        },
        "bigmodel" => anyhow::bail!(
            "--endpoint bigmodel requires --disable-nonstream because only bigmodel_async supports the second recognition pass"
        ),
        other => anyhow::bail!(
            "unsupported --endpoint {other}; expected bigmodel_async or bigmodel"
        ),
    };
    options.result_type = match args.result_type.as_str() {
        "full" => VolcengineResultType::Full,
        "single" => VolcengineResultType::Single,
        other => anyhow::bail!("unsupported --result-type {other}; expected full or single"),
    };
    if matches!(options.endpoint, VolcengineSessionEndpoint::Bidirectional)
        && (args.end_window_size_override.is_some() || args.force_to_speech_time_override.is_some())
    {
        anyhow::bail!(
            "--end-window-size and --force-to-speech-time only apply to --endpoint bigmodel_async"
        );
    }
    if let Some(end_window_size_ms) = args.end_window_size_override {
        options.end_window_size_ms = Some(end_window_size_ms);
    }
    if let Some(force_to_speech_time_ms) = args.force_to_speech_time_override {
        options.force_to_speech_time_ms = Some(force_to_speech_time_ms);
    }
    Ok(options)
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
    let entry = root
        .providers
        .asr
        .get("volcengine")
        .or_else(|| root.providers.asr.get(&root.active.asr));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_preview_options_without_explicit_transcribe_command() {
        let args = parse_args([
            "--preview-only".to_string(),
            "--preroll-ms".to_string(),
            "800".to_string(),
            "--resource-id".to_string(),
            "volc.seedasr.sauc.duration".to_string(),
            "sample.wav".to_string(),
        ])
        .unwrap();

        assert!(matches!(args.command, Command::Transcribe));
        assert!(args.preview_only);
        assert_eq!(args.preroll_ms, 800);
        assert_eq!(
            args.resource_id_override.as_deref(),
            Some("volc.seedasr.sauc.duration")
        );
        assert_eq!(args.audio, Some(PathBuf::from("sample.wav")));
    }

    #[test]
    fn parse_audio_path_before_probe_options() {
        let args = parse_args([
            "sample.wav".to_string(),
            "--preview-only".to_string(),
            "--pace-audio".to_string(),
        ])
        .unwrap();

        assert!(matches!(args.command, Command::Transcribe));
        assert!(args.preview_only);
        assert!(args.pace_audio);
        assert_eq!(args.audio, Some(PathBuf::from("sample.wav")));
    }

    #[test]
    fn single_result_type_is_limited_to_validation_probe_options() {
        let args = parse_args([
            "--result-type".to_string(),
            "single".to_string(),
            "sample.wav".to_string(),
        ])
        .unwrap();
        let options = probe_session_options(&args).unwrap();

        assert_eq!(options.result_type, VolcengineResultType::Single);
    }

    #[test]
    fn bidirectional_probe_mode_requires_nonstream_to_be_disabled() {
        let args = parse_args([
            "--endpoint".to_string(),
            "bigmodel".to_string(),
            "--disable-nonstream".to_string(),
            "sample.wav".to_string(),
        ])
        .unwrap();
        let options = probe_session_options(&args).unwrap();

        assert_eq!(options.endpoint, VolcengineSessionEndpoint::Bidirectional);
        assert!(!options.enable_nonstream);
        assert!(options.end_window_size_ms.is_none());
        assert!(options.force_to_speech_time_ms.is_none());
    }
}
