//! Request-owned workers. Closing a request cancels only its process tree.
use super::*;
use std::sync::Arc;

static ACTIVE: parking_lot::Mutex<Option<(String, Arc<AtomicBool>)>> = parking_lot::Mutex::new(None);

pub(super) struct Request {
    id: String,
    pub(super) cancel: Arc<AtomicBool>,
}
impl Request {
    pub(super) fn reserve(id: Option<String>) -> Result<Self, String> {
        let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        uuid::Uuid::parse_str(&id).map_err(|_| "local_translation_invalid_request")?;
        let mut active = ACTIVE.lock();
        if active.is_some() { return Err("local_translation_busy".into()); }
        let cancel = Arc::new(AtomicBool::new(false));
        *active = Some((id.clone(), cancel.clone()));
        Ok(Self { id, cancel })
    }
    pub(super) fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) { Err("local_translation_cancelled".into()) } else { Ok(()) }
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock();
        if active.as_ref().is_some_and(|(id, _)| id == &self.id) { *active = None; }
    }
}
pub(crate) fn cancel(id: &str) {
    if let Some((owner, flag)) = ACTIVE.lock().as_ref() {
        if owner == id { flag.store(true, Ordering::Release); }
    }
}

pub(super) struct CancelOnDrop(pub Arc<AtomicBool>);
impl Drop for CancelOnDrop { fn drop(&mut self) { self.0.store(true, Ordering::Release); } }

