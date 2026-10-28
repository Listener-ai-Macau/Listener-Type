//! Explicitly downloaded CPU-only translator; never a cloud fallback.
use super::*;

const MODEL_FILE: &str = "Hy-MT2-1.8B-Q4_K_M.gguf";
const CRT: [&str; 3] = ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"];
#[derive(Deserialize)]
struct EnhancedCatalog { runtime: Asset, model: Asset }
fn assets() -> EnhancedCatalog {
    serde_json::from_str(include_str!("../../resources/translation/enhanced-catalog.json")).expect("compiled CPU translation assets")
}

pub(super) fn status() -> Result<Status, String> {
    let root = root()?;
    let assets = assets();
    let base_ready = manifest_ready(&root.join(RUNTIME), false);
    let runtime_ready = base_ready && manifest_ready(&root.join(&assets.runtime.id), false);
    let model_ready = manifest_ready(&root.join(&assets.model.id), false);
    let ready = runtime_ready && model_ready;
    Ok(Status {
        supported: cfg!(all(target_os = "windows", target_arch = "x86_64")), runtime_ready,
        runtime_download_bytes: if base_ready { 0 } else { catalog().runtime.iter().map(|a| a.bytes).sum::<u64>() }
            + if manifest_ready(&root.join(&assets.runtime.id), false) { 0 } else { assets.runtime.bytes },
        pairs: [("zh", "en"), ("en", "zh")].into_iter().map(|(source,target)| PairStatus {
            source: source.into(), target: target.into(), download_bytes: if model_ready { 0 } else { assets.model.bytes }, ready,
        }).collect(),
        preparing: PREPARING.load(Ordering::Acquire), received_bytes: RECEIVED.load(Ordering::Relaxed), expected_bytes: EXPECTED.load(Ordering::Relaxed),
    })
}

fn publish(stage: &Stage, content: &Path, dest: &Path) -> Result<(), String> {
    write_manifest(content)?;
    cancelled()?;
    let previous = stage.path.join("previous");
    if dest.exists() { fs::rename(dest, &previous).map_err(|e| e.to_string())?; }
    if let Err(error) = fs::rename(content, dest) {
        if previous.exists() { let _ = fs::rename(&previous, dest); }
        return Err(error.to_string());
    }
    Ok(())
}

fn cpu_file(name: &str) -> bool {
    ["llama-server.exe", "llama-server-impl.dll", "llama-common.dll", "llama.dll", "mtmd.dll", "ggml.dll", "ggml-base.dll", "libomp.dll", "LICENSE-LLVM-OpenMP"].contains(&name)
        || name.starts_with("ggml-cpu-") && name.ends_with(".dll")
}

