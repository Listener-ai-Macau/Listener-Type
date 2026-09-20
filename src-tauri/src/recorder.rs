//! 麦克风采集：平台 CaptureSession 拉流 → 16 kHz 单声道 Int16 PCM → 喂给 `AudioConsumer`。
//!
//! 与 Swift 版 `ListenerTypeRecorder/Recorder.swift` 行为对齐：
//! - 输出格式固定为 16 kHz 单声道小端 Int16，方便 ASR 直接消费。
//! - 多声道输入 → 算术平均下混到单声道；非 16 kHz → 线性插值重采样。
//! - 每个 buffer 计算 RMS 归一化到 0..1（再乘以 4 并 clamp），用于胶囊电平动画。
//! - 每 ~50 个回调打一行诊断日志，包含峰值 RMS。
//!
//! 线程模型：
//! - 平台线程持有 `!Send` 的 cpal `Stream`。
//! - 产品句柄停止平台 session，并保留 Listener 的 PCM/RMS/WAV 语义。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use denzic_host_audio_v1_core::capture::{self, CaptureError, CaptureOptions, CaptureSession};
use parking_lot::Mutex;
use serde::Serialize;
use thiserror::Error;

/// 目标采样率（与 Swift 端常量一致；不要改）。
const TARGET_SAMPLE_RATE: u32 = 16_000;
/// 每多少个回调打一次诊断日志。
const LOG_EVERY_N_CALLBACKS: usize = 50;
/// RMS → UI 电平的放大系数，与 Swift 端 `min(1.0, rms * 4)` 一致。
const LEVEL_RMS_GAIN: f32 = 4.0;

/// 接收已重采样 Int16 PCM 字节流（小端）的下游。
pub trait AudioConsumer: Send + Sync {
    /// 每次拿到的是若干 Int16 样本拼成的 little-endian 字节序列。
    /// 长度一定是 2 的倍数。
    fn consume_pcm_chunk(&self, pcm: &[u8]);

    /// Evidence-only source-carrying path. The default keeps all existing
    /// recorder consumers byte-for-byte unchanged.
    fn consume_pcm_chunk_with_source(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
    ) {
        self.consume_pcm_chunk_with_source_interval(pcm, observation, segment_id, None);
    }