struct Worker {
    child: std::process::Child,
    #[cfg(target_os = "windows")]
    job: Option<windows::Win32::Foundation::HANDLE>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if let Some(job) = self.job.take() { unsafe { let _ = windows::Win32::Foundation::CloseHandle(job); } }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Worker {
    fn own(child: std::process::Child) -> Result<Self, String> {
        let mut worker = Self { child, #[cfg(target_os = "windows")] job: None };
        #[cfg(target_os = "windows")]
        {
            use std::{mem::size_of, os::windows::io::AsRawHandle};
            use windows::Win32::{Foundation::HANDLE, System::JobObjects::*};
            // The Python worker waits for stdin EOF. Assign the job before any
            // request is written, so no descendant can escape the ownership.
            let job = unsafe { CreateJobObjectW(None, None) }.map_err(|_| "local_translation_runtime_failed")?;
            worker.job = Some(job);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 2;
            unsafe {
                SetInformationJobObject(job, JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const std::ffi::c_void, size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)
                    .map_err(|_| "local_translation_runtime_failed")?;
                AssignProcessToJobObject(job, HANDLE(worker.child.as_raw_handle() as *mut std::ffi::c_void))
                    .map_err(|_| "local_translation_runtime_failed")?;
            }
        }
        Ok(worker)
    }
}

pub(super) fn run(runtime: &Path, script: &str, request: serde_json::Value, timeout: Duration, owner: &Request) -> Result<String, String> {
    owner.check()?;
    let request = serde_json::to_vec(&request).map_err(|_| "local_translation_runtime_failed")?;
    let mut command = Command::new(runtime.join("python.exe"));
    command.env_clear();
    for key in ["SYSTEMROOT", "WINDIR", "PATH", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "SYSTEMDRIVE", "PROGRAMDATA", "COMSPEC"] {
        if let Some(value) = std::env::var_os(key) { command.env(key, value); }
    }
    command.args(["-I", "-S", "-X", "utf8", "-c", script]).current_dir(runtime)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    let mut worker = Worker::own(command.spawn().map_err(|_| "local_translation_runtime_failed")?)?;
    let stdout = worker.child.stdout.take().ok_or("local_translation_runtime_failed")?;
    let output = std::thread::spawn(move || { let mut bytes = Vec::new(); stdout.take(64_001).read_to_end(&mut bytes).map(|_| bytes) });
    let mut stdin = worker.child.stdin.take().ok_or("local_translation_runtime_failed")?;
    // A stalled child must not make a pipe write bypass Cancel or the deadline.
    let input = std::thread::spawn(move || stdin.write_all(&request));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Err(error) = owner.check() { break Err(error); }
        match worker.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => break Err("local_translation_timeout".to_owned()),
            Err(_) => break Err("local_translation_runtime_failed".to_owned()),
        }
    };
    // Kill descendants even when Python unexpectedly exits before its finally.
    drop(worker);
    let written = input.join().map_err(|_| "local_translation_runtime_failed")?;
    let bytes = output.join().map_err(|_| "local_translation_runtime_failed")?.map_err(|_| "local_translation_runtime_failed")?;
    let status = status?;
    written.map_err(|_| "local_translation_runtime_failed")?;
    if bytes.len() > 64_000 { return Err("local_translation_output_limit".into()); }
    let result: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "local_translation_runtime_failed")?;
    if let Some(error) = result["error"].as_str() {
        return Err(match error {
            "local_translation_memory_low" | "local_translation_context_limit" | "local_translation_output_limit"
            | "local_translation_date_fidelity" | "local_translation_time_fidelity" | "local_translation_structure_fidelity" | "local_translation_not_ready"
            | "local_translation_invalid_text" | "local_translation_unsupported_pair" | "local_translation_timeout"
            | "local_translation_empty_output" => error,
            _ => "local_translation_runtime_failed",
        }.into());
    }
    if !status.success() { return Err("local_translation_runtime_failed".into()); }
    result["text"].as_str().filter(|text| !text.trim().is_empty()).map(str::to_owned)
        .ok_or_else(|| "local_translation_empty_output".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_cancel_cannot_cancel_a_later_request() {
        let old = Request::reserve(None).unwrap();
        let old_id = old.id.clone();
        cancel(&old_id);
        assert_eq!(old.check().unwrap_err(), "local_translation_cancelled");
        drop(old);
        let next = Request::reserve(None).unwrap();
        cancel(&old_id);
        next.check().unwrap();
        assert_eq!(Request::reserve(None).err().unwrap(), "local_translation_busy");
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires an explicitly supplied verified embedded Python runtime; no model, download or daily data"]
    fn actual_worker_exit_cancel_timeout_reap_the_owned_descendant() {
        let _serial = TEST_SERIAL.lock();
        use windows::Win32::{Foundation::CloseHandle, System::Threading::{OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION}};
        let runtime = PathBuf::from(std::env::var_os("LISTENER_TRANSLATION_PYTHON_RUNTIME").expect("explicit isolated runtime"));
        assert!(runtime.join("python.exe").is_file());
        let stage = Stage::new(&std::env::temp_dir()).unwrap();
        let script = "import sys,json,subprocess,time\nfrom pathlib import Path\nr=json.loads(sys.stdin.read())\np=subprocess.Popen([sys.executable,'-I','-S','-c','import time; time.sleep(60)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=subprocess.CREATE_NO_WINDOW)\nPath(r['pidPath']).write_text(str(p.pid))\nif r['mode']=='exit': print(json.dumps({'text':'complete'}))\nelse: time.sleep(60)\n";
        for mode in ["exit", "cancel", "timeout"] {
            let marker = stage.path.join(format!("{mode}.pid"));
            let owner = Request::reserve(None).unwrap();
            let flag = owner.cancel.clone();
            let waiting_for = marker.clone();
            let cancel = if mode == "cancel" { Some(std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(4);
                while !waiting_for.exists() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
                flag.store(true, Ordering::Release);
            })) } else { None };
            let began = Instant::now();
            let result = run(&runtime, script, serde_json::json!({"mode":mode,"pidPath":marker}), Duration::from_secs(2), &owner);
            if let Some(cancel) = cancel { cancel.join().unwrap(); }
            match mode {
                "exit" => assert_eq!(result.unwrap(), "complete"),
                "cancel" => assert_eq!(result.unwrap_err(), "local_translation_cancelled"),
                _ => assert_eq!(result.unwrap_err(), "local_translation_timeout"),
            }
            assert!(began.elapsed() < Duration::from_secs(5));
            let pid = fs::read_to_string(marker).unwrap().parse::<u32>().unwrap();
            let until = Instant::now() + Duration::from_secs(1);
            loop {
                let alive = unsafe {
                    if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                        let mut code = 259u32;
                        let result = GetExitCodeProcess(handle, &mut code);
                        let _ = CloseHandle(handle);
                        result.is_ok() && code == 259
                    } else { false }
                };
                if !alive { break; }
                assert!(Instant::now() < until, "owned descendant survived {mode}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
