use std::sync::Arc;

use crate::coordinator_state::{SessionId, SessionPhase, SessionState};
use crate::recorder::Recorder;
use tauri::Manager;

use crate::coordinator_state::DictationUiState;

use super::{publish_dictation_capsule, ActiveAsr, Inner};

pub(super) struct SessionResource<T> {
    pub(super) session_id: SessionId,
    resource: T,
}

impl<T> SessionResource<T> {
    pub(super) fn new(session_id: SessionId, resource: T) -> Self {
        Self {
            session_id,
            resource,
        }
    }

    fn into_inner(self) -> T {
        self.resource
    }
}

pub(super) struct SharedRecordingMuteState {
    guard: Option<crate::audio_mute::AudioMuteGuard>,
    holders: u32,
}

impl SharedRecordingMuteState {
    pub(super) fn new() -> Self {
        Self {
            guard: None,
            holders: 0,
        }
    }
}

pub(super) fn take_session_resource<T>(
    slot: &mut Option<SessionResource<T>>,
    session_id: SessionId,
) -> Option<T> {
    if slot
        .as_ref()
        .map(|resource| resource.session_id == session_id)
        .unwrap_or(false)
    {
        slot.take().map(SessionResource::into_inner)
    } else {
        None
    }
}

fn accepts_startup_resource(inner: &Arc<Inner>, state: &SessionState, session_id: SessionId) -> bool {
    state.session_id == session_id
        && !state.cancelled
        && matches!(state.phase, SessionPhase::Starting | SessionPhase::Listening)
        && super::recording_gate::try_admit_arc(
            inner, super::recording_gate::RecordIntent::HotkeyPress, "recording resource handoff",
        )
}

pub(super) fn recording_resource_start_allowed(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    accepts_startup_resource(inner, &inner.state.lock(), session_id)
}

pub(super) fn asr_startup_completion_allowed(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let state = inner.state.lock();
    // Finalization may still be waiting for the provider. Inserting already
    // has its final text and must not revive a late connection or callback.
    state.session_id == session_id && !state.cancelled
        && matches!(state.phase, SessionPhase::Starting | SessionPhase::Listening | SessionPhase::Processing)
        && super::recording_gate::try_admit_arc(
            inner, super::recording_gate::RecordIntent::HotkeyPress, "ASR startup completion",
        )
}