    /// Explicit source-coordinate variant.  Metadata is carried beside the
    /// PCM through deferred adapters; legacy consumers keep the old behavior.
    fn consume_pcm_chunk_with_source_interval(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
    ) {
        let _ = (observation, segment_id, source_interval);
        self.consume_pcm_chunk(pcm);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneDevice {
    pub name: String,
    pub is_default: bool,
}

/// 采集器错误。
#[derive(Debug, Error)]
pub enum RecorderError {
    #[error("microphone permission denied")]
    PermissionDenied,
    #[error("audio engine failed: {0}")]
    EngineFailed(String),
}

/// 采集器句柄。Drop 时不会自动停止——必须显式调用 `stop`。
pub struct Recorder {
    session: Mutex<Option<CaptureSession>>,
}

impl Recorder {
    /// 启动采集。`consumer` 收到 16 kHz/Mono/Int16-LE 的 PCM；
    /// `level_handler` 收到 0..1 的 RMS 电平。
    /// `audio_archive_path` 不为 None 时，同样的 16 kHz/Mono/Int16-LE 旁路写入 WAV 文件，
    /// 用于 debug 麦克风灵敏度 / ASR 误识别。Drop 时自动回填 RIFF / data 长度。
    ///
    /// 返回值第三个 `bool` = "archive 实际成功创建"：caller 写 history 时应当用这个值
    /// 决定 `has_audio_recording`，而不是 prefs 开关。开关打开但写盘失败（路径不存在 /
    /// 权限不足 / 磁盘满）时仍返回 false，避免前端渲染播放按钮后端却 404。
    ///
    /// 实际的 cpal Stream 由平台 CaptureSession 在线程内构造、播放和析构。
    pub fn start(
        microphone_device_name: Option<String>,
        consumer: Arc<dyn AudioConsumer>,
        level_handler: Arc<dyn Fn(f32) + Send + Sync>,
        audio_archive_path: Option<PathBuf>,
    ) -> Result<(Self, Receiver<RecorderError>, bool), RecorderError> {
        // 运行期错误：平台统一上报 cpal 与 liveness watchdog 事件。
        let (runtime_error_tx, runtime_error_rx) = channel::<RecorderError>();

        // 同步路径上尝试创建 WavArchiver——成功 / 失败都立刻知道，传给 caller 决定
        // 是否在 history 标 has_audio_recording。失败仅 log::warn 不抛错，主路径继续。
        let archiver = audio_archive_path.and_then(|path| match WavArchiver::create(&path) {
            Ok(arch) => Some(Arc::new(Mutex::new(arch))),
            Err(err) => {
                log::warn!("[recorder] wav archive create failed at {path:?}: {err}");
                None
            }
        });
        let archive_active = archiver.is_some();
        let state = Arc::new(StreamState::new());
        let state_for_sink = Arc::clone(&state);
        let consumer_for_sink = Arc::clone(&consumer);
        let level_for_sink = Arc::clone(&level_handler);
        let archive_for_sink = archiver.clone();
        let error_tx = runtime_error_tx.clone();
        let session = CaptureSession::start(
            CaptureOptions {
                device_name: microphone_device_name,
                on_stream_error: Some(Arc::new(move |message| {
                    let _ = error_tx.send(map_capture_runtime_error(message));
                })),
            },
            move |samples, input_sr| {
                process_callback(
                    samples,
                    1,
                    input_sr,
                    consumer_for_sink.as_ref(),
                    level_for_sink.as_ref(),
                    archive_for_sink.as_deref(),
                    state_for_sink.as_ref(),
                );
            },
        )
        .map_err(map_capture_error)?;

        Ok((
            Self {
                session: Mutex::new(Some(session)),
            },
            runtime_error_rx,
            archive_active,
        ))
    }

    /// 停止采集并等待音频线程退出。
    ///
    /// 用 `self`（消费）签名，与 Swift API 语义一致——一次性资源。
    pub fn stop(self) {
        if let Some(session) = self.session.lock().take() {
            if let Err(err) = session.stop() {
                log::warn!("recorder 线程停止失败: {err}");
            }
        }
    }
}

pub fn list_input_devices() -> Result<Vec<MicrophoneDevice>, RecorderError> {
    capture::list_input_devices()
        .map(|devices| {
            devices
                .into_iter()
                .map(|device| MicrophoneDevice {
                    name: device.name,
                    is_default: device.is_default,
                })
                .collect()
        })
        .map_err(map_capture_error)
}

fn map_capture_error(error: CaptureError) -> RecorderError {
    match error {
        CaptureError::PermissionDenied => RecorderError::PermissionDenied,
        other => RecorderError::EngineFailed(other.to_string()),
    }
}

fn map_capture_runtime_error(message: String) -> RecorderError {
    if let Some(seconds) = message
        .strip_prefix("audio callback silent for ")
        .and_then(|rest| rest.strip_suffix(" seconds"))
    {
        return RecorderError::EngineFailed(format!("录音回调静默停止 {seconds} 秒"));
    }
    if let Some(seconds) = message
        .strip_prefix("no audio callback within ")
        .and_then(|rest| rest.strip_suffix(" seconds after capture start"))
    {
        return RecorderError::EngineFailed(format!("录音启动后 {seconds} 秒内未收到回调"));
    }
    RecorderError::EngineFailed(format!("stream: {message}"))
}

/// 跨回调维持的状态：上一帧残留（重采样），诊断计数与峰值。
struct StreamState {
    /// 上一回调没被消费完的"小数位置"。线性插值重采样会跨 buffer。
    resample_phase: Mutex<f64>,
    /// 上一回调最后一帧（单声道下混后），下一回调插值起点。
    last_sample: Mutex<f32>,
    callback_count: AtomicUsize,
    peak_input_rms_milli: AtomicUsize,
    peak_output_rms_milli: AtomicUsize,
    /// 最后一次成功调用 consumer 的时间戳（用于 liveness 检测）
    last_callback_time: Mutex<Option<std::time::Instant>>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            resample_phase: Mutex::new(0.0),
            last_sample: Mutex::new(0.0),
            callback_count: AtomicUsize::new(0),
            peak_input_rms_milli: AtomicUsize::new(0),
            peak_output_rms_milli: AtomicUsize::new(0),
            // 初始化为 None，只有在第一次回调后才开始计时，避免误报慢启动设备
            last_callback_time: Mutex::new(None),
        }
    }
}

