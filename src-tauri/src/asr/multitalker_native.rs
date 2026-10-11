//! Windows CPU adapter for the trained English multitalker Parakeet bundle.
//!
//! This is a candidate batch component, not an active dictation provider. Run
//! it in a supervised worker: native model loading has no cancellation callback
//! and requires a parent that can kill the worker at the resource/time limit.
//! The decoder callback is an additional cooperative guard, not a hard timeout.
//!
//! Supply the complete original 16 kHz mono i16 PCM. No denoising, VAD trimming,
//! peak normalization or speaker rejection happens here. Channel numbers are
//! anonymous; they grant neither owner identity nor permission to end recording.
//! Native segment order is not certified word order.
//! It must not replace text already acknowledged by the shared insertion ledger.

use super::RawTranscript;
use super::content_consensus::AnonymousActivity;
use libloading::Library;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_void, CStr, CString, OsString};
use std::fs::File;
use std::io::{BufReader, Read};
use std::mem::{align_of, size_of, ManuallyDrop, MaybeUninit};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RATE: u32 = 16_000;
const MAX_SAMPLES: usize = RATE as usize * 30;
const MAX_RSS: u64 = 4 * 1024u64.pow(3);
const MIN_AVAILABLE: u64 = 2 * 1024u64.pow(3);
const DEADLINE: Duration = Duration::from_secs(60);
const MODEL_SHA256: &str = "cf83cedc6777d2528a4d2a8a4bf476174ef7e5c65b86bc6c955f4cc9f5951ac5";
const MODEL_BYTES: u64 = 873_443_456;
const VARIANT: &str = "multitalker-parakeet-streaming-0.6b-v1";
const MASKED_MODEL_SHA256: &str = "970b624a8a2be33450e56bb4ca4ff8380d2c5d795e6a1131d8831dd07746aef4";
const MASKED_MODEL_BYTES: u64 = 890_413_984;
const MASKED_VARIANT: &str = "nemotron-3.5-asr-streaming-0.6b";

#[derive(Clone, Copy)]
enum ContentProfile { EnglishKernel, ChineseActivityMask }

// transcribe.cpp v0.3.1 official Windows x86_64 CPU/Vulkan release. CPU is
// explicitly selected; hash every loadable DLL before initializing backends.
const DLLS: &[(&str, &str)] = &[
    (
        "transcribe.dll",
        "8dc86a6a86b589239a82ca6b2a9189855073257ea84cbc34e35ee269d73a50ae",
    ),
    (
        "ggml.dll",
        "f6628301f126538300b21455b4ac763876e10e4ae171b7f8cd8e5faeb9905a30",
    ),
    (
        "ggml-base.dll",
        "cb04a737ed9e0b13d3828cce2a63d77e89596fe60a0b9e658b86127de3e15790",
    ),
    (
        "ggml-cpu-cascadelake.dll",
        "526d48e7b3105050bd25af779f23b424144c58e15e35139de2380ad2538d3a7e",
    ),
    (
        "ggml-cpu-alderlake.dll",
        "39566ee10a69b3a97a0023bd24043243a6a1bebddb669272bcb4741a29358916",
    ),
    (
        "ggml-cpu-cannonlake.dll",
        "1e0b7ab14f87f7d9ef920f24e3c5f7bea45e052dc5d349143d8a35ca268ac3c2",
    ),
    (
        "ggml-cpu-haswell.dll",
        "9dd21a3c37d64d87fee0077f892f8a0b85255fb4929ad46a6d3a605fcbc88872",
    ),
    (
        "ggml-cpu-icelake.dll",
        "0688ed0224ff72f2ab9d91f5f7ece2fa21828f6ca2ea63e1d0d015bd1f2a1545",
    ),
    (
        "ggml-cpu-sandybridge.dll",
        "229c8919f1e2d46acbec7fb3ad87901247e32bd7f5bf5f24d848847241518d19",
    ),
    (
        "ggml-cpu-skylakex.dll",
        "f8dc5d3729b1e26965af6030858c64c1d5d06e70e8bd2126193e833e3d7586af",
    ),
    (
        "ggml-cpu-sse42.dll",
        "e93f7736051749221306a5061ddbe02ab251a1d70e0458da848f64e4111e903d",
    ),
    (
        "ggml-cpu-x64.dll",
        "e90f376dc22deafcd183456add765c71a6dac4223824a19bbd090499cde83a00",
    ),
    (
        "ggml-vulkan.dll",
        "f3a7c866cabc8e65c431dfac13cbdee541eed0e68fb27baf3aa83036df2337a7",
    ),
];

