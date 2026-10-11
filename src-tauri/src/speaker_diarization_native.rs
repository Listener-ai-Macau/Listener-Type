//! Thin, path-injected wrapper around NeMo-Speech's standalone diarization C ABI.
//!
//! The native speaker numbers exposed here are anonymous, stream-local channels.
//! They are temporal evidence only. A caller must bind a channel to the enrolled
//! owner with independent identity evidence before it can authorize Owner/Other.

use libloading::Library;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::ffi::{c_char, c_void, CStr, CString};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) const NEMOTRON_V3_Q8_SHA256: &str =
    "08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1";
pub(crate) const NEMOTRON_V3_CHANNELS: usize = 8;
pub(crate) const NEMOTRON_V3_FRAME_SECONDS: f64 = 0.01;

// Audited release 6a3ca369370782acee0dd155e34394ba60b920c4. Its standalone
// V3 push calls feed_audio -> run_ready_chunks(false), never flush_available;
// therefore every published raw frame is committed. This is an implementation
// contract for both loaded images, not a promise made by the generic C ABI.
const AUDITED_WINDOWS_C_API_SHA256: &str =
    "9f6390224bf8d9c7a81830507ec9d937277beb079c9f0bdee15bd0f25b11a7f5";
const AUDITED_WINDOWS_IMPLEMENTATION_SHA256: &str =
    "f7129dc212c4d353fb76279e10244fed92c4f7814e80e408c51777a6045f11be";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveFrameContract {
    Unspecified,
    AuditedStandaloneV3Push,
}

impl LiveFrameContract {
    fn from_loaded_hashes(c_api: &str, implementation: &str) -> Self {
        if c_api == AUDITED_WINDOWS_C_API_SHA256
            && implementation == AUDITED_WINDOWS_IMPLEMENTATION_SHA256
        {
            Self::AuditedStandaloneV3Push
        } else {
            Self::Unspecified
        }
    }

    fn settled_frames(self, finished: bool, frame_count: u64) -> Option<u64> {
        (finished || self == Self::AuditedStandaloneV3Push).then_some(frame_count)
    }
}

const CPU_DEVICE: i32 = -1;
const MIN_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_SAMPLE_RATE_HZ: u32 = 96_000;
const MAX_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;
const SNAPSHOT_RETRIES: usize = 3;

const STATUS_OK: i32 = 0;

#[repr(C)]
struct NativeModelConfig {
    size: usize,
    model_path: *const c_char,
    gpu: i32,
    preset: *const c_char,
    chunk_frames: i32,
    right_context_frames: i32,
    left_context_frames: i32,
    fifo_frames: i32,
    speaker_cache_frames: i32,
    update_period_frames: i32,
}

#[repr(C)]
struct NativeSegmentationConfig {
    size: usize,
    onset: f32,
    offset: f32,
    pad_onset_seconds: f64,
    pad_offset_seconds: f64,
    min_gap_seconds: f64,
    min_duration_seconds: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeSegment {
    start_seconds: f64,
    end_seconds: f64,
    speaker: i32,
}

type NativeModel = c_void;
type NativeStream = c_void;
type CreateModel = unsafe extern "C" fn(*const NativeModelConfig, *mut *mut NativeModel) -> i32;
type DestroyModel = unsafe extern "C" fn(*mut NativeModel);
type ModelChannelCount = unsafe extern "C" fn(*const NativeModel) -> i32;
type ModelFrameSeconds = unsafe extern "C" fn(*const NativeModel) -> f64;
type OpenStream = unsafe extern "C" fn(*mut NativeModel, *mut *mut NativeStream) -> i32;
type PushF32 = unsafe extern "C" fn(*mut NativeStream, *const f32, usize, i32) -> i32;
type FinishStream = unsafe extern "C" fn(*mut NativeStream) -> i32;
type CloseStream = unsafe extern "C" fn(*mut NativeStream);
type FrameCount = unsafe extern "C" fn(*const NativeStream) -> i64;
type FrameProbabilityStart = unsafe extern "C" fn(*const NativeStream) -> i64;
type CopyFrameProbabilities = unsafe extern "C" fn(*const NativeStream, *mut f32, usize) -> i32;
type CopySegments = unsafe extern "C" fn(
    *const NativeStream,
    *const NativeSegmentationConfig,
    *mut NativeSegment,
    usize,
    *mut usize,
) -> i32;
type LastError = unsafe extern "C" fn() -> *const c_char;
type RuntimeVersion = unsafe extern "C" fn() -> *const c_char;

struct NativeApi {
    // NeMo-Speech owns C++ runtime globals whose teardown is not safe after an
    // explicit FreeLibrary/dlclose. Keep the module pinned for process life;
    // streams and models still receive their documented close/destroy calls.
    _library: ManuallyDrop<Library>,
    live_frame_contract: LiveFrameContract,
    create_model: CreateModel,
    destroy_model: DestroyModel,
    model_channel_count: ModelChannelCount,
    model_frame_seconds: ModelFrameSeconds,
    open_stream: OpenStream,
    push_f32: PushF32,
    finish_stream: FinishStream,
    close_stream: CloseStream,
    frame_count: FrameCount,
    frame_probability_start: FrameProbabilityStart,
    copy_frame_probabilities: CopyFrameProbabilities,
    copy_segments: CopySegments,
    last_error: LastError,
    runtime_version: RuntimeVersion,
}

impl NativeApi {
    fn load(path: &Path) -> Result<Self, NativeDiarizationError> {
        require_absolute_file(path, "native diarization library")?;
        let canonical_path = canonical_file(path, "native diarization library")?;
        // The primary library itself is always loaded by its validated absolute
        // path. We do not modify PATH or any process-wide DLL search setting.
        let library = unsafe { load_exact_library(&canonical_path) }.map_err(|source| {
            NativeDiarizationError::LibraryLoad {
                path: canonical_path.clone(),
                detail: format!("{source:?}: {source}"),
            }
        })?;

        let live_frame_contract = loaded_live_frame_contract(&library);
        unsafe {
            Ok(Self {
                live_frame_contract,
                create_model: load_symbol(&library, b"nemo_speech_diar_create\0")?,
                destroy_model: load_symbol(&library, b"nemo_speech_diar_destroy\0")?,
                model_channel_count: load_symbol(&library, b"nemo_speech_diar_num_speakers\0")?,
                model_frame_seconds: load_symbol(
                    &library,
                    b"nemo_speech_diar_seconds_per_frame\0",
                )?,
                open_stream: load_symbol(&library, b"nemo_speech_diar_stream_open\0")?,
                push_f32: load_symbol(&library, b"nemo_speech_diar_stream_push_f32\0")?,
                finish_stream: load_symbol(&library, b"nemo_speech_diar_stream_finish\0")?,
                close_stream: load_symbol(&library, b"nemo_speech_diar_stream_close\0")?,
                frame_count: load_symbol(&library, b"nemo_speech_diar_frame_count\0")?,
                frame_probability_start: load_symbol(
                    &library,
                    b"nemo_speech_diar_frame_probs_start\0",
                )?,
                copy_frame_probabilities: load_symbol(&library, b"nemo_speech_diar_frame_probs\0")?,
                copy_segments: load_symbol(&library, b"nemo_speech_diar_segments\0")?,
                last_error: load_symbol(&library, b"nemo_speech_asr_last_error\0")?,
                runtime_version: load_symbol(&library, b"nemo_speech_asr_version\0")?,
                _library: ManuallyDrop::new(library),
            })
        }
    }

