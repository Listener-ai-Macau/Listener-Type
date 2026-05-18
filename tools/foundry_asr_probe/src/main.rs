mod persistence {
    use std::fs;
    use std::path::PathBuf;

    use anyhow::{Context, Result};

    fn data_dir() -> Result<PathBuf> {
        let appdata = std::env::var("APPDATA").context("APPDATA not set")?;
        Ok(PathBuf::from(appdata).join("Listener Type"))
    }

    fn ensure_dir(dir: PathBuf) -> Result<PathBuf> {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(dir)
    }

    pub fn foundry_local_root() -> Result<PathBuf> {
        ensure_dir(data_dir()?.join("models").join("foundry-local"))
    }

    pub fn foundry_native_runtime_root() -> Result<PathBuf> {
        ensure_dir(foundry_local_root()?.join("runtime"))
    }

    pub fn foundry_model_cache_root() -> Result<PathBuf> {
        foundry_local_root()
    }

    pub fn foundry_app_data_root() -> Result<PathBuf> {
        ensure_dir(foundry_local_root()?.join("app-data"))
    }

    pub fn foundry_logs_root() -> Result<PathBuf> {
        ensure_dir(foundry_local_root()?.join("logs"))
    }
}

#[path = "../../../src-tauri/src/asr/local/foundry.rs"]
pub mod foundry_impl;
#[path = "../../../src-tauri/src/asr/local/foundry_native.rs"]
pub mod foundry_native_impl;
#[path = "../../../src-tauri/src/asr/local/foundry_runtime.rs"]
pub mod foundry_runtime_impl;

mod asr {
    pub mod local {
        pub use crate::foundry_impl as foundry;
        pub use crate::foundry_native_impl as foundry_native;
        pub use crate::foundry_runtime_impl as foundry_runtime;
    }
}

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use asr::local::foundry::DEFAULT_MODEL_ALIAS;
use asr::local::foundry_runtime::FoundryLocalRuntime;
use serde::Serialize;

const USAGE: &str = "\
Usage:
  listener-foundry-asr-probe <status|prepare|transcribe> [options]

Options:
  --model <alias>             Foundry model alias. Defaults to whisper-small.
  --runtime-source <source>   auto | nuget | ort-nightly. Defaults to auto.
  --audio <wav>               WAV path for transcribe.
  --language <code>           Optional ISO 639-1 hint, e.g. zh.
  --timeout-seconds <n>       Transcribe timeout. Defaults to 120.
  --help                      Show this help.
";

#[derive(Debug)]
enum Command {
    Status,
    Prepare,
    Transcribe,
}

#[derive(Debug)]
struct Args {
    command: Command,
    model: String,
    runtime_source: String,
    audio: Option<PathBuf>,
    language: Option<String>,
    timeout_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeReport {
    status: &'static str,
    command: &'static str,
    model: String,
    runtime_source: String,
    runtime_ready: Option<bool>,
    model_id: Option<String>,
    transcript: Option<String>,
    error: Option<String>,
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let result = run().await;
    match result {
        Ok(report) => {
            print_report(&report);
            if report.status != "PASS" {
                std::process::exit(1);
            }
        }
        Err(error) => {
            let report = ProbeReport {
                status: "FAIL",
                command: "unknown",
                model: DEFAULT_MODEL_ALIAS.to_string(),
                runtime_source: "auto".to_string(),
                runtime_ready: None,
                model_id: None,
                transcript: None,
                error: Some(format!("{error:#}")),
            };
            print_report(&report);
            std::process::exit(2);
        }
    }
}

async fn run() -> Result<ProbeReport> {
    let args = parse_args(env::args().skip(1))?;
    let runtime = FoundryLocalRuntime::new();
    match args.command {
        Command::Status => {
            let status = runtime
                .status_snapshot(&args.model, &args.runtime_source)
                .await;
            Ok(ProbeReport {
                status: "PASS",
                command: "status",
                model: args.model,
                runtime_source: status.runtime_source,
                runtime_ready: Some(status.runtime_ready),
                model_id: status.loaded_model_id,
                transcript: None,
                error: status.error,
            })
        }
        Command::Prepare => {
            let model_id = runtime
                .ensure_loaded_with_progress(&args.model, &args.runtime_source, |payload| {
                    let line = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "{\"type\":\"progress_encode_error\"}".to_string());
                    println!("foundry_progress_json={line}");
                })
                .await
                .with_context(|| format!("prepare Foundry model {}", args.model))?;
            Ok(ProbeReport {
                status: "PASS",
                command: "prepare",
                model: args.model,
                runtime_source: args.runtime_source,
                runtime_ready: Some(true),
                model_id: Some(model_id),
                transcript: None,
                error: None,
            })
        }
        Command::Transcribe => {
            let audio = args
                .audio
                .as_deref()
                .context("--audio is required for transcribe")?;
            let text = runtime
                .transcribe_audio_file(
                    &args.model,
                    &args.runtime_source,
                    args.language.as_deref(),
                    audio,
                    Duration::from_secs(args.timeout_seconds),
                )
                .await
                .with_context(|| format!("transcribe {}", audio.display()))?;
            Ok(ProbeReport {
                status: if text.trim().is_empty() {
                    "FAIL"
                } else {
                    "PASS"
                },
                command: "transcribe",
                model: args.model,
                runtime_source: args.runtime_source,
                runtime_ready: Some(true),
                model_id: None,
                transcript: Some(text),
                error: None,
            })
        }
    }
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
        "prepare" => Command::Prepare,
        "transcribe" => Command::Transcribe,
        other => anyhow::bail!("unknown command: {other}\n\n{USAGE}"),
    };

    let mut parsed = Args {
        command,
        model: DEFAULT_MODEL_ALIAS.to_string(),
        runtime_source: "auto".to_string(),
        audio: None,
        language: None,
        timeout_seconds: 120,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => parsed.model = next_value(&mut args, "--model")?,
            "--runtime-source" => {
                parsed.runtime_source = next_value(&mut args, "--runtime-source")?
            }
            "--audio" => parsed.audio = Some(PathBuf::from(next_value(&mut args, "--audio")?)),
            "--language" => parsed.language = Some(next_value(&mut args, "--language")?),
            "--timeout-seconds" => {
                parsed.timeout_seconds = next_value(&mut args, "--timeout-seconds")?.parse()?
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

fn print_report(report: &ProbeReport) {
    let pretty = serde_json::to_string_pretty(report).expect("serialize pretty report");
    println!("{pretty}");
    let compact = serde_json::to_string(report).expect("serialize compact report");
    println!("foundry_probe_result_json={compact}");
}
