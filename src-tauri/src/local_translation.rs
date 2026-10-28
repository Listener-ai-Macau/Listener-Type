//! Optional offline translation. Explicit preparation owns network access;
//! inference never downloads, reads provider credentials or falls back to cloud.
use serde::{Deserialize, Serialize};
use crate::types::LocalTranslationQuality;
mod enhanced;
mod supervised;
pub(crate) use supervised::cancel as cancel_inference;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}},
    time::{Duration, Instant},
};

const RUNTIME: &str = "python31317-ct2481-spm021-v1";
const WORKER: &str = include_str!("../resources/translation/worker.py");
const MINIMAL_CT2_INIT: &str = "# Listener inference-only adapter for the MIT CTranslate2 wheel.\n# Modification: omit converters, Torch, Transformers and model specs.\nimport os\nfrom pathlib import Path\n_dll_directory = os.add_dll_directory(str(Path(__file__).resolve().parent))\nfrom ._ext import Translator\n";
static PREPARING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static RECEIVED: AtomicU64 = AtomicU64::new(0);
static EXPECTED: AtomicU64 = AtomicU64::new(0);
static PREPARATION_CONTROL: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
#[cfg(test)]
static TEST_SERIAL: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Asset {
    pub id: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pair {
    pub source: String,
    pub target: String,
    pub version: String,
    pub asset: Asset,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog { runtime: Vec<Asset>, pairs: Vec<Pair> }

fn catalog() -> Catalog {
    serde_json::from_str(include_str!("../resources/translation/catalog.json"))
        .expect("compiled offline translation catalog")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub supported: bool,
    pub runtime_ready: bool,
    pub runtime_download_bytes: u64,
    pub pairs: Vec<PairStatus>,
    pub preparing: bool,
    pub received_bytes: u64,
    pub expected_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairStatus {
    pub source: String,
    pub target: String,
    pub download_bytes: u64,
    pub ready: bool,
}

#[derive(Deserialize, Serialize)]
struct VerifiedFile { path: String, bytes: u64, sha256: String }

fn root() -> Result<PathBuf, String> {
    crate::persistence::offline_translation_root().map_err(|e| e.to_string())
}

fn relative(name: &str) -> Result<&Path, String> {
    let path = Path::new(name);
    if name.is_empty() || name.contains('\\') || name.contains(':')
        || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("local_translation_invalid_asset_path".into());
    }
    Ok(path)
}

fn manifest_ready(dir: &Path, hash: bool) -> bool {
    let Ok(bytes) = fs::read(dir.join("verified.json")) else { return false };
    let Ok(files) = serde_json::from_slice::<Vec<VerifiedFile>>(&bytes) else { return false };
    !files.is_empty() && files.iter().all(|item| {
        let Ok(path) = relative(&item.path) else { return false };
        let path = dir.join(path);
        if !path.metadata().is_ok_and(|m| m.is_file() && m.len() == item.bytes) { return false; }
        !hash || file_hash(&path).is_ok_and(|digest| digest == item.sha256)
    })
}

fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn pair_for(source: &str, target: &str) -> Result<Pair, String> {
    catalog().pairs.into_iter().find(|p| p.source == source && p.target == target)
        .ok_or_else(|| "local_translation_unsupported_pair".into())
}

pub(crate) fn status() -> Result<Status, String> {
    let root = root()?;
    let runtime_ready = manifest_ready(&root.join(RUNTIME), false);
    Ok(Status {
        supported: cfg!(all(target_os = "windows", target_arch = "x86_64")),
        runtime_ready,
        runtime_download_bytes: catalog().runtime.iter().map(|asset| asset.bytes).sum(),
        pairs: catalog().pairs.into_iter().map(|p| PairStatus {
            source: p.source, target: p.target, download_bytes: p.asset.bytes,
            ready: runtime_ready && manifest_ready(&root.join(p.asset.id), false),
        }).collect(),
        preparing: PREPARING.load(Ordering::Acquire),
        received_bytes: RECEIVED.load(Ordering::Relaxed),
        expected_bytes: EXPECTED.load(Ordering::Relaxed),
    })
}

pub(crate) fn status_for(quality: LocalTranslationQuality) -> Result<Status, String> {
    match quality { LocalTranslationQuality::Standard => status(), LocalTranslationQuality::Enhanced => enhanced::status() }
}

fn cancelled() -> Result<(), String> {
    if CANCEL.load(Ordering::Acquire) { Err("local_translation_cancelled".into()) } else { Ok(()) }
}

pub(crate) fn cancel_preparation() {
    let _control = PREPARATION_CONTROL.lock();
    if PREPARING.load(Ordering::Acquire) { CANCEL.store(true, Ordering::Release); }
}

struct Stage { path: PathBuf, root: PathBuf }
impl Stage {
    fn new(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let path = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self { path, root })
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        // Delete only this UUID staging directory, after resolving containment.
        if self.path.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".staging-"))
            && self.path.canonicalize().is_ok_and(|p| p.parent() == Some(self.root.as_path())) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn download(asset: &Asset, path: &Path) -> Result<(), String> {
    cancelled()?;
    // This function runs on the preparation blocking worker. Its private async
    // network loop can cancel a stalled response without waiting for the full
    // multi-minute model transfer timeout; it never occupies the UI runtime.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20)).read_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(1200))
        .user_agent("Listener-Type-offline-translation/1");
    #[cfg(test)]
    let builder = builder.no_proxy(); // local fixture HTTP never uses machine proxy
    log::info!("[offline-translation] download asset={} expected_bytes={}", asset.id, asset.bytes);
    let client = builder.build().map_err(|e| e.to_string())?;
    let request = client.get(&asset.url).header(reqwest::header::RANGE, format!("bytes=0-{}", asset.bytes - 1));
    let cancellation = async {
        while !CANCEL.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(50)).await; }
    };
    tokio::pin!(cancellation);
    let mut response = tokio::select! {
        biased;
        _ = &mut cancellation => return Err("local_translation_cancelled".into()),
        result = request.send() => result.map_err(|error| {
            log::warn!("[offline-translation] download transport asset={} timeout={} connect={}", asset.id, error.is_timeout(), error.is_connect());
            "local_translation_download_failed".to_owned()
        })?,
    };
    if !response.status().is_success() {
        log::warn!("[offline-translation] download rejected asset={} status={}", asset.id, response.status().as_u16());
        return Err("local_translation_download_failed".into());
    }
    let mut output = File::create(path).map_err(|e| e.to_string())?;
    let mut count = 0;
    let mut hash = Sha256::new();
    loop {
        cancelled()?;
        let chunk = tokio::select! {
            biased;
            _ = &mut cancellation => return Err("local_translation_cancelled".into()),
            chunk = response.chunk() => chunk.map_err(|_| "local_translation_download_failed".to_owned())?,
        };
        let Some(chunk) = chunk else { break };
        count += chunk.len() as u64;
        if count > asset.bytes { return Err("local_translation_checksum_failed".into()); }
        hash.update(&chunk);
        output.write_all(&chunk).map_err(|e| e.to_string())?;
        RECEIVED.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    if count != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err("local_translation_checksum_failed".into());
    }
    output.sync_all().map_err(|e| e.to_string())
    })
}