    fn check_status(
        &self,
        operation: &'static str,
        status: i32,
    ) -> Result<(), NativeDiarizationError> {
        if status == STATUS_OK {
            return Ok(());
        }
        let detail = unsafe { copy_c_string((self.last_error)()) }
            .unwrap_or_else(|| "native runtime returned no error detail".to_string());
        Err(NativeDiarizationError::Native {
            operation,
            status,
            detail,
        })
    }

    fn version(&self) -> Result<String, NativeDiarizationError> {
        unsafe { copy_c_string((self.runtime_version)()) }.ok_or_else(|| {
            NativeDiarizationError::InvalidNativeValue(
                "nemo_speech_asr_version returned null or invalid UTF-8".to_string(),
            )
        })
    }
}

#[cfg(target_os = "windows")]
unsafe fn load_exact_library(path: &Path) -> Result<Library, libloading::Error> {
    use libloading::os::windows::{
        Library as WindowsLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };

    // Matches Python 3.8+'s secure absolute-path CDLL behavior: dependencies
    // may come from the supplied DLL's directory or standard safe locations,
    // without searching or changing PATH.
    let library = unsafe {
        WindowsLibrary::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }?;
    Ok(library.into())
}

#[cfg(not(target_os = "windows"))]
unsafe fn load_exact_library(path: &Path) -> Result<Library, libloading::Error> {
    unsafe { Library::new(path) }
}

#[cfg(target_os = "windows")]
fn loaded_live_frame_contract(library: &Library) -> LiveFrameContract {
    // Inspect the images actually loaded, since Windows may reuse an already
    // loaded dependency with this basename. Hashing a neighboring file alone
    // would incorrectly qualify that different implementation.
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, size: u32) -> u32;
        fn GetCurrentProcess() -> *mut c_void;
        fn K32EnumProcessModules(
            process: *mut c_void,
            modules: *mut *mut c_void,
            size: u32,
            needed: *mut u32,
        ) -> i32;
    }
    unsafe fn loaded_path(module: *mut c_void) -> Option<PathBuf> {
        if module.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32_768];
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
            as usize;
        if len == 0 || len >= buffer.len() {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])))
    }
    let inspect = || -> Option<LiveFrameContract> {
        let symbol = unsafe {
            library
                .get::<PushF32>(b"nemo_speech_diar_stream_push_f32\0")
                .ok()?
        };
        let mut c_module = std::ptr::null_mut();
        // FROM_ADDRESS | UNCHANGED_REFCOUNT. The pinned library owns lifetime.
        let found = unsafe {
            GetModuleHandleExW(0x4 | 0x2, *symbol as *const () as *const u16, &mut c_module)
        };
        if found == 0 {
            return None;
        }
        let c_path = unsafe { loaded_path(c_module)? };
        // A basename lookup alone is ambiguous when two copies were loaded.
        // Qualify only a unique implementation image; a truncated/failed module
        // enumeration cannot manufacture a live commitment contract.
        let mut modules = vec![std::ptr::null_mut(); 2048];
        let bytes = u32::try_from(modules.len() * std::mem::size_of::<*mut c_void>()).ok()?;
        let mut needed = 0;
        let enumerated = unsafe {
            K32EnumProcessModules(
                GetCurrentProcess(),
                modules.as_mut_ptr(),
                bytes,
                &mut needed,
            )
        };
        if enumerated == 0
            || needed > bytes
            || needed as usize % std::mem::size_of::<*mut c_void>() != 0
        {
            return None;
        }
        let mut implementations = Vec::new();
        for module in modules
            .into_iter()
            .take(needed as usize / std::mem::size_of::<*mut c_void>())
        {
            let path = unsafe { loaded_path(module)? };
            if path
                .file_name()?
                .to_string_lossy()
                .eq_ignore_ascii_case("nemo_speech_asr.dll")
            {
                implementations.push(path);
            }
        }
        if implementations.len() != 1 {
            return None;
        }
        Some(LiveFrameContract::from_loaded_hashes(
            &file_sha256(&c_path).ok()?,
            &file_sha256(&implementations[0]).ok()?,
        ))
    };
    inspect().unwrap_or(LiveFrameContract::Unspecified)
}

#[cfg(not(target_os = "windows"))]
fn loaded_live_frame_contract(_library: &Library) -> LiveFrameContract {
    LiveFrameContract::Unspecified
}

unsafe fn load_symbol<T: Copy>(
    library: &Library,
    name: &'static [u8],
) -> Result<T, NativeDiarizationError> {
    let symbol = unsafe { library.get::<T>(name) }.map_err(|source| {
        NativeDiarizationError::MissingSymbol {
            symbol: String::from_utf8_lossy(name)
                .trim_end_matches('\0')
                .to_string(),
            detail: source.to_string(),
        }
    })?;
    Ok(*symbol)
}