#[derive(Debug)]
pub(crate) struct MultitalkerAssets {
    pub runtime_directory: PathBuf,
    pub model_path: PathBuf,
}

#[derive(Debug, Serialize)]
pub(crate) struct AnonymousTranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub channel: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct MultitalkerResult {
    pub transcript: RawTranscript,
    pub segments: Vec<AnonymousTranscriptSegment>,
    pub activity: Vec<AnonymousActivity>,
    pub input_samples: usize,
    pub input_pcm_sha256: String,
    pub inference_ms: u64,
}

// ABI surface frozen from transcribe.cpp v0.3.1, header hash 57b1af43650f195d.
#[repr(C)]
struct BackendParams {
    struct_size: u64,
    artifact_dir: *const c_char,
    allowed_backends: u32,
}

#[repr(C)]
struct ModelParams {
    struct_size: u64,
    backend: i32,
    device: *mut c_void,
}

#[repr(C)]
struct SessionParams {
    struct_size: u64,
    n_threads: i32,
    kv_type: i32,
    n_ctx: i32,
}

#[repr(C)]
struct RunParams {
    struct_size: u64,
    task: i32,
    timestamps: i32,
    pnc: i32,
    itn: i32,
    diarize: i32,
    language: *const c_char,
    target_language: *const c_char,
    keep_special_tags: bool,
    family: *const c_void,
    spec_k_drafts: i32,
    vocabulary: *const *const c_char,
    n_vocabulary: i32,
    prompt: *const c_char,
    prefix: *const c_char,
}

#[repr(C)]
struct Segment {
    struct_size: u64,
    t0_ms: i64,
    t1_ms: i64,
    first_word: i32,
    n_words: i32,
    first_token: i32,
    n_tokens: i32,
    text: *const c_char,
    speaker_id: i32,
}

#[repr(C)]
struct SpeakerSegment {
    struct_size: u64,
    t0_ms: i64,
    t1_ms: i64,
    speaker_id: i32,
    probability: f32,
}

type AbortCallback = unsafe extern "C" fn(*mut c_void) -> bool;

macro_rules! native_api {
    ($($field:ident: $ty:ty => $symbol:literal),+ $(,)?) => {
        struct NativeApi {
            // Keep backend globals loaded for worker lifetime; handles below
            // still receive explicit session/model frees in the right order.
            _library: ManuallyDrop<Library>,
            directory: PathBuf,
            $($field: $ty),+
        }

        impl NativeApi {
            fn load(directory: &Path) -> Result<Self, String> {
                let canonical = directory.canonicalize().map_err(|e| e.to_string())?;
                verify_runtime(directory)?;
                verify_loaded_modules(&canonical)?;
                use libloading::os::windows::{Library as WindowsLibrary,
                    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR};
                let library: ManuallyDrop<Library> = ManuallyDrop::new(unsafe {
                    WindowsLibrary::load_with_flags(
                        canonical.join("transcribe.dll"),
                        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                    ).map_err(|e| format!("multitalker library: {e}"))?.into()
                });
                let api = unsafe { Self {
                    $($field: *library.get::<$ty>(concat!($symbol, "\0").as_bytes())
                        .map_err(|e| format!("multitalker symbol {}: {e}", $symbol))?),+,
                    _library: library,
                    directory: canonical,
                }};
                api.check_abi()?;
                if unsafe { string_from_native((api.version)()) }? != "0.3.1" {
                    return Err("multitalker native version mismatch".into());
                }
                let dir = path_string(directory)?;
                let mut params = unsafe { initialized(api.backend_params_init) };
                params.artifact_dir = dir.as_ptr();
                params.allowed_backends = 1; // TRANSCRIBE_BACKEND_MASK_CPU
                api.check("initialize CPU backend", unsafe { (api.init_backends)(&params) })?;
                verify_loaded_modules(&api.directory)?;
                Ok(api)
            }
        }
    };
}

