//! Optional content weights live in a verified user cache, never in the MSI.
//! Preparation runs off the dictation path. A worker only reads complete assets;
//! first use during a download retains primary ASR without waiting for the network.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const NAME: &str = "multilingual-activity-mask.gguf";
const BYTES: u64 = 890_413_984;
const SHA256: &str = "970b624a8a2be33450e56bb4ca4ff8380d2c5d795e6a1131d8831dd07746aef4";
// Immutable, public LFS object in our repository. No customer credentials/audio.
const URL: &str = "https://media.githubusercontent.com/media/Listener-ai-Macau/Listener-Type/2d0f5305c246e8ac4f20ab82154d027e41482a96/src-tauri/resources/content-recovery/multilingual-activity-mask.gguf";
static PREPARING: AtomicBool = AtomicBool::new(false);
static PREPARED: AtomicBool = AtomicBool::new(false);

fn cache_path() -> Result<PathBuf, String> {
    let root = crate::persistence::content_recovery_model_root().map_err(|e| e.to_string())?;
    Ok(root.join(format!("{SHA256}.gguf")))
}

fn legacy_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("resources/content-recovery").join(NAME);
    path.is_file().then_some(path)
}

/// Native decoding independently hashes this file before loading any weights.
/// A temporary download can never appear at this published cache path.
pub(crate) fn ready_model() -> Result<PathBuf, String> {
    let cache = cache_path()?;
    if cache.metadata().is_ok_and(|m| m.len() == BYTES) {
        return Ok(cache);
    }
    if let Some(path) = legacy_path() {
        if path.metadata().is_ok_and(|m| m.len() == BYTES) {
            return Ok(path);
        }
    }
    #[cfg(debug_assertions)]
    {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/content-recovery")
            .join(NAME);
        if source.metadata().is_ok_and(|m| m.len() == BYTES) {
            return Ok(source);
        }
    }
    Err("optional content model is preparing; primary transcription remains available".into())
}

fn copy_verified(
    mut input: impl Read,
    mut output: impl Write,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<(), String> {
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > expected_bytes {
            return Err("content model exceeds expected size".into());
        }
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
    }
    if count != expected_bytes || format!("{:x}", hash.finalize()) != expected_hash {
        return Err("content model download size or fingerprint mismatch".into());
    }
    Ok(())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn prepare() -> Result<(), String> {
    let cache = cache_path()?;
    remove_abandoned_downloads(cache.parent().ok_or("model cache has no parent")?)?;
    if let Ok(file) = File::open(&cache) {
        if copy_verified(file, std::io::sink(), BYTES, SHA256).is_ok() {
            return Ok(());
        }
    }
    if let Some(legacy) = legacy_path() {
        publish_verified(
            &cache,
            File::open(legacy).map_err(|e| e.to_string())?,
            BYTES,
            SHA256,
        )
    } else {
        log::info!("[content-model] downloading optional model bytes={BYTES} primary_waits=false");
        let response = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|e| e.to_string())?
            .get(URL)
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        if response.content_length().is_some_and(|n| n != BYTES) {
            return Err("content model server size mismatch".into());
        }
        publish_verified(&cache, response, BYTES, SHA256)
    }
}

fn remove_abandoned_downloads(root: &Path) -> Result<(), String> {
    // A single app instance owns preparation. After process termination its
    // private UUID staging file may remain, but must not accumulate per retry.
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name
            .strip_prefix(&format!("{SHA256}."))
            .and_then(|n| n.strip_suffix(".part"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_ok()
            && entry.file_type().map_err(|e| e.to_string())?.is_file()
        {
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn publish_verified(cache: &Path, input: impl Read, bytes: u64, hash: &str) -> Result<(), String> {
    let temporary = TemporaryFile(cache.with_extension(format!("{}.part", uuid::Uuid::new_v4())));
    let mut output = File::create(&temporary.0).map_err(|e| e.to_string())?;
    copy_verified(input, &mut output, bytes, hash)?;
    output.sync_all().map_err(|e| e.to_string())?;
    drop(output);
    // On Windows rename replaces an existing file atomically; neither a partial
    // response nor failed checksum can replace a previously published model.
    fs::rename(&temporary.0, &cache).map_err(|e| e.to_string())?;
    log::info!("[content-model] optional model ready bytes={BYTES}");
    Ok(())
}

pub(crate) fn prepare_in_background(locale: &str) {
    if locale != "zh-CN"
        || (PREPARED.load(Ordering::Acquire) && cache_path().is_ok_and(|p| p.is_file()))
        || PREPARING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    if std::thread::Builder::new()
        .name("content-model-download".into())
        .spawn(|| {
            match prepare() {
                Ok(()) => PREPARED.store(true, Ordering::Release),
                Err(error) => log::warn!(
                    "[content-model] optional download unavailable; primary retained: {error}"
                ),
            }
            PREPARING.store(false, Ordering::Release);
        })
        .is_err()
    {
        PREPARING.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_stream_only_publishes_the_exact_pinned_bytes() {
        let bytes = b"model fixture";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let mut out = Vec::new();
        copy_verified(&bytes[..], &mut out, bytes.len() as u64, &hash).unwrap();
        assert_eq!(out, bytes);
        assert!(copy_verified(&bytes[..3], Vec::new(), bytes.len() as u64, &hash).is_err());
        assert!(copy_verified(&bytes[..], Vec::new(), 3, &hash).is_err());
        assert!(
            copy_verified(&bytes[..], Vec::new(), bytes.len() as u64, &"0".repeat(64)).is_err()
        );
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::ConnectionReset.into())
            }
        }
        assert!(copy_verified(Failed, Vec::new(), bytes.len() as u64, &hash).is_err());
    }

    #[test]
    fn interrupted_or_corrupt_download_keeps_ready_cache_and_leaves_no_partial_file() {
        let root =
            std::env::temp_dir().join(format!("listener-content-assets-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("pinned.gguf");
        fs::write(&path, b"previous ready bytes").unwrap();
        let bytes = b"new verified model";
        let hash = format!("{:x}", Sha256::digest(bytes));
        for (input, expected) in [(&bytes[..3], hash.as_str()), (&bytes[..], "bad hash")] {
            assert!(publish_verified(&path, input, bytes.len() as u64, expected).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"previous ready bytes");
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        }
        publish_verified(&path, &bytes[..], bytes.len() as u64, &hash).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn restart_reclaims_only_this_models_abandoned_uuid_staging_files() {
        let root =
            std::env::temp_dir().join(format!("listener-content-restart-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let abandoned = root.join(format!("{SHA256}.{}.part", uuid::Uuid::new_v4()));
        fs::write(&abandoned, b"incomplete").unwrap();
        for name in ["other-model.part", "user-notes.txt", "pinned.gguf"] {
            fs::write(root.join(name), b"keep").unwrap();
        }
        remove_abandoned_downloads(&root).unwrap();
        assert!(!abandoned.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        for name in ["other-model.part", "user-notes.txt", "pinned.gguf"] {
            fs::remove_file(root.join(name)).unwrap();
        }
        fs::remove_dir(root).unwrap();
    }
}