unsafe fn copy_c_string(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

struct ModelInner {
    api: Arc<NativeApi>,
    handle: *mut NativeModel,
}

// The native contract permits independent streams on different threads and
// serializes shared model compute internally. The opaque model is never exposed.
unsafe impl Send for ModelInner {}
unsafe impl Sync for ModelInner {}

impl Drop for ModelInner {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.api.destroy_model)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Loaded Nemotron V3 model. Cloning shares the model, not stream state.
#[derive(Clone)]
pub(crate) struct NemotronDiarizer {
    inner: Arc<ModelInner>,
    runtime_version: Arc<str>,
    model_path: Arc<PathBuf>,
}

impl NemotronDiarizer {
    /// Loads exactly the caller-supplied library and official Q8 model.
    ///
    /// This performs no download, install, PATH mutation, or model discovery.
    pub(crate) fn load(
        native_library_path: impl AsRef<Path>,
        model_path: impl AsRef<Path>,
    ) -> Result<Self, NativeDiarizationError> {
        let native_library_path = native_library_path.as_ref();
        let requested_model_path = model_path.as_ref();
        require_absolute_file(requested_model_path, "Nemotron V3 model")?;
        let model_path = canonical_file(requested_model_path, "Nemotron V3 model")?;
        verify_official_model(&model_path)?;

        let api = Arc::new(NativeApi::load(native_library_path)?);
        let runtime_version: Arc<str> = api.version()?.into();
        let model_path_text = path_c_string(&model_path, "Nemotron V3 model")?;
        let preset = CString::new("v3-streaming").expect("literal contains no NUL");
        let config = NativeModelConfig {
            size: std::mem::size_of::<NativeModelConfig>(),
            model_path: model_path_text.as_ptr(),
            gpu: CPU_DEVICE,
            preset: preset.as_ptr(),
            chunk_frames: 13,
            right_context_frames: 1,
            left_context_frames: -1,
            fifo_frames: 80,
            speaker_cache_frames: 264,
            update_period_frames: 40,
        };
        let mut handle = std::ptr::null_mut();
        let status = unsafe { (api.create_model)(&config, &mut handle) };
        api.check_status("nemo_speech_diar_create", status)?;
        if handle.is_null() {
            return Err(NativeDiarizationError::InvalidNativeValue(
                "model create succeeded with a null handle".to_string(),
            ));
        }

        let channels = unsafe { (api.model_channel_count)(handle) };
        let frame_seconds = unsafe { (api.model_frame_seconds)(handle) };
        if channels != NEMOTRON_V3_CHANNELS as i32
            || !frame_seconds.is_finite()
            || (frame_seconds - NEMOTRON_V3_FRAME_SECONDS).abs() > 1e-6
        {
            unsafe { (api.destroy_model)(handle) };
            return Err(NativeDiarizationError::UnexpectedModelGeometry {
                channels,
                frame_seconds,
            });
        }

        Ok(Self {
            inner: Arc::new(ModelInner { api, handle }),
            runtime_version,
            model_path: Arc::new(model_path),
        })
    }

    pub(crate) fn runtime_version(&self) -> &str {
        &self.runtime_version
    }

    pub(crate) fn model_path(&self) -> &Path {
        self.model_path.as_path()
    }

    pub(crate) fn open_stream(
        &self,
        sample_rate_hz: u32,
    ) -> Result<AnonymousDiarizationStream, NativeDiarizationError> {
        validate_sample_rate(sample_rate_hz)?;
        let handle = open_native_stream(&self.inner)?;
        Ok(AnonymousDiarizationStream {
            model: Arc::clone(&self.inner),
            handle,
            sample_rate_hz,
            received_samples: 0,
            finished: false,
        })
    }
}

fn open_native_stream(
    model: &Arc<ModelInner>,
) -> Result<*mut NativeStream, NativeDiarizationError> {
    let mut stream = std::ptr::null_mut();
    let status = unsafe { (model.api.open_stream)(model.handle, &mut stream) };
    model
        .api
        .check_status("nemo_speech_diar_stream_open", status)?;
    if stream.is_null() {
        return Err(NativeDiarizationError::InvalidNativeValue(
            "stream open succeeded with a null handle".to_string(),
        ));
    }
    Ok(stream)
}

/// A 1-based anonymous channel assigned within one native stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AnonymousSpeakerChannel(u8);

impl AnonymousSpeakerChannel {
    fn from_native(value: i32) -> Result<Self, NativeDiarizationError> {
        if !(1..=NEMOTRON_V3_CHANNELS as i32).contains(&value) {
            return Err(NativeDiarizationError::InvalidNativeValue(format!(
                "native speaker channel {value} is outside 1..={NEMOTRON_V3_CHANNELS}"
            )));
        }
        Ok(Self(value as u8))
    }

    pub(crate) fn one_based(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnonymousSpeakerSegment {
    pub(crate) start_seconds: f64,
    pub(crate) end_seconds: f64,
    pub(crate) channel: AnonymousSpeakerChannel,
}

/// Native channel activity before display padding, gap filling, and short-span
/// removal. This is still anonymous evidence, not an enrolled identity.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnonymousCoreActivity {
    pub(crate) start_seconds: f64,
    pub(crate) end_seconds: f64,
    pub(crate) channel: AnonymousSpeakerChannel,
}

/// Exact unpadded activity in the caller's 16 kHz source sample clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AnonymousCoreSampleActivity {
    pub(crate) start_sample: u64,
    pub(crate) end_sample: u64,
    pub(crate) channel: AnonymousSpeakerChannel,
}

struct AnonymousCoreFrameActivity {
    start_frame: u64,
    end_frame: u64,
    channel: AnonymousSpeakerChannel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AudioWatermark {
    pub(crate) end_sample: u64,
    pub(crate) end_seconds: f64,
}

/// Coherent copy of the native stream result at one serialized observation.
#[derive(Clone, Debug)]
pub(crate) struct AnonymousDiarizationSnapshot {
    pub(crate) input: AudioWatermark,
    pub(crate) finished: bool,
    pub(crate) first_retained_frame: u64,
    pub(crate) frame_count: u64,
    /// None means the ABI/runtime has not proved a stable live frontier.
    /// A matching audited implementation, or successful finish, supplies it.
    pub(crate) settled_frame_count: Option<u64>,
    pub(crate) frame_seconds: f64,
    /// Frame-major probabilities, eight values per retained frame.
    pub(crate) frame_probabilities: Vec<f32>,
    /// Presentation spans include the SDK's 229 ms onset / 79 ms offset pads.
    /// Never use their expanded edges as hard source-identity evidence.
    pub(crate) segments: Vec<AnonymousSpeakerSegment>,
}

impl AnonymousDiarizationSnapshot {
    /// The one source-time frontier used by both activity and channel binding.
    /// Missing live commitment stays None; missing compacted history is an
    /// error until the consumer supplies a carried ledger. Finish padding is
    /// never evidence beyond audio actually received.
    pub(crate) fn settled_through_seconds(&self) -> Result<Option<f64>, NativeDiarizationError> {
        if self.first_retained_frame != 0 {
            return Err(NativeDiarizationError::InvalidInput(
                "stable activity requires uncompacted history or a carried activity ledger".into(),
            ));
        }
        if !self.frame_seconds.is_finite()
            || self.frame_seconds <= 0.0
            || !self.input.end_seconds.is_finite()
            || self.input.end_seconds < 0.0
        {
            return Err(NativeDiarizationError::InvalidNativeValue(
                "invalid stable activity clock".into(),
            ));
        }
        let Some(settled) = self.settled_frame_count else {
            return Ok(None);
        };
        if settled > self.frame_count {
            return Err(NativeDiarizationError::InvalidNativeValue(
                "stable frontier exceeds available frames".into(),
            ));
        }
        Ok(Some(
            (settled as f64 * self.frame_seconds).min(self.input.end_seconds),
        ))
    }

    pub(crate) fn settled_through_ms(&self) -> Result<Option<u64>, NativeDiarizationError> {
        Ok(self
            .settled_through_seconds()?
            .map(|seconds| (seconds * 1000.0).floor() as u64))
    }

    /// Fixed V3 10 ms frames represent exactly160 samples at16 kHz. Derive
    /// this frontier from frame/sample integers, not rounded seconds. Unknown
    /// live commitment remains None; finish padding stops at the last sample.
    pub(crate) fn settled_through_samples_16k(
        &self,
    ) -> Result<Option<u64>, NativeDiarizationError> {
        self.settled_through_seconds()?;
        if self.frame_seconds != NEMOTRON_V3_FRAME_SECONDS
            || self.input.end_seconds != self.input.end_sample as f64 / 16_000.0
        {
            return Err(NativeDiarizationError::InvalidInput(
                "exact source samples require the fixed V3 frame clock and16kHz input".into(),
            ));
        }
        self.settled_frame_count
            .map(|frames| {
                frames
                    .checked_mul(160)
                    .map(|samples| samples.min(self.input.end_sample))
                    .ok_or_else(|| {
                        NativeDiarizationError::InvalidNativeValue(
                            "stable frame sample coordinate overflow".into(),
                        )
                    })
            })
            .transpose()
    }

    /// Reproduce the first stage of the pinned v0.2.0 postprocessor only.
    /// Its official onset/offset values are unchanged; presentation collars and
    /// filled gaps are deliberately not promoted into acoustic evidence.
    /// A compacted snapshot needs the caller's prior hysteresis/identity ledger;
    /// this standalone helper fails rather than inventing that missing history.
    pub(crate) fn core_activity(
        &self,
    ) -> Result<Vec<AnonymousCoreActivity>, NativeDiarizationError> {
        let through = self.settled_through_seconds()?.ok_or_else(|| {
            NativeDiarizationError::InvalidInput(
                "live raw frames have no audited stable frontier".into(),
            )
        })?;
        Ok(self
            .core_frame_activity()?
            .into_iter()
            .filter_map(|core| {
                let start_seconds = core.start_frame as f64 * self.frame_seconds;
                let end_seconds = (core.end_frame as f64 * self.frame_seconds).min(through);
                (start_seconds < end_seconds).then_some(AnonymousCoreActivity {
                    start_seconds,
                    end_seconds,
                    channel: core.channel,
                })
            })
            .collect())
    }

    pub(crate) fn core_activity_samples_16k(
        &self,
    ) -> Result<Vec<AnonymousCoreSampleActivity>, NativeDiarizationError> {
        let through = self.settled_through_samples_16k()?.ok_or_else(|| {
            NativeDiarizationError::InvalidInput(
                "live raw frames have no audited stable sample frontier".into(),
            )
        })?;
        let coordinate = |frame: u64| {
            frame.checked_mul(160).ok_or_else(|| {
                NativeDiarizationError::InvalidNativeValue("core sample coordinate overflow".into())
            })
        };
        let mut activity = Vec::new();
        for core in self.core_frame_activity()? {
            let start_sample = coordinate(core.start_frame)?;
            let end_sample = coordinate(core.end_frame)?.min(through);
            if start_sample < end_sample {
                activity.push(AnonymousCoreSampleActivity {
                    start_sample,
                    end_sample,
                    channel: core.channel,
                });
            }
        }
        Ok(activity)
    }

    fn core_frame_activity(
        &self,
    ) -> Result<Vec<AnonymousCoreFrameActivity>, NativeDiarizationError> {
        const ONSET: f32 = 0.641;
        const OFFSET: f32 = 0.561;
        self.settled_through_seconds()?.ok_or_else(|| {
            NativeDiarizationError::InvalidInput(
                "live raw frames have no audited stable frontier".into(),
            )
        })?;
        let settled = self.settled_frame_count.expect("validated stable frontier");
        let frames = usize::try_from(settled).map_err(|_| {
            NativeDiarizationError::InvalidNativeValue("frame count exceeds address space".into())
        })?;
        if usize::try_from(self.frame_count)
            .ok()
            .and_then(|count| count.checked_mul(NEMOTRON_V3_CHANNELS))
            != Some(self.frame_probabilities.len())
            || !self.frame_seconds.is_finite()
            || self.frame_seconds <= 0.0
            || !self.input.end_seconds.is_finite()
            || self.input.end_seconds < 0.0
            || self
                .frame_probabilities
                .iter()
                .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
        {
            return Err(NativeDiarizationError::InvalidNativeValue(
                "invalid activity probability timeline".into(),
            ));
        }
        let mut activity = Vec::new();
        for index in 0..NEMOTRON_V3_CHANNELS {
            let channel = AnonymousSpeakerChannel::from_native(index as i32 + 1)?;
            let mut start = None;
            for frame in 0..frames {
                let probability = self.frame_probabilities[frame * NEMOTRON_V3_CHANNELS + index];
                if start.is_none() && probability > ONSET {
                    start = Some(frame);
                } else if start.is_some() && probability < OFFSET {
                    let first = start.take().unwrap();
                    activity.push(AnonymousCoreFrameActivity {
                        start_frame: first as u64,
                        end_frame: frame as u64,
                        channel,
                    });
                }
            }
            if let Some(first) = start {
                activity.push(AnonymousCoreFrameActivity {
                    start_frame: first as u64,
                    end_frame: settled,
                    channel,
                });
            }
        }
        activity.sort_by(|a, b| {
            a.start_frame
                .cmp(&b.start_frame)
                .then_with(|| a.channel.one_based().cmp(&b.channel.one_based()))
        });
        Ok(activity)
    }

    pub(crate) fn retained_frame_count(&self) -> u64 {
        self.frame_count - self.first_retained_frame
    }

    pub(crate) fn probability(
        &self,
        absolute_frame: u64,
        channel: AnonymousSpeakerChannel,
    ) -> Option<f32> {
        let relative = absolute_frame.checked_sub(self.first_retained_frame)?;
        if relative >= self.retained_frame_count() {
            return None;
        }
        let index = usize::try_from(relative)
            .ok()?
            .checked_mul(NEMOTRON_V3_CHANNELS)?
            .checked_add(usize::from(channel.one_based() - 1))?;
        self.frame_probabilities.get(index).copied()
    }
}

/// Single-threaded stream state. Moving it between threads is safe, but every
/// operation requires exclusive access and channel numbers reset with the stream.
pub(crate) struct AnonymousDiarizationStream {
    model: Arc<ModelInner>,
    handle: *mut NativeStream,
    sample_rate_hz: u32,
    received_samples: u64,
    finished: bool,
}

unsafe impl Send for AnonymousDiarizationStream {}

impl AnonymousDiarizationStream {
    pub(crate) fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub(crate) fn input_watermark(&self) -> AudioWatermark {
        AudioWatermark {
            end_sample: self.received_samples,
            end_seconds: self.received_samples as f64 / self.sample_rate_hz as f64,
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn push_f32(
        &mut self,
        samples: &[f32],
    ) -> Result<AudioWatermark, NativeDiarizationError> {
        if self.finished {
            return Err(NativeDiarizationError::InvalidInput(
                "cannot push audio after stream finish".to_string(),
            ));
        }
        if samples.is_empty() {
            return Ok(self.input_watermark());
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(NativeDiarizationError::InvalidInput(
                "audio contains a non-finite sample".to_string(),
            ));
        }
        let next_samples = self
            .received_samples
            .checked_add(samples.len() as u64)
            .ok_or_else(|| {
                NativeDiarizationError::InvalidInput("audio sample clock overflow".to_string())
            })?;
        let status = unsafe {
            (self.model.api.push_f32)(
                self.handle,
                samples.as_ptr(),
                samples.len(),
                self.sample_rate_hz as i32,
            )
        };
        self.model
            .api
            .check_status("nemo_speech_diar_stream_push_f32", status)?;
        self.received_samples = next_samples;
        Ok(self.input_watermark())
    }

    pub(crate) fn push_pcm16_le(
        &mut self,
        pcm: &[u8],
    ) -> Result<AudioWatermark, NativeDiarizationError> {
        if pcm.len() % 2 != 0 {
            return Err(NativeDiarizationError::InvalidInput(
                "PCM16-LE byte count must be even".to_string(),
            ));
        }
        let samples = pcm
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
            .collect::<Vec<_>>();
        self.push_f32(&samples)
    }

    /// Closes the old native stream and starts a fresh anonymous timeline.
    /// A failed open leaves the existing stream and sample clock untouched.
    pub(crate) fn reset(&mut self) -> Result<(), NativeDiarizationError> {
        let replacement = open_native_stream(&self.model)?;
        unsafe { (self.model.api.close_stream)(self.handle) };
        self.handle = replacement;
        self.received_samples = 0;
        self.finished = false;
        Ok(())
    }

    pub(crate) fn snapshot(
        &mut self,
    ) -> Result<AnonymousDiarizationSnapshot, NativeDiarizationError> {
        for _ in 0..SNAPSHOT_RETRIES {
            let before = self.native_frame_range()?;
            let probabilities = self.copy_probabilities(before)?;
            let segments = self.copy_segments()?;
            let after = self.native_frame_range()?;
            if before == after {
                return Ok(AnonymousDiarizationSnapshot {
                    input: self.input_watermark(),
                    finished: self.finished,
                    first_retained_frame: before.0,
                    frame_count: before.1,
                    settled_frame_count: self
                        .model
                        .api
                        .live_frame_contract
                        .settled_frames(self.finished, before.1),
                    frame_seconds: NEMOTRON_V3_FRAME_SECONDS,
                    frame_probabilities: probabilities,
                    segments,
                });
            }
        }
        Err(NativeDiarizationError::InvalidNativeValue(
            "native frame range changed repeatedly while copying a snapshot".to_string(),
        ))
    }

    pub(crate) fn finish(
        &mut self,
    ) -> Result<AnonymousDiarizationSnapshot, NativeDiarizationError> {
        if !self.finished {
            let status = unsafe { (self.model.api.finish_stream)(self.handle) };
            self.model
                .api
                .check_status("nemo_speech_diar_stream_finish", status)?;
            self.finished = true;
        }
        self.snapshot()
    }

    fn native_frame_range(&self) -> Result<(u64, u64), NativeDiarizationError> {
        let count = unsafe { (self.model.api.frame_count)(self.handle) };
        let start = unsafe { (self.model.api.frame_probability_start)(self.handle) };
        if count < 0 || start < 0 || start > count {
            return Err(NativeDiarizationError::InvalidNativeValue(format!(
                "invalid native frame range start={start}, count={count}"
            )));
        }
        Ok((start as u64, count as u64))
    }

    fn copy_probabilities(&self, frames: (u64, u64)) -> Result<Vec<f32>, NativeDiarizationError> {
        let retained = frames.1 - frames.0;
        let float_count = usize::try_from(retained)
            .ok()
            .and_then(|value| value.checked_mul(NEMOTRON_V3_CHANNELS))
            .ok_or_else(|| {
                NativeDiarizationError::InvalidNativeValue(
                    "native probability dimensions overflow usize".to_string(),
                )
            })?;
        enforce_snapshot_size(float_count, std::mem::size_of::<f32>(), "probabilities")?;
        let mut values = Vec::new();
        values.try_reserve_exact(float_count).map_err(|_| {
            NativeDiarizationError::SnapshotTooLarge {
                kind: "probabilities",
                elements: float_count,
            }
        })?;
        values.resize(float_count, 0.0);
        let status = unsafe {
            (self.model.api.copy_frame_probabilities)(
                self.handle,
                values.as_mut_ptr(),
                values.len(),
            )
        };
        self.model
            .api
            .check_status("nemo_speech_diar_frame_probs", status)?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err(NativeDiarizationError::InvalidNativeValue(
                "native probabilities contain a non-finite value".to_string(),
            ));
        }
        Ok(values)
    }

    fn copy_segments(&self) -> Result<Vec<AnonymousSpeakerSegment>, NativeDiarizationError> {
        for _ in 0..SNAPSHOT_RETRIES {
            let mut count = 0usize;
            let status = unsafe {
                (self.model.api.copy_segments)(
                    self.handle,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                    &mut count,
                )
            };
            self.model
                .api
                .check_status("nemo_speech_diar_segments(count)", status)?;
            enforce_snapshot_size(count, std::mem::size_of::<NativeSegment>(), "segments")?;

            let mut native = Vec::new();
            native.try_reserve_exact(count).map_err(|_| {
                NativeDiarizationError::SnapshotTooLarge {
                    kind: "segments",
                    elements: count,
                }
            })?;
            native.resize(
                count,
                NativeSegment {
                    start_seconds: 0.0,
                    end_seconds: 0.0,
                    speaker: 0,
                },
            );
            let mut written = count;
            let status = unsafe {
                (self.model.api.copy_segments)(
                    self.handle,
                    std::ptr::null(),
                    native.as_mut_ptr(),
                    native.len(),
                    &mut written,
                )
            };
            if status != STATUS_OK {
                // A live result may grow between the count and copy calls. Retry
                // only when the ABI reports a larger required count.
                if written > count {
                    continue;
                }
                self.model
                    .api
                    .check_status("nemo_speech_diar_segments(copy)", status)?;
            }
            if written > native.len() {
                continue;
            }
            native.truncate(written);

            let mut segments = Vec::with_capacity(native.len());
            let mut previous_start = 0.0f64;
            for (index, value) in native.into_iter().enumerate() {
                if !value.start_seconds.is_finite()
                    || !value.end_seconds.is_finite()
                    || value.start_seconds < 0.0
                    || value.end_seconds < value.start_seconds
                    || (index > 0 && value.start_seconds < previous_start)
                {
                    return Err(NativeDiarizationError::InvalidNativeValue(format!(
                        "invalid or unsorted native segment at index {index}: {}..{}",
                        value.start_seconds, value.end_seconds
                    )));
                }
                previous_start = value.start_seconds;
                segments.push(AnonymousSpeakerSegment {
                    start_seconds: value.start_seconds,
                    end_seconds: value.end_seconds,
                    channel: AnonymousSpeakerChannel::from_native(value.speaker)?,
                });
            }
            return Ok(segments);
        }
        Err(NativeDiarizationError::InvalidNativeValue(
            "native segment count changed repeatedly while copying a snapshot".to_string(),
        ))
    }
}

impl Drop for AnonymousDiarizationStream {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.model.api.close_stream)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

fn validate_sample_rate(sample_rate_hz: u32) -> Result<(), NativeDiarizationError> {
    if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz) {
        return Err(NativeDiarizationError::InvalidInput(format!(
            "sample rate {sample_rate_hz} Hz is outside {MIN_SAMPLE_RATE_HZ}..={MAX_SAMPLE_RATE_HZ}"
        )));
    }
    Ok(())
}

fn enforce_snapshot_size(
    elements: usize,
    element_bytes: usize,
    kind: &'static str,
) -> Result<(), NativeDiarizationError> {
    let bytes = elements
        .checked_mul(element_bytes)
        .ok_or(NativeDiarizationError::SnapshotTooLarge { kind, elements })?;
    if bytes > MAX_SNAPSHOT_BYTES {
        return Err(NativeDiarizationError::SnapshotTooLarge { kind, elements });
    }
    Ok(())
}

fn require_absolute_file(path: &Path, role: &'static str) -> Result<(), NativeDiarizationError> {
    if !path.is_absolute() {
        return Err(NativeDiarizationError::InvalidPath {
            role,
            path: path.to_path_buf(),
            detail: "path must be absolute; lookup through the process PATH is disabled",
        });
    }
    if !path.is_file() {
        return Err(NativeDiarizationError::InvalidPath {
            role,
            path: path.to_path_buf(),
            detail: "file does not exist",
        });
    }
    Ok(())
}

fn canonical_file(path: &Path, role: &'static str) -> Result<PathBuf, NativeDiarizationError> {
    std::fs::canonicalize(path).map_err(|_| NativeDiarizationError::InvalidPath {
        role,
        path: path.to_path_buf(),
        detail: "file cannot be resolved to a canonical absolute path",
    })
}

fn path_c_string(path: &Path, role: &'static str) -> Result<CString, NativeDiarizationError> {
    let text = path
        .to_str()
        .ok_or_else(|| NativeDiarizationError::InvalidPath {
            role,
            path: path.to_path_buf(),
            detail: "path is not valid UTF-8 for the native C ABI",
        })?;
    CString::new(text).map_err(|_| NativeDiarizationError::InvalidPath {
        role,
        path: path.to_path_buf(),
        detail: "path contains an embedded NUL",
    })
}

fn verify_official_model(path: &Path) -> Result<(), NativeDiarizationError> {
    let actual = file_sha256(path)?;
    if actual != NEMOTRON_V3_Q8_SHA256 {
        return Err(NativeDiarizationError::ModelHashMismatch {
            path: path.to_path_buf(),
            expected: NEMOTRON_V3_Q8_SHA256,
            actual,
        });
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, NativeDiarizationError> {
    let file = File::open(path).map_err(|source| NativeDiarizationError::FileRead {
        path: path.to_path_buf(),
        detail: source.to_string(),
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // Keep the one-megabyte hashing buffer off Windows' small main-thread stack.
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| NativeDiarizationError::FileRead {
                path: path.to_path_buf(),
                detail: source.to_string(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug)]
pub(crate) enum NativeDiarizationError {
    InvalidPath {
        role: &'static str,
        path: PathBuf,
        detail: &'static str,
    },
    FileRead {
        path: PathBuf,
        detail: String,
    },
    ModelHashMismatch {
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
    LibraryLoad {
        path: PathBuf,
        detail: String,
    },
    MissingSymbol {
        symbol: String,
        detail: String,
    },
    Native {
        operation: &'static str,
        status: i32,
        detail: String,
    },
    UnexpectedModelGeometry {
        channels: i32,
        frame_seconds: f64,
    },
    SnapshotTooLarge {
        kind: &'static str,
        elements: usize,
    },
    InvalidNativeValue(String),
    InvalidInput(String),
}

impl fmt::Display for NativeDiarizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath {
                role,
                path,
                detail,
            } => write!(formatter, "invalid {role} path {}: {detail}", path.display()),
            Self::FileRead { path, detail } => {
                write!(formatter, "cannot read {}: {detail}", path.display())
            }
            Self::ModelHashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "Nemotron V3 model hash mismatch for {}: expected {expected}, got {actual}",
                path.display()
            ),
            Self::LibraryLoad { path, detail } => write!(
                formatter,
                "cannot load native diarization library {}: {detail}",
                path.display()
            ),
            Self::MissingSymbol { symbol, detail } => {
                write!(formatter, "native diarization symbol {symbol} is unavailable: {detail}")
            }
            Self::Native {
                operation,
                status,
                detail,
            } => write!(
                formatter,
                "native diarization call {operation} failed with status {status}: {detail}"
            ),
            Self::UnexpectedModelGeometry {
                channels,
                frame_seconds,
            } => write!(
                formatter,
                "unexpected diarization model geometry: channels={channels}, frame_seconds={frame_seconds} (expected 8 and 0.01)"
            ),
            Self::SnapshotTooLarge { kind, elements } => write!(
                formatter,
                "native diarization {kind} snapshot is too large ({elements} elements)"
            ),
            Self::InvalidNativeValue(detail) => {
                write!(formatter, "invalid native diarization result: {detail}")
            }
            Self::InvalidInput(detail) => write!(formatter, "invalid diarization input: {detail}"),
        }
    }
}

impl Error for NativeDiarizationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_both_audited_loaded_images_certify_the_live_frame_frontier() {
        let trusted = LiveFrameContract::from_loaded_hashes(
            AUDITED_WINDOWS_C_API_SHA256,
            AUDITED_WINDOWS_IMPLEMENTATION_SHA256,
        );
        assert_eq!(trusted.settled_frames(false, 208), Some(208));
        for contract in [
            LiveFrameContract::from_loaded_hashes(
                "other C ABI",
                AUDITED_WINDOWS_IMPLEMENTATION_SHA256,
            ),
            LiveFrameContract::from_loaded_hashes(
                AUDITED_WINDOWS_C_API_SHA256,
                "other implementation",
            ),
        ] {
            assert_eq!(contract.settled_frames(false, 208), None);
            assert_eq!(contract.settled_frames(true, 208), Some(208));
        }
    }

    #[test]
    fn provisional_frames_cannot_supply_live_owner_activity() {
        let mut snapshot = activity_fixture(&[0.0, 0.0, 0.9, 0.9]);
        snapshot.settled_frame_count = None;
        assert_eq!(snapshot.settled_through_ms().unwrap(), None);
        assert!(snapshot.core_activity().is_err());
        snapshot.settled_frame_count = Some(2);
        assert_eq!(snapshot.settled_through_ms().unwrap(), Some(20));
        assert!(snapshot.core_activity().unwrap().is_empty());
        snapshot.settled_frame_count = Some(5);
        assert!(snapshot.core_activity().is_err());
        snapshot.finished = true;
        snapshot.settled_frame_count = Some(4);
        assert_eq!(snapshot.core_activity().unwrap()[0].start_seconds, 0.02);
    }

    #[test]
    fn finished_padding_never_extends_activity_past_received_audio() {
        let mut snapshot = activity_fixture(&[0.9, 0.9]);
        snapshot.finished = true;
        snapshot.input.end_sample = 240;
        snapshot.input.end_seconds = 0.015;
        assert_eq!(snapshot.settled_through_ms().unwrap(), Some(15));
        let core = snapshot.core_activity().unwrap();
        assert_eq!(core.len(), 1);
        assert_eq!(core[0].end_seconds, 0.015);
    }

    #[test]
    fn exact_sample_activity_preserves_a_partial_ms_finish_boundary() {
        let mut snapshot = activity_fixture(&[0.0, 0.9, 0.9]);
        snapshot.finished = true;
        snapshot.input.end_sample = 257;
        snapshot.input.end_seconds = 257.0 / 16_000.0;
        assert_eq!(snapshot.settled_through_samples_16k().unwrap(), Some(257));
        let core = snapshot.core_activity_samples_16k().unwrap();
        assert_eq!(core.len(), 1);
        assert_eq!((core[0].start_sample, core[0].end_sample), (160, 257));
        assert_eq!(
            snapshot.core_activity().unwrap()[0].end_seconds,
            257.0 / 16_000.0
        );
    }

    #[test]
    fn exact_samples_cannot_adopt_unknown_or_inconsistent_native_clocks() {
        let mut snapshot = activity_fixture(&[0.0, 0.9]);
        snapshot.settled_frame_count = None;
        assert_eq!(snapshot.settled_through_samples_16k().unwrap(), None);
        assert!(snapshot.core_activity_samples_16k().is_err());
        snapshot.settled_frame_count = Some(1);
        assert_eq!(snapshot.settled_through_samples_16k().unwrap(), Some(160));
        assert!(snapshot.core_activity_samples_16k().unwrap().is_empty());
        snapshot.input.end_seconds = snapshot.input.end_sample as f64 / 8_000.0;
        assert!(snapshot.settled_through_samples_16k().is_err());
        snapshot.input.end_seconds = snapshot.input.end_sample as f64 / 16_000.0;
        snapshot.frame_seconds = 0.02;
        assert!(snapshot.core_activity_samples_16k().is_err());
    }

    #[test]
    fn exact_sample_frontier_rejects_frame_coordinate_overflow() {
        let mut snapshot = activity_fixture(&[]);
        snapshot.frame_count = u64::MAX;
        snapshot.settled_frame_count = Some(u64::MAX);
        snapshot.input.end_sample = u64::MAX;
        snapshot.input.end_seconds = u64::MAX as f64 / 16_000.0;
        assert!(snapshot.settled_through_samples_16k().is_err());
    }

    fn activity_fixture(channel_one: &[f32]) -> AnonymousDiarizationSnapshot {
        let mut frame_probabilities = vec![0.0; channel_one.len() * NEMOTRON_V3_CHANNELS];
        for (frame, probability) in channel_one.iter().enumerate() {
            frame_probabilities[frame * NEMOTRON_V3_CHANNELS] = *probability;
        }
        AnonymousDiarizationSnapshot {
            input: AudioWatermark {
                end_sample: channel_one.len() as u64 * 160,
                end_seconds: (channel_one.len() as u64 * 160) as f64 / 16_000.0,
            },
            finished: false,
            first_retained_frame: 0,
            frame_count: channel_one.len() as u64,
            settled_frame_count: Some(channel_one.len() as u64),
            frame_seconds: 0.01,
            frame_probabilities,
            segments: vec![AnonymousSpeakerSegment {
                start_seconds: 0.0,
                end_seconds: 0.2,
                channel: AnonymousSpeakerChannel::from_native(1).unwrap(),
            }],
        }
    }

    #[test]
    fn display_collar_cannot_extend_identity_activity_across_a_speaker_boundary() {
        let snapshot = activity_fixture(&[0.0, 0.641, 0.7, 0.6, 0.561, 0.55, 0.0]);
        let core = snapshot.core_activity().unwrap();
        assert_eq!(core.len(), 1);
        assert_eq!(core[0].start_seconds, 0.02);
        assert_eq!(core[0].end_seconds, 0.05);
        assert!(snapshot.segments[0].end_seconds > core[0].end_seconds);
    }

    #[test]
    fn core_activity_preserves_simultaneous_channels_and_unobserved_gaps() {
        let mut snapshot = activity_fixture(&[0.7, 0.0, 0.7, 0.7]);
        snapshot.frame_probabilities[2 * NEMOTRON_V3_CHANNELS + 1] = 0.8;
        snapshot.frame_probabilities[3 * NEMOTRON_V3_CHANNELS + 1] = 0.8;
        let core = snapshot.core_activity().unwrap();
        assert_eq!(core.len(), 3);
        assert_eq!((core[0].start_seconds, core[0].end_seconds), (0.0, 0.01));
        assert_eq!((core[1].start_seconds, core[1].end_seconds), (0.02, 0.04));
        assert_eq!((core[2].start_seconds, core[2].end_seconds), (0.02, 0.04));
        assert_ne!(core[1].channel, core[2].channel);
    }

    #[test]
    fn missing_history_and_invalid_probabilities_cannot_manufacture_activity() {
        let mut snapshot = activity_fixture(&[0.7]);
        snapshot.first_retained_frame = 1;
        assert!(snapshot.settled_through_ms().is_err());
        assert!(snapshot.core_activity().is_err());
        snapshot.first_retained_frame = 0;
        snapshot.frame_probabilities[0] = f32::NAN;
        assert!(snapshot.core_activity().is_err());
    }

    #[test]
    fn ffi_layout_matches_the_64_bit_c_header() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(std::mem::size_of::<NativeModelConfig>(), 56);
            assert_eq!(std::mem::size_of::<NativeSegmentationConfig>(), 48);
            assert_eq!(std::mem::size_of::<NativeSegment>(), 24);
        }
    }

    #[test]
    fn channel_and_sample_rate_boundaries_are_explicit() {
        assert!(AnonymousSpeakerChannel::from_native(0).is_err());
        assert_eq!(
            AnonymousSpeakerChannel::from_native(8).unwrap().one_based(),
            8
        );
        assert!(AnonymousSpeakerChannel::from_native(9).is_err());
        assert!(validate_sample_rate(7_999).is_err());
        assert!(validate_sample_rate(16_000).is_ok());
        assert!(validate_sample_rate(96_001).is_err());
    }

    #[test]
    fn absolute_frame_probability_index_respects_compaction_origin() {
        let snapshot = AnonymousDiarizationSnapshot {
            input: AudioWatermark {
                end_sample: 160,
                end_seconds: 0.01,
            },
            finished: false,
            first_retained_frame: 10,
            frame_count: 12,
            settled_frame_count: None,
            frame_seconds: NEMOTRON_V3_FRAME_SECONDS,
            frame_probabilities: (0..16).map(|value| value as f32).collect(),
            segments: Vec::new(),
        };
        let channel = AnonymousSpeakerChannel::from_native(2).unwrap();
        assert_eq!(snapshot.probability(10, channel), Some(1.0));
        assert_eq!(snapshot.probability(11, channel), Some(9.0));
        assert_eq!(snapshot.probability(9, channel), None);
        assert_eq!(snapshot.probability(12, channel), None);
    }

    /// Set the three paths explicitly; the wrapper never discovers or downloads
    /// native assets. This smoke verifies ABI layout and lifecycle on real PCM.
    #[test]
    #[ignore = "requires official local native runtime, Q8 model, and PCM fixture"]
    fn official_runtime_stream_smoke() {
        let library = std::env::var_os("LISTENER_NEMO_DIAR_LIBRARY").unwrap();
        let model = std::env::var_os("LISTENER_NEMOTRON_V3_MODEL").unwrap();
        let pcm_path = std::env::var_os("LISTENER_NEMO_DIAR_PCM16").unwrap();
        let pcm = std::fs::read(pcm_path).unwrap();

        let diarizer =
            NemotronDiarizer::load(PathBuf::from(library), PathBuf::from(model)).unwrap();
        let mut stream = diarizer.open_stream(16_000).unwrap();
        let mut committed = Vec::new();
        let mut first_live_frontier = None;
        for chunk in pcm.chunks(2 * 2_560) {
            stream.push_pcm16_le(chunk).unwrap();
            let live = stream.snapshot().unwrap();
            if cfg!(target_os = "windows") {
                assert_eq!(
                    live.settled_frame_count,
                    Some(live.frame_count),
                    "official Windows smoke requires the exact audited loaded runtime"
                );
                assert_eq!(
                    &live.frame_probabilities[..committed.len()],
                    &committed,
                    "a previously committed raw frame must not be rewritten"
                );
                if live.frame_count > 0 {
                    first_live_frontier.get_or_insert((live.input.end_seconds, live.frame_count));
                }
                live.core_activity().unwrap();
                committed = live.frame_probabilities;
            }
        }
        let snapshot = stream.finish().unwrap();
        assert_eq!(&snapshot.frame_probabilities[..committed.len()], &committed);
        assert_eq!(snapshot.settled_frame_count, Some(snapshot.frame_count));
        assert_eq!(
            stream.finish().unwrap().frame_probabilities,
            snapshot.frame_probabilities
        );
        assert!(stream.push_pcm16_le(&[0, 0]).is_err());
        assert!(snapshot.finished);
        assert!(snapshot.frame_count > 0);
        assert_eq!(
            snapshot.frame_probabilities.len(),
            snapshot.retained_frame_count() as usize * NEMOTRON_V3_CHANNELS
        );
        assert!(snapshot
            .segments
            .iter()
            .all(|segment| segment.channel.one_based() <= NEMOTRON_V3_CHANNELS as u8));
        println!("audited native lifecycle: first_live={first_live_frontier:?}, final_frames={}, input_seconds={}, settled_core_spans={}",
            snapshot.frame_count, snapshot.input.end_seconds, snapshot.core_activity().unwrap().len());
        stream.reset().unwrap();
        assert_eq!(stream.input_watermark().end_sample, 0);
        assert_eq!(stream.snapshot().unwrap().frame_count, 0);
    }
}