native_api! {
    version: unsafe extern "C" fn() -> *const c_char => "transcribe_version",
    abi_size: unsafe extern "C" fn(i32) -> usize => "transcribe_abi_struct_size",
    abi_align: unsafe extern "C" fn(i32) -> usize => "transcribe_abi_struct_align",
    backend_params_init: unsafe extern "C" fn(*mut BackendParams) -> () => "transcribe_backend_init_params_init",
    init_backends: unsafe extern "C" fn(*const BackendParams) -> i32 => "transcribe_init_backends_ex",
    model_params_init: unsafe extern "C" fn(*mut ModelParams) -> () => "transcribe_model_load_params_init",
    model_load: unsafe extern "C" fn(*const c_char, *const ModelParams, *mut *mut c_void) -> i32 => "transcribe_model_load_file",
    model_free: unsafe extern "C" fn(*mut c_void) -> () => "transcribe_model_free",
    model_variant: unsafe extern "C" fn(*const c_void) -> *const c_char => "transcribe_model_variant_string",
    model_backend: unsafe extern "C" fn(*const c_void) -> *const c_char => "transcribe_model_backend",
    model_supports: unsafe extern "C" fn(*const c_void, i32) -> bool => "transcribe_model_supports",
    session_params_init: unsafe extern "C" fn(*mut SessionParams) -> () => "transcribe_session_params_init",
    session_init: unsafe extern "C" fn(*mut c_void, *const SessionParams, *mut *mut c_void) -> i32 => "transcribe_session_init",
    session_free: unsafe extern "C" fn(*mut c_void) -> () => "transcribe_session_free",
    run_params_init: unsafe extern "C" fn(*mut RunParams) -> () => "transcribe_run_params_init",
    run: unsafe extern "C" fn(*mut c_void, *const f32, i32, *const RunParams) -> i32 => "transcribe_run",
    set_abort: unsafe extern "C" fn(*mut c_void, AbortCallback, *mut c_void) -> () => "transcribe_set_abort_callback",
    full_text: unsafe extern "C" fn(*const c_void) -> *const c_char => "transcribe_full_text",
    segment_count: unsafe extern "C" fn(*const c_void) -> i32 => "transcribe_n_segments",
    segment_init: unsafe extern "C" fn(*mut Segment) -> () => "transcribe_segment_init",
    get_segment: unsafe extern "C" fn(*const c_void, i32, *mut Segment) -> i32 => "transcribe_get_segment",
    speaker_segment_count: unsafe extern "C" fn(*const c_void) -> i32 => "transcribe_n_speaker_segments",
    speaker_segment_init: unsafe extern "C" fn(*mut SpeakerSegment) -> () => "transcribe_speaker_segment_init",
    get_speaker_segment: unsafe extern "C" fn(*const c_void, i32, *mut SpeakerSegment) -> i32 => "transcribe_get_speaker_segment",
    status_string: unsafe extern "C" fn(i32) -> *const c_char => "transcribe_status_string",
}

impl NativeApi {
    fn check_abi(&self) -> Result<(), String> {
        for (id, size, alignment) in [
            (15, size_of::<BackendParams>(), align_of::<BackendParams>()),
            (0, size_of::<ModelParams>(), align_of::<ModelParams>()),
            (1, size_of::<SessionParams>(), align_of::<SessionParams>()),
            (2, size_of::<RunParams>(), align_of::<RunParams>()),
            (6, size_of::<Segment>(), align_of::<Segment>()),
            (14, size_of::<SpeakerSegment>(), align_of::<SpeakerSegment>()),
        ] {
            if unsafe { (self.abi_size)(id) } != size
                || unsafe { (self.abi_align)(id) } != alignment
            {
                return Err(format!("multitalker ABI layout mismatch: struct {id}"));
            }
        }
        Ok(())
    }

    fn check(&self, operation: &str, status: i32) -> Result<(), String> {
        if status == 0 {
            Ok(())
        } else {
            let detail = unsafe { string_from_native((self.status_string)(status)) }
                .unwrap_or_else(|_| "unknown native status".into());
            Err(format!("multitalker {operation}: {detail} ({status})"))
        }
    }
}

// The native initializers fully initialize their respective C ABI structures.
unsafe fn initialized<T>(init: unsafe extern "C" fn(*mut T)) -> T {
    let mut value = MaybeUninit::<T>::uninit();
    unsafe {
        init(value.as_mut_ptr());
        value.assume_init()
    }
}

unsafe fn string_from_native(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("multitalker returned a null string".into());
    }
    // Pointer validity and NUL termination are guaranteed by the pinned ABI.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|e| format!("multitalker invalid UTF-8: {e}"))
}

fn path_string(path: &Path) -> Result<CString, String> {
    if !path.is_absolute() {
        return Err("multitalker assets require absolute paths".into());
    }
    CString::new(path.to_str().ok_or("multitalker path is not UTF-8")?)
        .map_err(|_| "multitalker path contains NUL".into())
}

