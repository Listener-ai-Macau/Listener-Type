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
}

#[path = "../../../src-tauri/src/asr/local/foundry_native.rs"]
pub mod foundry_native;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

const USAGE: &str = "\
Usage:
  listener-foundry-runtime-prepare <status|prepare> [options]

Options:
  --runtime-source <source>   auto | nuget | ort-nightly. Defaults to auto.
  --help                      Show this help.
";

#[derive(Debug)]
enum Command {
    Status,
    Prepare,
}

#[derive(Debug)]
struct Args {
    command: Command,
    runtime_source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeReport {
    status: &'static str,
    command: &'static str,
    runtime_source: String,
    runtime_ready: bool,
    runtime_dir: Option<PathBuf>,
    dlls: Vec<String>,
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
            let report = RuntimeReport {
                status: "FAIL",
                command: "unknown",
                runtime_source: "auto".into(),
                runtime_ready: false,
                runtime_dir: None,
                dlls: Vec::new(),
                error: Some(format!("{error:#}")),
            };
            print_report(&report);
            std::process::exit(2);
        }
    }
}

async fn run() -> Result<RuntimeReport> {
    let args = parse_args(std::env::args().skip(1))?;
    let runtime_source = foundry_native::normalize_runtime_source_str(&args.runtime_source);
    match args.command {
        Command::Status => Ok(report("status", runtime_source, None)),
        Command::Prepare => {
            let source = foundry_native::normalize_runtime_source(&args.runtime_source);
            let runtime_dir = foundry_native::ensure_runtime(source, |label, percent| {
                let payload = serde_json::json!({
                    "type": "runtime",
                    "label": label,
                    "percent": percent,
                });
                println!("foundry_runtime_progress_json={payload}");
            })
            .await?;
            Ok(report("prepare", runtime_source, Some(runtime_dir)))
        }
    }
}

fn report(
    command: &'static str,
    runtime_source: String,
    runtime_dir: Option<PathBuf>,
) -> RuntimeReport {
    let runtime_dir = runtime_dir.or_else(|| foundry_native::runtime_dir().ok());
    let dlls = runtime_dir.as_deref().map(runtime_dlls).unwrap_or_default();
    let runtime_ready = foundry_native::runtime_ready();
    RuntimeReport {
        status: if runtime_ready { "PASS" } else { "FAIL" },
        command,
        runtime_source,
        runtime_ready,
        runtime_dir,
        dlls,
        error: None,
    }
}

fn runtime_dlls(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            let is_dll = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
            is_dll.then(|| entry.file_name().to_string_lossy().to_string())
        })
        .collect()
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
        other => anyhow::bail!("unknown command: {other}\n\n{USAGE}"),
    };
    let mut parsed = Args {
        command,
        runtime_source: "auto".into(),
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runtime-source" => {
                parsed.runtime_source = next_value(&mut args, "--runtime-source")?
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
        .ok_or_else(|| anyhow::anyhow!("missing value for {name}"))
}

fn print_report(report: &RuntimeReport) {
    let pretty = serde_json::to_string_pretty(report).expect("serialize pretty report");
    println!("{pretty}");
    let compact = serde_json::to_string(report).expect("serialize compact report");
    println!("foundry_runtime_result_json={compact}");
}