/// 单次回调：下混 → 重采样 → 量化为 i16 → 算 RMS → 喂下游。
fn process_callback(
    interleaved: &[f32],
    channels: usize,
    input_sr: u32,
    consumer: &dyn AudioConsumer,
    level_handler: &(dyn Fn(f32) + Send + Sync),
    archiver: Option<&Mutex<WavArchiver>>,
    state: &StreamState,
) {
    if interleaved.is_empty() || channels == 0 {
        return;
    }

    let mono = downmix_to_mono(interleaved, channels);
    let input_rms = rms(&mono);

    let resampled = resample_to_target(&mono, input_sr, TARGET_SAMPLE_RATE, state);
    if resampled.is_empty() {
        return;
    }

    let (pcm_bytes, output_rms) = quantize_to_i16_le(&resampled);
    let level = (output_rms * LEVEL_RMS_GAIN).clamp(0.0, 1.0);

    consumer.consume_pcm_chunk(&pcm_bytes);
    if let Some(arch) = archiver {
        arch.lock().append(&pcm_bytes);
    }
    level_handler(level);

    // 更新最后一次成功调用的时间戳（用于 liveness 检测）
    *state.last_callback_time.lock() = Some(std::time::Instant::now());

    // 诊断：峰值 + 周期性日志。
    let count = state.callback_count.fetch_add(1, Ordering::Relaxed) + 1;
    update_peak(&state.peak_input_rms_milli, input_rms);
    update_peak(&state.peak_output_rms_milli, output_rms);
    if count == 1 || count % LOG_EVERY_N_CALLBACKS == 0 {
        let pk_in = state.peak_input_rms_milli.load(Ordering::Relaxed) as f32 / 1000.0;
        let pk_out = state.peak_output_rms_milli.load(Ordering::Relaxed) as f32 / 1000.0;
        log::info!(
            "[recorder] cb#{count} inLen={} outLen={} inRMS={:.5} outRMS={:.5} peakIn={:.5} peakOut={:.5}",
            mono.len(),
            resampled.len(),
            input_rms,
            output_rms,
            pk_in,
            pk_out
        );
    }
}

/// 多声道交错样本 → 单声道（算术平均）。
fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / channels;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let base = i * channels;
        let mut sum = 0.0f32;
        for c in 0..channels {
            sum += interleaved[base + c];
        }
        out.push(sum / channels as f32);
    }
    out
}

/// 线性插值重采样到目标采样率，状态跨 buffer 保留。
///
/// 算法说明：把上一回调的尾样本作为本回调起点，避免缝隙；用浮点
/// `phase` 记录"已经走到上一帧的多少位置"，每输出一个目标样本前进
/// `step = src_sr / dst_sr`。
fn resample_to_target(samples: &[f32], src_sr: u32, dst_sr: u32, state: &StreamState) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    if src_sr == dst_sr {
        // 直通——但仍需更新 last_sample，便于切换设备时不抖。
        if let Some(&last) = samples.last() {
            *state.last_sample.lock() = last;
        }
        return samples.to_vec();
    }

    let step = src_sr as f64 / dst_sr as f64;
    let mut phase = *state.resample_phase.lock();
    let prev = *state.last_sample.lock();

    // 估容量：dst_len ≈ src_len / step。
    let estimated = ((samples.len() as f64) / step).ceil() as usize + 1;
    let mut out = Vec::with_capacity(estimated);

    // 把 prev 作为虚拟索引 -1 的样本。
    // phase 表示"距离当前段起点还差多少"，区间 [0, 1)。
    while phase < samples.len() as f64 {
        let idx_floor = phase.floor() as isize;
        let frac = (phase - phase.floor()) as f32;
        let a = if idx_floor < 0 {
            prev
        } else {
            samples[idx_floor as usize]
        };
        let b_index = (idx_floor + 1) as usize;
        if b_index >= samples.len() {
            // 没有下一帧可插值——把当前帧填进去并退出，让下一回调接力。
            out.push(a);
            phase += step;
            break;
        }
        let b = samples[b_index];
        out.push(a + (b - a) * frac);
        phase += step;
    }

    // 把 phase 折回到"相对于下一回调起点"——减去当前 buffer 长度。
    let new_phase = phase - samples.len() as f64;
    *state.resample_phase.lock() = new_phase.max(0.0);
    *state.last_sample.lock() = *samples.last().unwrap_or(&0.0);

    out
}