// Use Windows' optimized SHA-256 provider for asset files. In a debug build,
// hashing the pinned 833 MiB model with the Rust sha2 path takes about 20 s on
// the measured host. Every model/DLL byte and every original pin is still
// checked; this is not a metadata cache or a skipped integrity check.
#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(
        algorithm: *mut *mut c_void,
        algorithm_id: *const u16,
        implementation: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptCreateHash(
        algorithm: *mut c_void,
        hash: *mut *mut c_void,
        object: *mut u8,
        object_bytes: u32,
        secret: *const u8,
        secret_bytes: u32,
        flags: u32,
    ) -> i32;
    fn BCryptHashData(hash: *mut c_void, input: *const u8, input_bytes: u32, flags: u32) -> i32;
    fn BCryptFinishHash(hash: *mut c_void, output: *mut u8, output_bytes: u32, flags: u32) -> i32;
    fn BCryptDestroyHash(hash: *mut c_void) -> i32;
    fn BCryptCloseAlgorithmProvider(algorithm: *mut c_void, flags: u32) -> i32;
}

struct CngSha256 {
    algorithm: *mut c_void,
    hash: *mut c_void,
}

impl CngSha256 {
    fn check(operation: &str, status: i32) -> Result<(), String> {
        // NT_SUCCESS includes nonnegative success/informational statuses.
        if status < 0 {
            Err(format!(
                "multitalker {operation}: NTSTATUS 0x{:08x}",
                status as u32
            ))
        } else {
            Ok(())
        }
    }

    fn new() -> Result<Self, String> {
        let mut state = Self {
            algorithm: std::ptr::null_mut(),
            hash: std::ptr::null_mut(),
        };
        let algorithm = [83u16, 72, 65, 50, 53, 54, 0]; // SHA256\0
        Self::check("open SHA-256", unsafe {
            BCryptOpenAlgorithmProvider(
                &mut state.algorithm,
                algorithm.as_ptr(),
                std::ptr::null(),
                0,
            )
        })?;
        // Windows 7+ allocates the hash object when pbHashObject is null and
        // cbHashObject is zero, and releases it through BCryptDestroyHash.
        Self::check("create SHA-256", unsafe {
            BCryptCreateHash(
                state.algorithm,
                &mut state.hash,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                0,
                0,
            )
        })?;
        Ok(state)
    }

    fn update(&mut self, input: &[u8]) -> Result<(), String> {
        let size = u32::try_from(input.len()).map_err(|_| "multitalker hash block too large")?;
        Self::check("update SHA-256", unsafe {
            BCryptHashData(self.hash, input.as_ptr(), size, 0)
        })
    }

    fn finish(self) -> Result<String, String> {
        let mut digest = [0u8; 32];
        Self::check("finish SHA-256", unsafe {
            BCryptFinishHash(self.hash, digest.as_mut_ptr(), digest.len() as u32, 0)
        })?;
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

impl Drop for CngSha256 {
    fn drop(&mut self) {
        unsafe {
            if !self.hash.is_null() {
                BCryptDestroyHash(self.hash);
            }
            if !self.algorithm.is_null() {
                BCryptCloseAlgorithmProvider(self.algorithm, 0);
            }
        }
    }
}

fn reader_sha256(reader: &mut impl Read) -> Result<String, String> {
    let mut hash = CngSha256::new()?;
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n])?;
    }
    hash.finish()
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
    reader_sha256(&mut file)
}

fn require_hash(path: &Path, expected: &str) -> Result<(), String> {
    if file_sha256(path)? != expected {
        return Err(format!(
            "multitalker asset hash mismatch: {}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_runtime(directory: &Path) -> Result<(), String> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err("multitalker runtime requires an absolute directory".into());
    }
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".dll") && !DLLS.iter().any(|(known, _)| *known == name) {
            return Err(format!("multitalker unpinned DLL in runtime: {name}"));
        }
    }
    for (name, hash) in DLLS {
        require_hash(&directory.join(name), hash)?;
    }
    Ok(())
}

fn verify_loaded_modules(directory: &Path) -> Result<(), String> {
    // Windows can reuse a DLL already loaded by basename. Validate the actual
    // images, not only neighboring files; do not mutate PATH or DLL search dirs.
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, size: u32) -> u32;
    }
    for (name, hash) in DLLS {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if module.is_null() {
            continue;
        }
        let mut buffer = vec![0u16; 32768];
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
            as usize;
        if len == 0 || len >= buffer.len() {
            return Err("multitalker loaded module path unavailable".into());
        }
        let path = PathBuf::from(OsString::from_wide(&buffer[..len]));
        if path.canonicalize().map_err(|e| e.to_string())? != directory.join(name) {
            return Err(format!("multitalker conflicting loaded DLL: {name}"));
        }
        require_hash(&path, hash)?;
    }
    Ok(())
}