fn same_asr_connection(first: &ActiveAsr, second: &ActiveAsr) -> bool {
    match (first, second) {
        (ActiveAsr::Volcengine(a), ActiveAsr::Volcengine(b)) => Arc::ptr_eq(a, b),
        (ActiveAsr::Whisper(a), ActiveAsr::Whisper(b)) => Arc::ptr_eq(a, b),
        (ActiveAsr::Bailian(a), ActiveAsr::Bailian(b)) => Arc::ptr_eq(a, b),
        #[cfg(target_os = "windows")]
        (ActiveAsr::FoundryLocalWhisper(a), ActiveAsr::FoundryLocalWhisper(b)) => Arc::ptr_eq(a, b),
        #[cfg(target_os = "macos")]
        (ActiveAsr::Local(a), ActiveAsr::Local(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

#[must_use = "abandoned ASR setup must not continue opening or attaching audio"]
pub(super) fn store_asr_for_session(inner: &Arc<Inner>, session_id: SessionId, asr: ActiveAsr) -> bool {
    let (accepted, retired) = {
        let state = inner.state.lock();
        let mut slot = inner.asr.lock();
        let aliases_current = slot.as_ref()
            .is_some_and(|current| same_asr_connection(&current.resource, &asr));
        let already_registered = slot.as_ref().is_some_and(|current| current.session_id == session_id);
        if !accepts_startup_resource(inner, &state, session_id)
            || (already_registered && !aliases_current)
        {
            // A stale clone cannot cancel a connection still owned by the slot.
            (false, if aliases_current { None } else { Some(asr) })
        } else {
            let previous = slot.replace(SessionResource::new(session_id, asr));
            (true, if aliases_current { None } else { previous.map(SessionResource::into_inner) })
        }
    };
    // Provider cleanup can wake callbacks. Never cancel under either lock.
    if let Some(retired) = retired { cancel_active_asr(retired); }
    accepted
}

pub(super) fn take_asr_for_session(inner: &Arc<Inner>, session_id: SessionId) -> Option<ActiveAsr> {
    let mut slot = inner.asr.lock();
    take_session_resource(&mut slot, session_id)
}

pub(super) fn clone_volcengine_asr_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<Arc<crate::asr::volcengine::VolcengineStreamingASR>> {
    let slot = inner.asr.lock();
    let resource = slot.as_ref()?;
    if resource.session_id != session_id {
        return None;
    }
    match &resource.resource {
        ActiveAsr::Volcengine(asr) => Some(Arc::clone(asr)),
        _ => None,
    }
}

pub(super) fn cancel_active_asr(asr: ActiveAsr) {
    match asr {
        ActiveAsr::Volcengine(v) => v.cancel(),
        ActiveAsr::Whisper(w) => w.cancel(),
        ActiveAsr::Bailian(b) => b.cancel(),
        #[cfg(target_os = "windows")]
        ActiveAsr::FoundryLocalWhisper(local) => local.cancel(),
        #[cfg(target_os = "macos")]
        ActiveAsr::Local(local) => local.cancel(),
    }
}

pub(super) fn cancel_asr_for_session(inner: &Arc<Inner>, session_id: SessionId) {
    if let Some(asr) = take_asr_for_session(inner, session_id) {
        cancel_active_asr(asr);
    }
}

#[must_use = "abandoned recorder setup must not continue startup"]
pub(super) fn store_recorder_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
    recorder: Recorder,
    archive_active: bool,
) -> bool {
    let handoff = {
        let state = inner.state.lock();
        let mut slot = inner.recorder.lock();
        if !accepts_startup_resource(inner, &state, session_id)
            || slot.as_ref().is_some_and(|current| current.session_id == session_id)
        {
            Err(recorder)
        } else {
            let replaced = slot.replace(SessionResource::new(session_id, recorder));
            // This flag belongs to the accepted owner, just like the handle.
            inner.audio_archive_active.store(archive_active, std::sync::atomic::Ordering::Relaxed);
            Ok(replaced)
        }
    };
    // Stopping joins the audio thread, whose callbacks inspect SessionState.
    // Neither state nor recorder-slot lock may be held across that join.
    match handoff {
        Ok(replaced) => {
            if let Some(replaced) = replaced {
                replaced.into_inner().stop();
                release_recording_mute(inner, "dictation");
            }
            true
        }
        Err(rejected) => {
            rejected.stop();
            false
        }
    }
}

pub(super) fn selected_microphone_device_name(inner: &Arc<Inner>) -> Option<String> {
    let name = inner.prefs.get().microphone_device_name.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub(super) fn stop_microphone_preview_monitor(inner: &Arc<Inner>, owner: &str) {
    let Some(app) = inner.app.lock().as_ref().cloned() else {
        return;
    };
    let state = app.state::<crate::commands::MicrophoneMonitorState>();
    let recorder = state.lock().take();
    if let Some(recorder) = recorder {
        log::info!("[recorder] stopping microphone preview monitor before {owner}");
        recorder.stop();
    }
}

/// Acquire system-output mute for the duration of a recording session.
///
/// `AudioMuteGuard::activate()` on macOS shells out to `osascript` (~100–300 ms)
/// and on Linux to `wpctl`/`pactl` (similar). When called from the async
/// `begin_session` path that blocks the tokio worker thread for the entire
/// duration, delaying the recorder start by exactly that much. Wrap the
/// activate + bookkeeping in `spawn_blocking` so the tokio worker is freed
/// while the shell-out runs. Parking-lot `Mutex` guards never cross an await
/// (they live entirely inside the blocking task). Audit 3.2.4.
pub(super) async fn acquire_recording_mute(inner: &Arc<Inner>, owner: &'static str) {
    if !inner.prefs.get().mute_during_recording {
        return;
    }
    let inner = Arc::clone(inner);
    let join_result = tokio::task::spawn_blocking(move || {
        let mut mute = inner.recording_mute.lock();
        if mute.holders == 0 {
            match crate::audio_mute::AudioMuteGuard::activate() {
                Ok(guard) => {
                    mute.guard = Some(guard);
                    log::info!("[audio-mute] system output muted for recording");
                }
                Err(err) => {
                    log::warn!("[audio-mute] failed to mute output for {owner}: {err}");
                    return;
                }
            }
        }
        mute.holders = mute.holders.saturating_add(1);
        log::info!("[audio-mute] acquired by {owner}; holders={}", mute.holders);
    })
    .await;
    // 显式记录 spawn_blocking 任务的 panic（之前是 `let _ = .await` 静默吞掉）。
    // holders/guard 状态本身在 panic 路径下仍然一致 —— 因为 panic 只能发生在
    // activate() 抛 / lock 抛，前者会让 holders 不增 + guard 仍 None，后者根本
    // 进不到 mutate 阶段；但用户碰到 system audio 在录音时漏出系统声却找不到
    // 任何 [audio-mute] 日志，没法 debug。pr_agent feedback on PR #391。
    if let Err(join_err) = join_result {
        log::error!(
            "[audio-mute] acquire task panicked for {owner}: {join_err}; mute did not activate"
        );
    }
}

/// Release the recording-mute guard. The Drop impl on `AudioMuteGuard` shells
/// out to `osascript` / `wpctl` again, so when holders reaches 0 we hand the
/// drop off to a blocking task to keep the tokio worker free. Audit 3.2.4.
///
/// Fire-and-forget (no await): callers — `cancel_session`, `end_session`,
/// recorder error monitor — don't need the mute restoration to complete
/// before they continue. The user has already stopped recording; system audio
/// recovery happening 100 ms later is fine.
///
/// `release_recording_mute` is also called from non-tokio threads (the recorder
/// error monitor uses `std::thread::spawn`), so fall back to a synchronous
/// run when there's no current tokio handle — running synchronously on a std
/// thread blocks nothing.
pub(super) fn release_recording_mute(inner: &Arc<Inner>, owner: &'static str) {
    let inner = Arc::clone(inner);
    let work = move || {
        let mut mute = inner.recording_mute.lock();
        if mute.holders == 0 {
            return;
        }
        mute.holders -= 1;
        log::info!("[audio-mute] released by {owner}; holders={}", mute.holders);
        if mute.holders == 0 {
            mute.guard.take();
            log::info!("[audio-mute] system output mute restored after recording");
        }
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn_blocking(work);
    } else {
        work();
    }
}

pub(super) fn stop_qa_recorder(inner: &Arc<Inner>, session_id: SessionId) {
    let recorder = take_session_resource(&mut inner.qa_recorder.lock(), session_id);
    if let Some(rec) = recorder {
        rec.stop();
        release_recording_mute(inner, "qa");
    }
}

pub(super) fn take_recorder_for_session(
    inner: &Arc<Inner>,
    session_id: SessionId,
) -> Option<Recorder> {
    let mut slot = inner.recorder.lock();
    take_session_resource(&mut slot, session_id)
}

pub(super) fn stop_recorder_for_session(inner: &Arc<Inner>, session_id: SessionId) {
    if let Some(recorder) = take_recorder_for_session(inner, session_id) {
        recorder.stop();
        release_recording_mute(inner, "dictation");
    }
}

pub(super) fn discard_startup_resources_for_session(inner: &Arc<Inner>, session_id: SessionId) {
    stop_recorder_for_session(inner, session_id);
    cancel_asr_for_session(inner, session_id);
}

pub(super) fn stop_recorder_if_pending_start_stop(inner: &Arc<Inner>) {
    let (should_stop, session_id) = {
        let state = inner.state.lock();
        (
            state.phase == SessionPhase::Starting && state.pending_stop,
            state.session_id,
        )
    };
    if !should_stop {
        return;
    }
    if let Some(rec) = take_recorder_for_session(inner, session_id) {
        rec.stop();
        release_recording_mute(inner, "dictation");
        publish_dictation_capsule(
            inner,
            session_id,
            DictationUiState::Transcribing,
            0.0,
            None,
            None,
        );
        log::info!("[coord] stopped recorder while ASR is still connecting");
    }
}