/// f32 → i16 little-endian 字节流，并顺手算 RMS（归一化到 0..1）。
fn quantize_to_i16_le(samples: &[f32]) -> (Vec<u8>, f32) {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let q = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&q.to_le_bytes());
        let n = clamped as f64;
        sum_sq += n * n;
    }
    let rms = if samples.is_empty() {
        0.0
    } else {
        (sum_sq / samples.len() as f64).sqrt() as f32
    };
    (bytes, rms)
}

/// f32 切片 RMS（归一化到 0..1，假设输入已在 [-1, 1]）。
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum_sq = 0.0f64;
    for &s in samples {
        let n = s as f64;
        sum_sq += n * n;
    }
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// 用毫单位整数原子值近似存储 f32 峰值（避免引入额外锁）。
fn update_peak(slot: &AtomicUsize, current: f32) {
    let scaled = (current * 1000.0).round().max(0.0) as usize;
    let mut prev = slot.load(Ordering::Relaxed);
    while scaled > prev {
        match slot.compare_exchange_weak(prev, scaled, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => prev = observed,
        }
    }
}

/// 16 kHz / mono / 16-bit PCM WAV 的简易追加写入器。
/// 构造时写一个 data_size=0 的 header 占位，每次 append 把 i16 PCM bytes 追加到文件，
/// Drop 时 seek 回 0 把 RIFF / data 长度字段回填——避免依赖外部 finalize 调用点。
struct WavArchiver {
    file: std::fs::File,
    bytes_written: u32,
}

impl WavArchiver {
    fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(path)?;
        use std::io::Write;
        file.write_all(&build_wav_header(0))?;
        Ok(Self {
            file,
            bytes_written: 0,
        })
    }

    fn append(&mut self, pcm_bytes: &[u8]) {
        use std::io::Write;
        if self.file.write_all(pcm_bytes).is_ok() {
            self.bytes_written = self
                .bytes_written
                .saturating_add(pcm_bytes.len().min(u32::MAX as usize) as u32);
        }
    }
}

impl Drop for WavArchiver {
    fn drop(&mut self) {
        use std::io::{Seek, SeekFrom, Write};
        let header = build_wav_header(self.bytes_written);
        if self.file.seek(SeekFrom::Start(0)).is_ok() {
            let _ = self.file.write_all(&header);
            let _ = self.file.sync_all();
        }
    }
}