fn english_only(languages: &[&str]) -> bool {
    !languages.is_empty()
        && languages.iter().all(|language| {
            let mut parts = language.split('-');
            parts
                .next()
                .is_some_and(|primary| primary.eq_ignore_ascii_case("en"))
                && parts.all(|part| {
                    !part.is_empty()
                        && part.len() <= 8
                        && part.bytes().all(|c| c.is_ascii_alphanumeric())
                })
        })
}

fn complete_pcm_samples(pcm: &[u8], rate: u32, languages: &[&str]) -> Result<Vec<f32>, String> {
    if !english_only(languages) {
        return Err("multitalker candidate supports explicit English locales only".into());
    }
    if rate != RATE {
        return Err("multitalker requires original 16 kHz PCM".into());
    }
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return Err("multitalker invalid i16 PCM length".into());
    }
    if pcm.len() / 2 > MAX_SAMPLES {
        return Err(
            "multitalker candidate batch limit exceeded; preserve original ASR path".into(),
        );
    }
    Ok(pcm
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect())
}

fn check_memory_values(available: u64, rss: u64) -> Result<(), String> {
    if available < MIN_AVAILABLE || rss > MAX_RSS {
        Err("multitalker memory protection".into())
    } else {
        Ok(())
    }
}

fn resource_headroom() -> Result<(), String> {
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetCurrentProcess,
    };
    let mut memory = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut memory) }.map_err(|e| e.to_string())?;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    }
    .map_err(|e| e.to_string())?;
    check_memory_values(memory.ullAvailPhys, counters.WorkingSetSize as u64)
}

struct AbortState {
    started: Instant,
}

impl AbortState {
    fn check(&self) -> Result<(), String> {
        if self.started.elapsed() >= DEADLINE {
            return Err("multitalker deadline protection".into());
        }
        resource_headroom()
    }
}

unsafe extern "C" fn should_abort(state: *mut c_void) -> bool {
    // The synchronous run borrows this stack value, which outlives the session.
    // Guard failure (including a Rust panic) must not unwind through the C ABI.
    if state.is_null() {
        return true;
    }
    let state = unsafe { &*(state as *const AbortState) };
    std::panic::catch_unwind(|| state.check().is_err()).unwrap_or(true)
}

struct Handles<'a> {
    api: &'a NativeApi,
    model: *mut c_void,
    session: *mut c_void,
}

impl Drop for Handles<'_> {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                (self.api.session_free)(self.session);
            }
            if !self.model.is_null() {
                (self.api.model_free)(self.model);
            }
        }
    }
}

/// Worker-only candidate entry point. Errors leave the caller's original PCM
/// and original provider text intact. Do not use this on the Type UI/coordinator
/// process until a supervised child and shared final-text reconciliation exist.
pub(crate) fn transcribe_complete_pcm_in_worker(
    assets: &MultitalkerAssets,
    pcm: &[u8],
    sample_rate: u32,
    languages: &[&str],
) -> Result<MultitalkerResult, String> {
    transcribe_profile_in_worker(assets, pcm, sample_rate, languages, ContentProfile::EnglishKernel)
}

/// Ordinary multilingual ASR plus anonymous activity masks, not a trained
/// owner-recognition kernel. Only the qualified Chinese content route uses it.
pub(crate) fn transcribe_chinese_activity_in_worker(
    assets: &MultitalkerAssets, pcm: &[u8],
) -> Result<MultitalkerResult, String> {
    transcribe_profile_in_worker(assets, pcm, RATE, &["zh-CN"], ContentProfile::ChineseActivityMask)
}