fn extract(archive: &Path, dest: &Path, model_only: bool, crt_only: bool) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        cancelled()?;
        let mut item = zip.by_index(i).map_err(|e| e.to_string())?;
        if item.is_dir() { continue; }
        let original = relative(item.name())?.to_owned();
        // The embedded interpreter already supplies VCRUNTIME. Only the C++
        // standard library DLL is required from this pinned Microsoft package;
        // no Appx installation, administrator action or older CRT overwrite.
        if crt_only && original != Path::new("msvcp140.dll") { continue; }
        // Never extract symlinks or executable model content.
        if item.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err("local_translation_invalid_asset_path".into());
        }
        let path = if model_only {
            let mut parts = original.components();
            parts.next().ok_or("local_translation_invalid_asset_path")?;
            let rest = parts.as_path();
            let filename = rest.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let lower = filename.to_lowercase();
            let license = ["license", "readme", "copyright"].iter().any(|p| lower.contains(p));
            if !(license || filename == "sentencepiece.model" || filename == "metadata.json"
                || rest.starts_with("model") && ["bin", "json", "txt"].contains(&rest.extension().and_then(|s| s.to_str()).unwrap_or(""))) { continue; }
            relative(&rest.to_string_lossy().replace('\\', "/"))?.to_owned()
        } else { original };
        total += item.size();
        if total > 300_000_000 { return Err("local_translation_asset_size_limit".into()); }
        let output = dest.join(path);
        fs::create_dir_all(output.parent().ok_or("local_translation_invalid_asset_path")?).map_err(|e| e.to_string())?;
        let mut file = File::create(output).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            cancelled()?;
            let bytes = item.read(&mut buffer).map_err(|e| e.to_string())?;
            if bytes == 0 { break; }
            file.write_all(&buffer[..bytes]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn write_manifest(dir: &Path) -> Result<(), String> {
    fn collect(base: &Path, dir: &Path, result: &mut Vec<VerifiedFile>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() { collect(base, &entry.path(), result)?; }
            else if kind.is_file() {
                result.push(VerifiedFile {
                    path: entry.path().strip_prefix(base).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/"),
                    bytes: entry.metadata().map_err(|e| e.to_string())?.len(),
                    sha256: file_hash(&entry.path())?,
                });
            } else { return Err("local_translation_invalid_asset_path".into()); }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    fs::write(dir.join("verified.json"), serde_json::to_vec(&files).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn prepare_directory(root: &Path, id: &str, assets: &[Asset], model: bool) -> Result<(), String> {
    let dest = root.join(relative(id)?);
    if manifest_ready(&dest, true) { return Ok(()); }
    let stage = Stage::new(root)?;
    let content = stage.path.join("content");
    fs::create_dir(&content).map_err(|e| e.to_string())?;
    for asset in assets {
        let archive = stage.path.join(format!("{}.zip", asset.id));
        download(asset, &archive)?;
        extract(&archive, &if model || ["python", "vclibs"].contains(&asset.id.as_str()) { content.clone() } else { content.join("libs") }, model, asset.id == "vclibs")?;
    }
    if model {
        if !content.join("model/model.bin").is_file() || !content.join("sentencepiece.model").is_file() {
            return Err("local_translation_incomplete_model".into());
        }
    } else {
        fs::write(content.join("python313._pth"), "python313.zip\n.\nlibs\n").map_err(|e| e.to_string())?;
        fs::write(content.join("libs/ctranslate2/__init__.py"), MINIMAL_CT2_INIT).map_err(|e| e.to_string())?;
    }
    write_manifest(&content)?;
    cancelled()?;
    // A corrupt cache is not replaced until complete verified content exists.
    // Retain it inside this owned staging directory until the atomic publish.
    if dest.exists() { fs::rename(&dest, stage.path.join("previous")).map_err(|e| e.to_string())?; }
    fs::rename(&content, &dest).map_err(|e| e.to_string())
}

pub(crate) struct Preparation { pair: Pair, quality: LocalTranslationQuality }
impl Drop for Preparation {
    fn drop(&mut self) {
        let _control = PREPARATION_CONTROL.lock();
        PREPARING.store(false, Ordering::Release);
    }
}

/// Reserve before queueing a blocking worker, so an immediate Cancel cannot be
/// erased by that worker resetting its flag after it eventually starts.
pub(crate) fn reserve_preparation(source: &str, target: &str) -> Result<Preparation, String> {
    reserve_preparation_for(source, target, LocalTranslationQuality::Standard)
}

pub(crate) fn reserve_preparation_for(source: &str, target: &str, quality: LocalTranslationQuality) -> Result<Preparation, String> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        return Err("local_translation_platform_unavailable".into());
    }
    let pair = pair_for(source, target)?;
    let _control = PREPARATION_CONTROL.lock();
    if PREPARING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err("local_translation_preparing".into());
    }
    CANCEL.store(false, Ordering::Release);
    RECEIVED.store(0, Ordering::Relaxed);
    EXPECTED.store(0, Ordering::Relaxed);
    Ok(Preparation { pair, quality })
}
impl Preparation {
    pub(crate) fn run(self) -> Result<Status, String> {
        cancelled()?;
        let pair = &self.pair;
        let root = root()?;
        if self.quality == LocalTranslationQuality::Enhanced {
            enhanced::prepare(&root)?;
            let mut status = enhanced::status()?;
            status.preparing = false;
            return Ok(status);
        }
        let catalog = catalog();
        let runtime_missing = !manifest_ready(&root.join(RUNTIME), true);
        let pair_missing = !manifest_ready(&root.join(&pair.asset.id), true);
        EXPECTED.store(if runtime_missing { catalog.runtime.iter().map(|a| a.bytes).sum::<u64>() } else { 0 }
            + if pair_missing { pair.asset.bytes } else { 0 }, Ordering::Relaxed);
        prepare_directory(&root, RUNTIME, &catalog.runtime, false)?;
        prepare_directory(&root, &pair.asset.id, std::slice::from_ref(&pair.asset), true)?;
        let mut status = status()?;
        status.preparing = false;
        Ok(status)
    }
}

fn worker_request_owned(runtime: &Path, model: &Path, text: &str, timeout: Duration, owner: &supervised::Request) -> Result<String, String> {
    supervised::run(runtime, WORKER, serde_json::json!({"text": text, "model": model.join("model"), "tokenizer": model.join("sentencepiece.model")}), timeout, owner)
}

#[cfg(test)]
fn worker_request(runtime: &Path, model: &Path, text: &str, timeout: Duration) -> Result<String, String> {
    let owner = supervised::Request::reserve(None)?;
    worker_request_owned(runtime, model, text, timeout, &owner)
}

fn translate_owned(source: &str, target: &str, text: &str, quality: LocalTranslationQuality, owner: &supervised::Request) -> Result<String, String> {
    owner.check()?;
    if text.trim().is_empty() || text.chars().count() > 4000 { return Err("local_translation_invalid_text".into()); }
    let pair = pair_for(source, target)?;
    let root = root()?;
    if quality == LocalTranslationQuality::Enhanced { return enhanced::translate(&root, source, target, text, owner); }
    let runtime = root.join(RUNTIME);
    let model = root.join(pair.asset.id);
    if !manifest_ready(&runtime, true) || !manifest_ready(&model, true) { return Err("local_translation_not_ready".into()); }
    worker_request_owned(&runtime, &model, text, Duration::from_secs(45), owner)
}

#[cfg(test)]
pub(crate) fn translate(source: &str, target: &str, text: &str) -> Result<String, String> {
    let owner = supervised::Request::reserve(None)?;
    translate_owned(source, target, text, LocalTranslationQuality::Standard, &owner)
}

pub(crate) fn language_code(language: &str) -> Option<&'static str> {
    match language.trim() {
        "en" | "English" => Some("en"),
        "zh" | "简体中文" | "繁体中文" => Some("zh"),
        _ => None,
    }
}