fn build_wav_header(data_size: u32) -> [u8; 44] {
    // RIFF/WAVE PCM 标准 44-byte header，布局由平台 host_audio 契约统一定义。
    denzic_host_audio_v1_core::wav::wav_header(data_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Default)]
    struct RecordingConsumer {
        chunks: StdMutex<Vec<Vec<u8>>>,
    }

    impl AudioConsumer for RecordingConsumer {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            self.chunks.lock().unwrap().push(pcm.to_vec());
        }
    }

    fn decode_i16_le(bytes: &[u8]) -> Vec<i16> {
        bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect()
    }

    #[test]
    fn downmix_to_mono_averages_complete_interleaved_frames() {
        let mono = downmix_to_mono(&[1.0, -1.0, 0.5, 0.25, 0.0], 2);

        assert_eq!(mono, vec![0.0, 0.375]);
    }

    #[test]
    fn quantize_to_i16_le_clamps_and_reports_rms() {
        let (bytes, rms) = quantize_to_i16_le(&[-2.0, 0.0, 0.5, 2.0]);
        let samples = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();

        assert_eq!(samples, vec![-32767, 0, 16383, 32767]);
        assert!((rms - 0.75).abs() < 0.0001);
    }

    #[test]
    fn resample_passthrough_updates_tail_sample_without_phase_drift() {
        let state = StreamState::new();
        *state.resample_phase.lock() = 0.5;

        let out = resample_to_target(
            &[0.1, -0.2, 0.3],
            TARGET_SAMPLE_RATE,
            TARGET_SAMPLE_RATE,
            &state,
        );

        assert_eq!(out, vec![0.1, -0.2, 0.3]);
        assert_eq!(*state.last_sample.lock(), 0.3);
        assert_eq!(*state.resample_phase.lock(), 0.5);
    }

    #[test]
    fn resample_upsamples_with_linear_interpolation_and_tail_state() {
        let state = StreamState::new();

        let out = resample_to_target(&[0.0, 1.0], 8_000, TARGET_SAMPLE_RATE, &state);

        assert_eq!(out, vec![0.0, 0.5, 1.0]);
        assert_eq!(*state.last_sample.lock(), 1.0);
        assert_eq!(*state.resample_phase.lock(), 0.0);
    }

    #[test]
    fn process_callback_resamples_non_target_input_before_emitting_pcm() {
        let consumer = RecordingConsumer::default();
        let levels = Arc::new(StdMutex::new(Vec::new()));
        let levels_for_handler = Arc::clone(&levels);
        let state = StreamState::new();

        process_callback(
            &[0.0, 1.0],
            1,
            8_000,
            &consumer,
            &move |level| levels_for_handler.lock().unwrap().push(level),
            None,
            &state,
        );

        let chunks = consumer.chunks.lock().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(decode_i16_le(&chunks[0]), vec![0, 16383, 32767]);
        assert_eq!(*levels.lock().unwrap(), vec![1.0]);
        assert_eq!(state.callback_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn process_callback_reports_scaled_rms_level_and_peaks() {
        let consumer = RecordingConsumer::default();
        let levels = Arc::new(StdMutex::new(Vec::new()));
        let levels_for_handler = Arc::clone(&levels);
        let state = StreamState::new();

        process_callback(
            &[0.125, -0.125],
            1,
            TARGET_SAMPLE_RATE,
            &consumer,
            &move |level| levels_for_handler.lock().unwrap().push(level),
            None,
            &state,
        );

        let levels = levels.lock().unwrap();
        assert_eq!(levels.len(), 1);
        assert!((levels[0] - 0.5).abs() < 0.0001);
        assert_eq!(state.peak_input_rms_milli.load(Ordering::Relaxed), 125);
        assert_eq!(state.peak_output_rms_milli.load(Ordering::Relaxed), 125);
    }

    #[test]
    fn process_callback_emits_pcm_level_and_liveness_marker() {
        let consumer = RecordingConsumer::default();
        let levels = Arc::new(StdMutex::new(Vec::new()));
        let levels_for_handler = Arc::clone(&levels);
        let state = StreamState::new();

        process_callback(
            &[0.25, -0.25],
            1,
            TARGET_SAMPLE_RATE,
            &consumer,
            &move |level| levels_for_handler.lock().unwrap().push(level),
            None,
            &state,
        );

        let chunks = consumer.chunks.lock().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 4);
        assert_eq!(*levels.lock().unwrap(), vec![1.0]);
        assert!(state.last_callback_time.lock().is_some());
        assert_eq!(state.callback_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn process_callback_ignores_empty_or_zero_channel_input_without_liveness_marker() {
        let consumer = RecordingConsumer::default();
        let levels = Arc::new(StdMutex::new(Vec::new()));
        let levels_for_handler = Arc::clone(&levels);
        let state = StreamState::new();

        process_callback(
            &[],
            1,
            TARGET_SAMPLE_RATE,
            &consumer,
            &move |level| levels_for_handler.lock().unwrap().push(level),
            None,
            &state,
        );
        process_callback(
            &[0.25, -0.25],
            0,
            TARGET_SAMPLE_RATE,
            &consumer,
            &move |level| levels.lock().unwrap().push(level),
            None,
            &state,
        );

        assert!(consumer.chunks.lock().unwrap().is_empty());
        assert!(state.last_callback_time.lock().is_none());
        assert_eq!(state.callback_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn platform_runtime_errors_keep_listener_wording() {
        assert_eq!(
            map_capture_runtime_error("audio callback silent for 4 seconds".into()).to_string(),
            "audio engine failed: 录音回调静默停止 4 秒"
        );
        assert_eq!(
            map_capture_runtime_error(
                "no audio callback within 6 seconds after capture start".into()
            )
            .to_string(),
            "audio engine failed: 录音启动后 6 秒内未收到回调"
        );
        assert_eq!(
            map_capture_runtime_error("device lost".into()).to_string(),
            "audio engine failed: stream: device lost"
        );
    }
}