fn transcribe_profile_in_worker(
    assets: &MultitalkerAssets, pcm: &[u8], sample_rate: u32,
    languages: &[&str], profile: ContentProfile,
) -> Result<MultitalkerResult, String> {
    static LEASE: Mutex<()> = Mutex::new(());
    static API: OnceLock<Result<NativeApi, String>> = OnceLock::new();
    let _lease = LEASE
        .try_lock()
        .map_err(|_| "multitalker worker is already busy")?;
    let mut abort = AbortState {
        started: Instant::now(),
    };
    abort.check()?;
    let (samples, model_bytes, model_hash, expected_variant, locale) = match profile {
        ContentProfile::EnglishKernel => {
            if std::env::var("TRANSCRIBE_MULTITALKER_MODE").is_ok_and(|mode| mode != "kernel")
                || std::env::var_os("TRANSCRIBE_MULTITALKER_OFFLINE").is_some() {
                return Err("multitalker conflicting diagnostic environment; require kernel mode".into());
            }
            (complete_pcm_samples(pcm, sample_rate, languages)?, MODEL_BYTES, MODEL_SHA256, VARIANT, "en")
        }
        ContentProfile::ChineseActivityMask => {
            if sample_rate != RATE || languages != ["zh-CN"]
                || std::env::var("TRANSCRIBE_MULTITALKER_MODE").as_deref() != Ok("masked")
                || std::env::var("TRANSCRIBE_MULTITALKER_OFFLINE").as_deref() != Ok("1") {
                return Err("content activity worker requires explicit Chinese offline mask mode".into());
            }
            // Reuse PCM validation without pretending that the English model
            // understands Chinese. Neither profile alters any input sample.
            (complete_pcm_samples(pcm, sample_rate, &["en"])?, MASKED_MODEL_BYTES,
                MASKED_MODEL_SHA256, MASKED_VARIANT, "zh-CN")
        }
    };
    if std::env::var_os("TRANSCRIBE_MULTITALKER_REF_BG_COMPAT").is_some() {
        return Err("multitalker conflicting reference-background compatibility mode".into());
    }
    let model_path = path_string(&assets.model_path)?;
    if std::fs::metadata(&assets.model_path)
        .map_err(|e| e.to_string())?
        .len()
        != model_bytes
    {
        return Err("multitalker model size mismatch".into());
    }
    require_hash(&assets.model_path, model_hash)?;
    abort.check()?;
    let api = API
        .get_or_init(|| NativeApi::load(&assets.runtime_directory))
        .as_ref()
        .map_err(Clone::clone)?;
    if assets
        .runtime_directory
        .canonicalize()
        .map_err(|e| e.to_string())?
        != api.directory
    {
        return Err("multitalker worker cannot change runtime directory".into());
    }
    let mut handles = Handles {
        api,
        model: std::ptr::null_mut(),
        session: std::ptr::null_mut(),
    };
    let mut model_params = unsafe { initialized(api.model_params_init) };
    model_params.backend = 1; // TRANSCRIBE_BACKEND_CPU
    api.check("load model", unsafe {
        (api.model_load)(model_path.as_ptr(), &model_params, &mut handles.model)
    })?;
    abort.check()?;
    if handles.model.is_null() {
        return Err("multitalker returned null model".into());
    }
    let variant = unsafe { string_from_native((api.model_variant)(handles.model)) }?;
    // This accessor returns ggml_backend_name ("CPU"), not the request's
    // lower-case enum label. Keep strict CPU selection and check that name.
    let backend = unsafe { string_from_native((api.model_backend)(handles.model)) }?;
    let diarization = unsafe { (api.model_supports)(handles.model, 6) }; // DIARIZATION
    if variant != expected_variant || !backend.eq_ignore_ascii_case("cpu") || !diarization {
        return Err(format!("multitalker trained model/CPU/diarization capability mismatch: variant={variant}, backend={backend}, diarization={diarization}"));
    }
    let mut session_params = unsafe { initialized(api.session_params_init) };
    session_params.n_threads = 4;
    api.check("create session", unsafe {
        (api.session_init)(handles.model, &session_params, &mut handles.session)
    })?;
    if handles.session.is_null() {
        return Err("multitalker returned null session".into());
    }
    unsafe {
        (api.set_abort)(
            handles.session,
            should_abort,
            &mut abort as *mut AbortState as *mut c_void,
        );
    }
    let language = CString::new(locale).expect("static language has no NUL");
    let mut run_params = unsafe { initialized(api.run_params_init) };
    run_params.task = 0;
    run_params.timestamps = 2;
    run_params.diarize = 2;
    run_params.language = language.as_ptr();
    // No evaluation phrase, enrollment phrase, vocabulary or transcript prefix
    // can enter the decoder as a prompt.
    run_params.prompt = std::ptr::null();
    run_params.prefix = std::ptr::null();
    run_params.vocabulary = std::ptr::null();
    run_params.n_vocabulary = 0;
    abort.check()?;
    let inference_start = Instant::now();
    api.check("transcribe complete PCM", unsafe {
        (api.run)(
            handles.session,
            samples.as_ptr(),
            samples.len() as i32,
            &run_params,
        )
    })?;
    let inference_ms = inference_start.elapsed().as_millis() as u64;
    abort.check()?;
    let text = unsafe { string_from_native((api.full_text)(handles.session)) }?;
    if text.trim().is_empty() {
        return Err("multitalker candidate returned no text".into());
    }
    let count = unsafe { (api.segment_count)(handles.session) };
    if !(0..=4096).contains(&count) {
        return Err("multitalker invalid segment count".into());
    }
    let mut segments = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut segment = unsafe { initialized(api.segment_init) };
        api.check("read segment", unsafe {
            (api.get_segment)(handles.session, i, &mut segment)
        })?;
        if segment.t0_ms < 0 || segment.t1_ms < segment.t0_ms {
            return Err("multitalker invalid segment timestamps".into());
        }
        segments.push(AnonymousTranscriptSegment {
            start_ms: segment.t0_ms,
            end_ms: segment.t1_ms,
            text: unsafe { string_from_native(segment.text) }?,
            channel: u32::try_from(segment.speaker_id).ok(),
        });
    }
    let activity_count = unsafe { (api.speaker_segment_count)(handles.session) };
    if !(0..=4096).contains(&activity_count) {
        return Err("multitalker invalid activity count".into());
    }
    let mut activity = Vec::with_capacity(activity_count as usize);
    for index in 0..activity_count {
        let mut span = unsafe { initialized(api.speaker_segment_init) };
        api.check("read speaker activity", unsafe {
            (api.get_speaker_segment)(handles.session, index, &mut span)
        })?;
        // Mask-mode probabilities are uncalibrated and may be NaN. Only its
        // bounded anonymous interval is content evidence; no owner probability.
        activity.push(AnonymousActivity {
            channel: u32::try_from(span.speaker_id).ok(), start_ms: span.t0_ms, end_ms: span.t1_ms,
        });
    }
    Ok(MultitalkerResult {
        transcript: RawTranscript {
            text,
            duration_ms: samples.len() as u64 * 1000 / RATE as u64,
        },
        segments,
        activity,
        input_samples: samples.len(),
        input_pcm_sha256: format!("{:x}", Sha256::digest(pcm)),
        inference_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_hash_matches_known_sha256_vectors_and_multiblock_data() {
        for (input, expected) in [
            (
                &b""[..],
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                &b"abc"[..],
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        ] {
            assert_eq!(
                reader_sha256(&mut std::io::Cursor::new(input)).unwrap(),
                expected
            );
        }
        // Cross both SHA-256 padding and the file-reader block boundary.
        for length in [55, 56, 63, 64, 65, 65_535, 65_536, 65_537, 1_048_833] {
            let data: Vec<u8> = (0..length).map(|i| ((i * 37 + 11) % 256) as u8).collect();
            assert_eq!(
                reader_sha256(&mut std::io::Cursor::new(&data)).unwrap(),
                format!("{:x}", Sha256::digest(&data)),
                "length={length}",
            );
        }
    }

    #[test]
    fn unreadable_or_changed_assets_never_return_a_partial_or_expected_hash() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("incomplete asset read"))
            }
        }
        assert!(reader_sha256(&mut BrokenReader).is_err());
        assert!(file_sha256(Path::new("missing-multitalker-asset-for-regression")).is_err());
        let actual = reader_sha256(&mut std::io::Cursor::new(b"abc!")).unwrap();
        assert_ne!(
            actual,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    #[ignore = "pinned model asset and a 60-second supervised verification worker required"]
    fn profile_pinned_asset_hash_without_model_inference() {
        let model = PathBuf::from(std::env::var_os("LISTENER_MULTITALKER_HASH_MODEL").unwrap());
        let started = Instant::now();
        require_hash(&model, MODEL_SHA256).unwrap();
        println!(
            "verified_model_sha256_seconds={:.3}",
            started.elapsed().as_secs_f64()
        );
    }

    #[test]
    fn full_pcm_keeps_quiet_owner_and_last_sample_without_peak_normalization() {
        let input = [0i16, -32768, 32, -64, 32767, 7];
        let pcm: Vec<u8> = input
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let samples = complete_pcm_samples(&pcm, RATE, &["en-US"]).unwrap();
        assert_eq!(samples.len(), input.len());
        assert_eq!(samples, input.map(|v| v as f32 / 32768.0));
        assert_eq!(*samples.last().unwrap(), 7.0 / 32768.0);
    }

    #[test]
    fn long_sessions_and_partial_samples_are_rejected_instead_of_cropped() {
        assert_eq!(
            complete_pcm_samples(&vec![0; MAX_SAMPLES * 2], RATE, &["en"])
                .unwrap()
                .len(),
            MAX_SAMPLES
        );
        assert!(complete_pcm_samples(&vec![0; (MAX_SAMPLES + 1) * 2], RATE, &["en"]).is_err());
        assert!(complete_pcm_samples(&[0, 1, 2], RATE, &["en"]).is_err());
        assert!(complete_pcm_samples(&[], RATE, &["en"]).is_err());
        assert!(complete_pcm_samples(&[0, 1], 48000, &["en"]).is_err());
    }

    #[test]
    fn multilingual_or_auto_selection_never_silently_uses_english() {
        for languages in [
            &[][..],
            &["auto"][..],
            &["zh-CN"][..],
            &["ja"][..],
            &["en-US", "zh-CN"][..],
            &["english"][..],
            &["en-"][..],
        ] {
            assert!(complete_pcm_samples(&[0, 0], RATE, languages).is_err());
        }
        assert!(complete_pcm_samples(&[0, 0], RATE, &["en-US", "en-GB"]).is_ok());
    }

    #[test]
    fn resource_and_deadline_failure_preserve_original_inputs() {
        assert!(check_memory_values(MIN_AVAILABLE - 1, 0).is_err());
        assert!(check_memory_values(MIN_AVAILABLE, MAX_RSS + 1).is_err());
        let expired = AbortState {
            started: Instant::now() - DEADLINE,
        };
        assert!(expired.check().is_err());
        let pcm = vec![0u8; 100];
        let before = pcm.clone();
        let assets = MultitalkerAssets {
            runtime_directory: PathBuf::from("missing-runtime"),
            model_path: PathBuf::from("missing-model"),
        };
        assert!(transcribe_complete_pcm_in_worker(&assets, &pcm, RATE, &["en"]).is_err());
        assert_eq!(pcm, before);
    }

    #[test]
    #[ignore = "public pinned audio/model/runtime and a 60-second supervised worker required"]
    fn frozen_public_fixture_through_type_native_adapter() {
        let protocol_path = PathBuf::from(
            std::env::var_os("LISTENER_MULTITALKER_PROTOCOL").expect("protocol path"),
        );
        let protocol: serde_json::Value =
            serde_json::from_slice(&std::fs::read(protocol_path).unwrap()).unwrap();
        let id = std::env::var("LISTENER_MULTITALKER_CASE").expect("case id");
        let case = protocol["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .expect("frozen case");
        let pcm = std::fs::read(case["pcmFile"].as_str().unwrap()).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&pcm)),
            case["pcmSha256"].as_str().unwrap()
        );
        let result = transcribe_complete_pcm_in_worker(
            &MultitalkerAssets {
                runtime_directory: PathBuf::from(protocol["runtimeDirectory"].as_str().unwrap()),
                model_path: PathBuf::from(protocol["modelPath"].as_str().unwrap()),
            },
            &pcm,
            RATE,
            &["en"],
        )
        .unwrap();
        let evidence = serde_json::json!({
            "case": id, "text": result.transcript.text, "durationMs": result.transcript.duration_ms,
            "inputSamples": result.input_samples, "inputPcmSha256": result.input_pcm_sha256,
            "inferenceMs": result.inference_ms, "segments": result.segments,
            "nativeVersion": "0.3.1", "backend": "cpu", "threads": 4,
            "diarize": "on", "mode": "kernel", "ownerIdentityAuthorized": false,
        });
        let evidence_file =
            PathBuf::from(std::env::var_os("LISTENER_MULTITALKER_OUTPUT").expect("output path"));
        std::fs::write(evidence_file, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        assert_eq!(
            result.input_samples as u64,
            case["sampleCount"].as_u64().unwrap()
        );
        assert_eq!(result.input_pcm_sha256, case["pcmSha256"].as_str().unwrap());
        let normalized = |text: &str| {
            text.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_uppercase)
                .collect::<String>()
        };
        let retained = normalized(&result.transcript.text)
            .contains(&normalized(case["expected"].as_str().unwrap()));
        assert_eq!(
            retained,
            case["mustRetain"].as_bool().unwrap(),
            "frozen target retention/control"
        );
    }
}