/// The same explicit offline route is shared by text previews and voice
/// translation. Language/script choice never changes the cloud provider.
pub(crate) async fn translate_to_language(text: String, source: String, target: String) -> Result<String, String> {
    translate_with_quality(text, source, target, LocalTranslationQuality::Standard, None).await
}

pub(crate) async fn translate_with_quality(text: String, source: String, target: String, quality: LocalTranslationQuality, request_id: Option<String>) -> Result<String, String> {
    let target_code = language_code(&target).ok_or("local_translation_unsupported_pair")?;
    // Reserve on the command thread, before queueing: an immediate UI Cancel
    // cannot be lost. Dropping the async command also cancels this same owner.
    let owner = supervised::Request::reserve(request_id)?;
    let _cancel_on_drop = supervised::CancelOnDrop(owner.cancel.clone());
    let output = tauri::async_runtime::spawn_blocking(move || translate_owned(&source, target_code, &text, quality, &owner))
        .await.map_err(|_| "local_translation_runtime_failed".to_owned())??;
    Ok(if target.trim() == "繁体中文" {
        crate::coordinator::apply_chinese_script_preference(&output, crate::types::ChineseScriptPreference::Traditional)
    } else { output })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::TEST_SERIAL as FLAGS;
    #[test]
    fn local_translation_paths_cannot_escape_cache() {
        for path in ["", "../name", "a/../b", "C:/temp", "a\\b", "/abs", "a:stream"] {
            assert!(relative(path).is_err(), "{path}");
        }
        assert!(relative("libs/ctranslate2/native.dll").is_ok());
    }
    #[test]
    fn local_translation_unknown_pair_never_selects_cloud() {
        assert_eq!(pair_for("unknown", "en").err().unwrap(), "local_translation_unsupported_pair");
    }
    #[test]
    fn local_translation_catalog_is_pinned_and_dedicated() {
        let catalog = catalog();
        assert_eq!(catalog.runtime.len(), 4);
        for asset in catalog.runtime.iter().chain(catalog.pairs.iter().map(|p| &p.asset)) {
            assert!(asset.url.starts_with("https://"));
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|x| x.is_ascii_hexdigit()));
            assert!(asset.bytes > 0 && asset.bytes < 150_000_000);
            assert!(relative(&asset.id).is_ok());
        }
        assert!(WORKER.contains("max_input_length=0"));
    }
    #[test]
    fn local_translation_manifest_rejects_corrupt_or_missing_data() {
        let temp = std::env::temp_dir().join(format!("listener-translation-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&temp).unwrap();
        fs::write(temp.join("test.bin"), b"original").unwrap();
        write_manifest(&temp).unwrap();
        assert!(manifest_ready(&temp, true));
        fs::write(temp.join("test.bin"), b"modified").unwrap();
        assert!(manifest_ready(&temp, false));
        assert!(!manifest_ready(&temp, true));
        fs::remove_file(temp.join("test.bin")).unwrap();
        assert!(!manifest_ready(&temp, false));
        // Only this explicit UUID-owned test directory is removed.
        let absolute = temp.canonicalize().unwrap();
        assert_eq!(absolute.parent(), Some(std::env::temp_dir().canonicalize().unwrap().as_path()));
        fs::remove_dir_all(absolute).unwrap();
    }

    #[test]
    fn local_translation_download_can_cancel_a_stalled_body() { stalled_download_cancels(true); }
    #[test]
    fn local_translation_download_can_cancel_before_response_headers() { stalled_download_cancels(false); }
    fn stalled_download_cancels(send_headers: bool) {
        let _flags = FLAGS.lock();
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let (ready, started) = std::sync::mpsc::channel();
        let (done, finish) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            stream.read(&mut request).unwrap();
            if send_headers { stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\na").unwrap(); }
            ready.send(()).unwrap();
            let _ = finish.recv_timeout(Duration::from_secs(4));
        });
        CANCEL.store(false, Ordering::Release);
        let cancel = std::thread::spawn(move || {
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            std::thread::sleep(Duration::from_millis(30));
            CANCEL.store(true, Ordering::Release);
        });
        let file = std::env::temp_dir().join(format!("listener-translation-{}.part", uuid::Uuid::new_v4()));
        let start = Instant::now();
        let error = download(&Asset { id: "fixture".into(), url, bytes: 4, sha256: "0".repeat(64) }, &file).unwrap_err();
        let _ = done.send(());
        server.join().unwrap(); cancel.join().unwrap();
        CANCEL.store(false, Ordering::Release);
        assert_eq!(error, "local_translation_cancelled");
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(file.parent().unwrap().canonicalize().unwrap(), std::env::temp_dir().canonicalize().unwrap());
        if send_headers { fs::remove_file(file).unwrap(); }
        else { assert!(!file.exists(), "cancel before headers must not create a partial asset"); }
    }

    #[test]
    fn local_translation_cancel_before_worker_start_cannot_be_reset() {
        let _flags = FLAGS.lock();
        let reservation = reserve_preparation("zh", "en").unwrap();
        assert!(PREPARING.load(Ordering::Acquire));
        cancel_preparation();
        assert_eq!(reservation.run().err().unwrap(), "local_translation_cancelled");
        assert!(!PREPARING.load(Ordering::Acquire));
        CANCEL.store(false, Ordering::Release);
    }

    #[test]
    #[ignore = "requires explicitly supplied saved public runtime/model archives; no download or daily data"]
    fn local_translation_saved_archive_worker_round_trip() {
        let fixtures = PathBuf::from(std::env::var_os("LISTENER_TRANSLATION_FIXTURE").expect("explicit saved fixtures"));
        let stage = Stage::new(&root().unwrap()).unwrap();
        let runtime = stage.path.join("runtime");
        fs::create_dir(&runtime).unwrap();
        for asset in catalog().runtime {
            let name = if asset.id == "vclibs" { "vclibs-desktop.appx" } else { asset.url.rsplit('/').next().unwrap() };
            let archive = fixtures.join("downloads").join(name);
            assert_eq!(archive.metadata().unwrap().len(), asset.bytes);
            assert_eq!(file_hash(&archive).unwrap(), asset.sha256);
            extract(&archive, &if ["python", "vclibs"].contains(&asset.id.as_str()) { runtime.clone() } else { runtime.join("libs") }, false, asset.id == "vclibs").unwrap();
        }
        fs::write(runtime.join("python313._pth"), "python313.zip\n.\nlibs\n").unwrap();
        fs::write(runtime.join("libs/ctranslate2/__init__.py"), MINIMAL_CT2_INIT).unwrap();
        write_manifest(&runtime).unwrap();
        assert!(manifest_ready(&runtime, true));
        let model = stage.path.join("zh-en");
        fs::create_dir(&model).unwrap();
        let pair = pair_for("zh", "en").unwrap();
        let archive = fixtures.join("translate-zh_en-1_9.argosmodel");
        assert_eq!(file_hash(&archive).unwrap(), pair.asset.sha256);
        extract(&archive, &model, true, false).unwrap();
        write_manifest(&model).unwrap();
        assert!(manifest_ready(&model, true));
        let output = worker_request(&runtime, &model, "请确认订单 A-102 的发货时间。\n\n不要发送邮件，只保存草稿。", Duration::from_secs(45)).unwrap();
        assert!(output.contains("A-102"));
        assert!(output.contains("\n\n"));
        assert!(output.to_lowercase().contains("do not send"));
        let long = "请确认五百件样品的报价，运费单独列出。".repeat(30) + "最后，请不要取消订单。";
        let output = worker_request(&runtime, &model, &long, Duration::from_secs(45)).unwrap();
        assert!(output.to_lowercase().contains("do not cancel the order"), "distinct final instruction must survive repetitive content");
        assert_eq!(output.matches("500").count(), 30);
        // Publish only into the isolated test profile to exercise the real
        // post-ASR voice route as well as the low-level worker. No microphone,
        // provider key, clipboard or foreground app is involved.
        let private = std::env::var_os("LISTENER_TYPE_DATA_DIR").expect("explicit private test profile");
        let private = PathBuf::from(private).canonicalize().unwrap();
        assert!(stage.root.starts_with(private));
        let published_runtime = stage.root.join(RUNTIME);
        let published_model = stage.root.join(pair.asset.id);
        assert!(!published_runtime.exists() && !published_model.exists());
        fs::rename(&runtime, &published_runtime).unwrap();
        fs::rename(&model, &published_model).unwrap();
        let prefs = crate::types::UserPreferences {
            translation_engine: crate::types::TranslationEngine::Local,
            translation_source_language: "zh".into(),
            translation_target_language: "English".into(),
            ..Default::default()
        };
        let raw = crate::asr::RawTranscript { text: "不要发送邮件，只保存草稿。".into(), duration_ms: 2500 };
        let (output, failure) = tokio::runtime::Runtime::new().unwrap().block_on(
            crate::coordinator::translate_voice_or_passthrough(&raw, &prefs, None));
        // Move all owned files back before assertions; Stage cleans them even
        // when the actual voice result fails the semantic checks below.
        fs::rename(published_runtime, runtime).unwrap();
        fs::rename(published_model, model).unwrap();
        assert!(failure.is_none(), "{failure:?}");
        assert!(output.to_lowercase().contains("do not send"));
    }
}
