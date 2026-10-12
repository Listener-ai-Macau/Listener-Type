//! One memory-only request per isolated CPU process. No credentials/network.
//! A Windows job permits one process, kills it on drop, and prevents descendants.
use super::content_consensus::{AnonymousActivity, AnonymousContentSegment};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    mem::size_of,
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
    },
};

const MAX_WIRE_BYTES: u64 = 1_048_576;
const MAX_RSS: u64 = 4 * 1024u64.pow(3);
const MIN_AVAILABLE: u64 = 2 * 1024u64.pow(3);
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    request_id: String,
    source_sha256: String,
    reference_sha256: String,
    pcm_base64: String,
    reference_base64: String,
    want_activity: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    request_id: String,
    source_sha256: String,
    reference_sha256: String,
    output_sha256: String,
    input_samples: usize,
    pcm_base64: String,
    segments: Vec<AnonymousContentSegment>,
    activity: Vec<AnonymousActivity>,
}

#[derive(Debug)]
pub(crate) struct RecoveredAudio {
    pub source_sha256: String,
    pub pcm: Vec<u8>,
    pub input_samples: usize,
    pub segments: Vec<AnonymousContentSegment>,
    pub activity: Vec<AnonymousActivity>,
}

fn digest(pcm: &[u8]) -> String {
    format!("{:x}", Sha256::digest(pcm))
}

pub(crate) fn valid_pcm(pcm: &[u8]) -> bool {
    pcm.len() % 2 == 0
        && (1026..=super::owner_content_extraction::MAX_RECOVERY_PCM_BYTES).contains(&pcm.len())
}

fn native_assets() -> Result<super::multitalker_native::MultitalkerAssets, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed = exe
        .parent()
        .ok_or("content worker has no executable directory")?
        .join("resources/content-recovery");
    let root = if installed.is_dir() {
        installed
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/content-recovery")
    };
    Ok(super::multitalker_native::MultitalkerAssets {
        runtime_directory: root.join("runtime"),
        model_path: super::content_recovery_assets::ready_model()?,
    })
}

fn decode_request(request: Request) -> Result<Reply, String> {
    if uuid::Uuid::parse_str(&request.request_id).is_err() {
        return Err("content request id is invalid".into());
    }
    let pcm = STANDARD
        .decode(&request.pcm_base64)
        .map_err(|_| "invalid content PCM encoding")?;
    let reference = STANDARD
        .decode(&request.reference_base64)
        .map_err(|_| "invalid content reference encoding")?;
    if !valid_pcm(&pcm)
        || !valid_pcm(&reference)
        || digest(&pcm) != request.source_sha256
        || digest(&reference) != request.reference_sha256
    {
        return Err("content request PCM bounds or fingerprint mismatch".into());
    }
    let (segments, activity) = if request.want_activity {
        let result = super::multitalker_native::transcribe_chinese_activity_in_worker(
            &native_assets()?,
            &pcm,
        )?;
        if result.input_pcm_sha256 != request.source_sha256 || result.input_samples != pcm.len() / 2
        {
            return Err("content activity decoder changed its input binding".into());
        }
        (
            result
                .segments
                .into_iter()
                .map(|segment| AnonymousContentSegment {
                    channel: segment.channel,
                    text: segment.text,
                })
                .collect(),
            result.activity,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let overlapping = activity.iter().any(|a| {
        activity.iter().any(|b| {
            a.channel.is_some()
                && b.channel.is_some()
                && a.channel != b.channel
                && a.start_ms.max(b.start_ms) < a.end_ms.min(b.end_ms)
        })
    });
    // A clean, single-channel or sequential recording needs no reference
    // extraction or extra cloud request. The untouched original stays primary.
    let extracted = if !request.want_activity || overlapping {
        super::owner_content_extraction::extract_full_session(&reference, &pcm)?
    } else {
        pcm.clone()
    };
    if extracted.len() != pcm.len() {
        return Err("content worker lost complete PCM length".into());
    }
    Ok(Reply {
        request_id: request.request_id,
        source_sha256: request.source_sha256,
        reference_sha256: request.reference_sha256,
        output_sha256: digest(&extracted),
        input_samples: pcm.len() / 2,
        pcm_base64: STANDARD.encode(extracted),
        segments,
        activity,
    })
}

pub(crate) fn run_helper() -> i32 {
    // This process cannot accept paths, credentials, prompts or another request.
    let result = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(MAX_WIRE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_WIRE_BYTES {
            return Err("content request is oversized".into());
        }
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|_| "invalid content request")?;
        let reply = decode_request(request)?;
        let bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("content worker: {error}");
            1
        }
    }
}