pub(super) fn prepare(root: &Path) -> Result<(), String> {
    let assets = assets();
    let base_missing = !manifest_ready(&root.join(RUNTIME), true);
    let cpu_missing = !manifest_ready(&root.join(&assets.runtime.id), true);
    let model_missing = !manifest_ready(&root.join(&assets.model.id), true);
    EXPECTED.store(if base_missing { catalog().runtime.iter().map(|a| a.bytes).sum::<u64>() } else { 0 }
        + if cpu_missing { assets.runtime.bytes } else { 0 } + if model_missing { assets.model.bytes } else { 0 }, Ordering::Relaxed);
    prepare_directory(root, RUNTIME, &catalog().runtime, false)?;
    if cpu_missing {
        let stage = Stage::new(root)?;
        let archive = stage.path.join("runtime.zip");
        download(&assets.runtime, &archive)?;
        let content = stage.path.join("content");
        fs::create_dir(&content).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut total = 0u64;
        for index in 0..zip.len() {
            cancelled()?;
            let mut item = zip.by_index(index).map_err(|e| e.to_string())?;
            if item.is_dir() { continue; }
            let name = relative(item.name())?.to_owned();
            if !cpu_file(&name.to_string_lossy()) { continue; }
            if item.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) { return Err("local_translation_invalid_asset_path".into()); }
            total += item.size();
            if total > 100_000_000 { return Err("local_translation_asset_size_limit".into()); }
            let mut file = File::create(content.join(name)).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut item.by_ref().take(100_000_001), &mut file).map_err(|e| e.to_string())?;
            if copied != item.size() { return Err("local_translation_checksum_failed".into()); }
        }
        for name in CRT { fs::copy(root.join(RUNTIME).join(name), content.join(name)).map_err(|_| "local_translation_not_ready")?; }
        for name in ["llama-server.exe", "llama-server-impl.dll", "llama-common.dll", "llama.dll", "mtmd.dll", "ggml.dll", "ggml-base.dll", "ggml-cpu-x64.dll", "libomp.dll"] {
            if !content.join(name).is_file() { return Err("local_translation_not_ready".into()); }
        }
        fs::write(content.join("LICENSE-llama.cpp"), include_str!("../../resources/translation/LICENSE-llama.cpp")).map_err(|e| e.to_string())?;
        publish(&stage, &content, &root.join(&assets.runtime.id))?;
    }
    if model_missing {
        let stage = Stage::new(root)?;
        let content = stage.path.join("content");
        fs::create_dir(&content).map_err(|e| e.to_string())?;
        download_model(&assets.model, &content.join(MODEL_FILE))?;
        fs::write(content.join("LICENSE-Hy-MT2.txt"), include_str!("../../resources/translation/LICENSE-Hy-MT2.txt")).map_err(|e| e.to_string())?;
        publish(&stage, &content, &root.join(&assets.model.id))?;
    }
    Ok(())
}

// Bounded ranges resume a transport failure within this preparation. An
// interrupted range is truncated before retry; only a full SHA publishes it.
fn download_model(asset: &Asset, path: &Path) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).read_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300));
        #[cfg(test)]
        let builder = builder.no_proxy();
        let client = builder.build().map_err(|_| "local_translation_download_failed")?;
        let mut output = File::create(path).map_err(|e| e.to_string())?;
        let mut position = 0u64;
        let mut hash = Sha256::new();
        while position < asset.bytes {
            cancelled()?;
            let end = (position + 4 * 1024 * 1024 - 1).min(asset.bytes - 1);
            let mut range = None;
            for _ in 0..3 {
                let transfer = async {
                    let mut response = client.get(&asset.url).header(reqwest::header::RANGE, format!("bytes={position}-{end}"))
                        .send().await.map_err(|_| "local_translation_download_failed")?;
                    let expected = format!("bytes {position}-{end}/{}", asset.bytes);
                    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                        || response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
                        return Err("local_translation_download_failed");
                    }
                    let mut bytes = Vec::with_capacity((end-position+1) as usize);
                    while let Some(chunk) = response.chunk().await.map_err(|_| "local_translation_download_failed")? {
                        if bytes.len() as u64 + chunk.len() as u64 > end-position+1 { return Err("local_translation_checksum_failed"); }
                        bytes.extend_from_slice(&chunk);
                    }
                    if bytes.len() as u64 != end-position+1 { return Err("local_translation_download_failed"); }
                    Ok(bytes)
                };
                let cancellation = async { while !CANCEL.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(50)).await; } };
                let result = tokio::select! { biased; _ = cancellation => return Err("local_translation_cancelled".to_owned()), result = transfer => result };
                match result { Ok(bytes) => { range = Some(bytes); break; }, Err("local_translation_checksum_failed") => return Err("local_translation_checksum_failed".into()), Err(_) => cancelled()? }
            }
            let bytes = range.ok_or("local_translation_download_failed")?;
            output.write_all(&bytes).map_err(|e| e.to_string())?;
            hash.update(&bytes);
            position += bytes.len() as u64;
            RECEIVED.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        output.sync_all().map_err(|e| e.to_string())?;
        if format!("{:x}", hash.finalize()) != asset.sha256 { return Err("local_translation_checksum_failed".into()); }
        Ok(())
    })
}

pub(super) fn translate(root: &Path, source: &str, target: &str, text: &str, owner: &supervised::Request) -> Result<String, String> {
    let assets = assets();
    for id in [RUNTIME, &assets.runtime.id, &assets.model.id] {
        owner.check()?;
        if !manifest_ready(&root.join(id), true) { return Err("local_translation_not_ready".into()); }
    }
    let script = format!("__name__='listener_translation_worker'\n{WORKER}\n{}\n{}\n{}\nenhanced_main()\n",
        include_str!("../../resources/translation/date_fidelity.py"), include_str!("../../resources/translation/result_fidelity.py"), include_str!("../../resources/translation/enhanced_worker.py"));
    supervised::run(&root.join(RUNTIME), &script, serde_json::json!({"text": text, "source": source, "target": target,
        "runtime": root.join(assets.runtime.id), "model": root.join(assets.model.id).join(MODEL_FILE)}), Duration::from_secs(180), owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_archive_excludes_accelerators_and_unneeded_executables() {
        assert!(cpu_file("ggml-cpu-x64.dll")); assert!(cpu_file("llama-server-impl.dll"));
        for name in ["ggml-vulkan.dll", "ggml-rpc.dll", "llama-cli.exe", "ggml-rpc-server.exe", "../ggml-cpu-x64.dll"] { assert!(!cpu_file(name), "{name}"); }
    }

    #[test]
    fn ranged_model_retry_keeps_exact_bytes_and_rejects_wrong_final_hash() {
        let _serial = TEST_SERIAL.lock();
        use std::net::TcpListener;
        let bytes = vec![b'x'; 4 * 1024 * 1024 + 5];
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for attempt in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0]; assert_eq!(stream.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]); assert!(request.len() < 4096);
                }
                let (start, end) = if attempt < 2 { (0, 4 * 1024 * 1024 - 1) } else { (4 * 1024 * 1024, bytes.len()-1) };
                assert!(String::from_utf8_lossy(&request).to_lowercase().contains(&format!("range: bytes={start}-{end}")));
                write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len(), end-start+1).unwrap();
                if attempt == 0 { stream.write_all(b"x").unwrap(); }
                else { stream.write_all(&bytes[start..=end]).unwrap(); }
            }
        });
        CANCEL.store(false, Ordering::Release);
        let stage = Stage::new(&std::env::temp_dir()).unwrap();
        let file = stage.path.join("model.gguf");
        let asset = Asset { id: "fixture".into(), url, bytes: 4 * 1024 * 1024 + 5, sha256: "0".repeat(64) };
        assert_eq!(download_model(&asset, &file).unwrap_err(), "local_translation_checksum_failed");
        server.join().unwrap();
        assert_eq!(file.metadata().unwrap().len(), asset.bytes);
        assert_eq!(file_hash(&file).unwrap(), digest);
        // A fully received but mismatching model never gets a ready manifest.
        assert!(!manifest_ready(&stage.path, true));
    }

    #[test]
    fn ranged_model_cancel_interrupts_stalled_headers() {
        let _serial = TEST_SERIAL.lock();
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let (ready, begun) = std::sync::mpsc::channel();
        let (done, finish) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048]; stream.read(&mut request).unwrap();
            ready.send(()).unwrap(); let _ = finish.recv_timeout(Duration::from_secs(4));
        });
        CANCEL.store(false, Ordering::Release);
        let cancel = std::thread::spawn(move || { begun.recv_timeout(Duration::from_secs(3)).unwrap(); CANCEL.store(true, Ordering::Release); });
        let stage = Stage::new(&std::env::temp_dir()).unwrap();
        let started = Instant::now();
        let result = download_model(&Asset { id: "fixture".into(), url, bytes: 5, sha256: "0".repeat(64) }, &stage.path.join("model.gguf"));
        let _ = done.send(()); cancel.join().unwrap(); server.join().unwrap(); CANCEL.store(false, Ordering::Release);
        assert_eq!(result.unwrap_err(), "local_translation_cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!manifest_ready(&stage.path, true));
    }
}