struct ProcessGuard {
    child: Child,
    job: HANDLE,
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Close kills the entire job even if an unexpected descendant escaped
        // a library failure. ActiveProcessLimit=1 prevents creating one.
        unsafe {
            let _ = CloseHandle(self.job);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn memory(process: HANDLE) -> Result<(u64, u64), String> {
    let mut available = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut available) }.map_err(|e| e.to_string())?;
    let rss = if process.is_invalid() {
        0
    } else {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        unsafe {
            GetProcessMemoryInfo(
                process,
                &mut counters,
                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        }
        .map_err(|e| e.to_string())?;
        counters.WorkingSetSize as u64
    };
    Ok((available.ullAvailPhys, rss))
}

fn supervised_request(
    reference: Vec<u8>,
    pcm: Vec<u8>,
    want_activity: bool,
    cancelled: Arc<AtomicBool>,
) -> Result<RecoveredAudio, String> {
    static LEASE: Mutex<()> = Mutex::new(());
    let _lease = LEASE
        .try_lock()
        .map_err(|_| "content recovery is already busy")?;
    if !valid_pcm(&pcm) || !valid_pcm(&reference) {
        return Err("content recovery requires complete bounded PCM".into());
    }
    if memory(HANDLE::default())?.0 < MIN_AVAILABLE || cancelled.load(Ordering::Acquire) {
        return Err("content recovery cancelled or insufficient headroom".into());
    }
    let started = Instant::now();
    let request = Request {
        request_id: uuid::Uuid::new_v4().to_string(),
        source_sha256: digest(&pcm),
        reference_sha256: digest(&reference),
        pcm_base64: STANDARD.encode(&pcm),
        reference_base64: STANDARD.encode(reference),
        want_activity,
    };
    let request_bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--content-recovery-helper")
        .env("TRANSCRIBE_MULTITALKER_MODE", "masked")
        .env("TRANSCRIBE_MULTITALKER_OFFLINE", "1")
        .env_remove("TRANSCRIBE_MULTITALKER_REF_BG_COMPAT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("start content recovery: {e}"))?;
    let job = match unsafe { CreateJobObjectW(None, None) } {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
    };
    let mut worker = ProcessGuard { child, job };
    let process = HANDLE(worker.child.as_raw_handle() as *mut std::ffi::c_void);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|e| e.to_string())?;
        AssignProcessToJobObject(job, process).map_err(|e| e.to_string())?;
    }
    let mut stdin = worker
        .child
        .stdin
        .take()
        .ok_or("content worker stdin unavailable")?;
    let stdout = worker
        .child
        .stdout
        .take()
        .ok_or("content worker stdout unavailable")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&request_bytes);
    });
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MAX_WIRE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let mut peak = 0;
    let mut minimum = u64::MAX;
    loop {
        if cancelled.load(Ordering::Acquire) || started.elapsed() >= DEADLINE {
            return Err("content recovery cancelled or deadline exceeded".into());
        }
        let (available, rss) = memory(process)?;
        peak = peak.max(rss);
        minimum = minimum.min(available);
        if available < MIN_AVAILABLE || rss >= MAX_RSS {
            return Err("content recovery memory protection".into());
        }
        if let Some(status) = worker.child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err(format!("content worker exited with {status}"));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let bytes = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| "content response incomplete")?
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_WIRE_BYTES {
        return Err("content response oversized".into());
    }
    let reply: Reply =
        serde_json::from_slice(&bytes).map_err(|_| "invalid content worker response")?;
    let extracted = STANDARD
        .decode(reply.pcm_base64)
        .map_err(|_| "invalid content output encoding")?;
    if reply.request_id != request.request_id
        || reply.source_sha256 != request.source_sha256
        || reply.reference_sha256 != request.reference_sha256
        || reply.input_samples != pcm.len() / 2
        || extracted.len() != pcm.len()
        || digest(&extracted) != reply.output_sha256
    {
        return Err("content worker derivation receipt mismatch".into());
    }
    log::info!("[content-recovery] isolated worker complete elapsed_ms={} peak_rss={} minimum_available={} samples={} channels={} identity_authorized=false",
        started.elapsed().as_millis(),peak,minimum,reply.input_samples,reply.segments.len());
    Ok(RecoveredAudio {
        source_sha256: reply.source_sha256,
        pcm: extracted,
        input_samples: reply.input_samples,
        segments: reply.segments,
        activity: reply.activity,
    })
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) async fn recover(
    reference: Vec<u8>,
    pcm: Vec<u8>,
    want_activity: bool,
) -> Result<RecoveredAudio, String> {
    if want_activity {
        super::content_recovery_assets::prepare_in_background("zh-CN");
        // Never wait for an 890 MB network transfer in finalization. The
        // model-independent empty-final extraction route still works offline.
        super::content_recovery_assets::ready_model()?;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(Arc::clone(&cancel));
    tauri::async_runtime::spawn_blocking(move || {
        supervised_request(reference, pcm, want_activity, cancel)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlong_odd_and_empty_pcm_are_rejected_without_cropping() {
        assert!(!valid_pcm(&[]));
        assert!(!valid_pcm(&vec![0; 1027]));
        assert!(!valid_pcm(&vec![0; 384002]));
        assert!(valid_pcm(&vec![0; 384000]));
    }
    #[test]
    fn cancelled_work_does_not_start_any_child() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(
            supervised_request(vec![0; 16000], vec![0; 16000], true, cancel)
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn invalid_reference_hash_is_rejected_before_loading_models() {
        let request = Request {
            request_id: uuid::Uuid::new_v4().to_string(),
            source_sha256: digest(&vec![0; 16000]),
            reference_sha256: "a".repeat(64),
            pcm_base64: STANDARD.encode(vec![0; 16000]),
            reference_base64: STANDARD.encode(vec![0; 16000]),
            want_activity: true,
        };
        assert!(decode_request(request).unwrap_err().contains("fingerprint"));
    }
}
