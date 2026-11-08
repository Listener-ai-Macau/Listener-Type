//! A live native candidate keeps its recorder while bounded confirmation recovers.
//! The original phrase/owner checks still authorize admission; no text is logged.
use super::microphone_wake_buffer::{WakeBuffer, PCM_BYTES_PER_SECOND, RETAINED_BYTES};
use crate::wake_phrase::LocalPhraseRelation;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(20);

enum Attempt {
    Confirmed(LocalPhraseRelation),
    Mismatch,
    Unavailable,
}

fn confirm_with_available_helper(
    pcm: &[u8],
    cancel: &AtomicBool,
    deadline: Instant,
    confirm: &mut impl FnMut(&[u8], Duration) -> Result<Option<LocalPhraseRelation>, String>,
) -> Attempt {
    if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
        return Attempt::Unavailable;
    }
    // Readiness polling must not repeatedly scan/allocate the same waveform.
    let normalized = crate::wake_phrase::gain_normalized_pcm16(pcm);
    let mut unavailable_logged = false;
    let mut missing_seen = false;
    loop {
        if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return Attempt::Unavailable;
        }
        match confirm(
            &normalized,
            deadline.saturating_duration_since(Instant::now()),
        ) {
            Ok(result) => {
                if cancel.load(Ordering::SeqCst) {
                    return Attempt::Unavailable;
                }
                log::info!("[microphone-wake] phrase confirmation result matched={} relation={result:?} pcm_ms={}", result.is_some(), pcm.len()/32);
                return match result {
                    Some(relation) => Attempt::Confirmed(relation),
                    None => Attempt::Mismatch,
                };
            }
            Err(error)
                if error == "local_wake_helper_busy" || error == "local_wake_helper_not_ready" =>
            {
                // No model request is sent while the helper lane is occupied.
                // Missing helpers recover through the existing single-flight background loader.
                if error == "local_wake_helper_not_ready" {
                    // A second empty slot means background preparation failed.
                    // Do not repeatedly restart a missing/broken model.
                    if missing_seen {
                        return Attempt::Unavailable;
                    }
                    missing_seen = true;
                }
                if !unavailable_logged {
                    let reason = if error == "local_wake_helper_busy" {
                        "helper_busy"
                    } else {
                        "helper_not_ready"
                    };
                    log::info!("[microphone-wake] phrase confirmation unavailable reason={reason} pcm_ms={}",pcm.len()/32);
                    unavailable_logged = true;
                }
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
            Err(_) => {
                log::info!("[microphone-wake] phrase confirmation unavailable reason=helper_error pcm_ms={}",pcm.len()/32);
                return Attempt::Unavailable;
            }
        }
    }
}

pub(super) fn confirm_live_candidate(
    buffer: &Mutex<WakeBuffer>,
    cancel: &AtomicBool,
    candidate_start: u64,
    keyword_end: u64,
    mut confirm: impl FnMut(&[u8], Duration) -> Result<Option<LocalPhraseRelation>, String>,
) -> Option<LocalPhraseRelation> {
    let deadline = Instant::now() + BUDGET;
    let (original, original_end) = {
        let b = buffer.lock();
        if candidate_start >= keyword_end || keyword_end > b.end {
            return None;
        }
        let end = b.end.min(keyword_end + PCM_BYTES_PER_SECOND);
        (b.range(candidate_start, end).ok()?, end)
    };
    match confirm_with_available_helper(&original, cancel, deadline, &mut confirm) {
        Attempt::Confirmed(relation) => return Some(relation),
        Attempt::Mismatch => {}
        Attempt::Unavailable => return None,
    }
    if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
        return None;
    }

    // The original path already permits up to one second after the native
    // keyword boundary. Wait for those real samples only after a failed first
    // confirmation; do not reopen capture or extend the admission timeout.
    let target_end = keyword_end + PCM_BYTES_PER_SECOND;
    let recovered = loop {
        if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return None;
        }
        {
            let b = buffer.lock();
            if b.end >= target_end {
                let start = candidate_start
                    .min(keyword_end.saturating_sub(4 * PCM_BYTES_PER_SECOND))
                    .max(b.start)
                    .max(target_end.saturating_sub(RETAINED_BYTES as u64));
                if start >= keyword_end {
                    return None;
                }
                break b.range(start, target_end).ok()?;
            }
            if keyword_end < b.start {
                return None;
            }
        }
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    };
    if recovered == original {
        return None;
    }
    log::info!("[microphone-wake] phrase confirmation recovery original_pcm_ms={} recovered_pcm_ms={} original_through_ms={} through_ms={} remaining_ms={}",original.len()/32,recovered.len()/32,original_end/32,target_end/32,deadline.saturating_duration_since(Instant::now()).as_millis());
    // Extra context must confirm the phrase at the start. A later complete
    // command cannot turn an earlier near-word candidate into a valid wake.
    match confirm_with_available_helper(&recovered, cancel, deadline, &mut confirm) {
        Attempt::Confirmed(
            relation @ (LocalPhraseRelation::ExactStart | LocalPhraseRelation::PhoneticStart),
        ) => Some(relation),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Mutex<WakeBuffer> {
        let mut b = WakeBuffer::default();
        b.append(&vec![0; 4 * 32_000]);
        Mutex::new(b)
    }

    #[test]
    #[ignore = "explicit existing neutral synthetic PCM through real native KWS and helper; no microphone/cloud"]
    fn microphone_confirmation_native_same_capture_recovers_clipped_first_input_and_rejects_near_word(
    ) {
        use super::super::microphone_wake_decoder::{
            confirm_normalized_candidate_with_timeout, Chunk, Decoder,
        };
        let root = std::path::PathBuf::from(std::env::var("LISTENER_WAKE_DIAG_DIR").unwrap());
        crate::asr::local::wake_helper::preload().unwrap();
        for name in ["wake_huihui", "neg_kaishi_luxiang"] {
            let wav = std::fs::read(root.join(format!("{name}.wav"))).unwrap();
            let offset = wav.windows(4).position(|w| w == b"data").unwrap() + 8;
            let length = u32::from_le_bytes(wav[offset - 4..offset].try_into().unwrap()) as usize;
            let audio = &wav[offset..offset + length];
            let mut decoder = Decoder::new(
                crate::wake_phrase::StreamingDetector::new_strict("开始录音").unwrap(),
                "开始录音".into(),
            );
            let buffer = Mutex::new(WakeBuffer::default());
            let mut boundary = None;
            for pcm in audio.chunks(5120) {
                let (start, end) = buffer.lock().append(pcm);
                if let Some(found) = decoder
                    .accept(
                        Chunk {
                            start,
                            end,
                            pcm: pcm.to_vec(),
                        },
                        &buffer,
                    )
                    .unwrap()
                {
                    boundary = found.audio_end_seconds;
                    break;
                }
            }
            let end = (boundary.expect("native source proof") as f64 * 16000.0).round() as u64 * 2;
            let through = end + PCM_BYTES_PER_SECOND;
            let current = buffer.lock().end;
            if current < through {
                buffer
                    .lock()
                    .append(&audio[current as usize..through as usize]);
            }
            let start = if name == "wake_huihui" {
                end.saturating_sub(240 * 32)
            } else {
                0
            };
            let before = buffer.lock().body_after(end).unwrap();
            let cancel = AtomicBool::new(false);
            let mut calls = 0;
            let mut first = None;
            let result = confirm_live_candidate(&buffer, &cancel, start, end, |pcm, budget| {
                calls += 1;
                let result = confirm_normalized_candidate_with_timeout(pcm, "开始录音", budget);
                if calls == 1 {
                    first = Some(result.clone());
                }
                result
            });
            eprintln!("same-capture synthetic case={name} calls={calls} first={first:?} final={result:?} native_end_ms={}",end/32);
            if name == "wake_huihui" {
                assert_eq!(first, Some(Ok(None)));
                assert!(matches!(
                    result,
                    Some(LocalPhraseRelation::ExactStart | LocalPhraseRelation::PhoneticStart)
                ));
                assert_eq!(calls, 2);
            } else {
                assert_eq!(result, None);
            }
            assert_eq!(
                buffer.lock().body_after(end).unwrap(),
                before,
                "confirmation cannot reopen, drop or duplicate body PCM"
            );
        }
    }

    #[test]
    fn microphone_confirmation_first_success_does_not_wait_or_reconfirm() {
        let buffer = source();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let result = confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |_, _| {
            calls += 1;
            Ok(Some(LocalPhraseRelation::PresentLater))
        });
        assert_eq!(result, Some(LocalPhraseRelation::PresentLater));
        assert_eq!(calls, 1);
    }

    #[test]
    fn microphone_confirmation_first_miss_uses_new_same_source_context_without_new_capture() {
        let buffer = source();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let mut budgets = vec![];
        let result =
            confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |pcm, budget| {
                calls += 1;
                budgets.push(budget);
                if calls == 1 {
                    assert_eq!(pcm.len(), 32_000);
                    buffer.lock().append(&vec![1; 32_000]);
                    Ok(None)
                } else {
                    assert_eq!(pcm.len(), 5 * 32_000);
                    assert!(buffer
                        .lock()
                        .range(4 * 32_000, 5 * 32_000)
                        .unwrap()
                        .iter()
                        .all(|v| *v == 1));
                    Ok(Some(LocalPhraseRelation::ExactStart))
                }
            });
        assert_eq!(result, Some(LocalPhraseRelation::ExactStart));
        assert_eq!(calls, 2);
        assert!(budgets[1] < budgets[0]);
        assert_eq!(buffer.lock().end, 5 * 32_000);
    }

    #[test]
    fn microphone_confirmation_busy_lane_preserves_frozen_candidate_and_one_budget() {
        let buffer = source();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let mut previous = BUDGET;
        let result =
            confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |pcm, budget| {
                calls += 1;
                assert_eq!(pcm.len(), 32_000);
                assert!(budget < previous);
                previous = budget;
                if calls < 3 {
                    Err("local_wake_helper_busy".into())
                } else {
                    Ok(Some(LocalPhraseRelation::ExactStart))
                }
            });
        assert_eq!(result, Some(LocalPhraseRelation::ExactStart));
        assert_eq!(calls, 3);
    }

    #[test]
    fn microphone_confirmation_persistent_missing_helper_does_not_loop_or_try_context() {
        let buffer = source();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let result = confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |_, _| {
            calls += 1;
            Err("local_wake_helper_not_ready".into())
        });
        assert_eq!(result, None);
        assert_eq!(calls, 2);
        assert_eq!(buffer.lock().end, 4 * 32_000);
    }

    #[test]
    fn microphone_confirmation_cancelled_model_result_cannot_start_recovery_or_admit() {
        let buffer = source();
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let result = confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |_, _| {
            calls += 1;
            cancel.store(true, Ordering::SeqCst);
            Ok(Some(LocalPhraseRelation::ExactStart))
        });
        assert_eq!(result, None);
        assert_eq!(calls, 1);
    }

    #[test]
    fn microphone_confirmation_recovery_cannot_borrow_a_later_phrase_or_unrelated_audio() {
        for later in [None, Some(LocalPhraseRelation::PresentLater)] {
            let buffer = source();
            let cancel = AtomicBool::new(false);
            let mut calls = 0;
            let result =
                confirm_live_candidate(&buffer, &cancel, 3 * 32_000, 4 * 32_000, |_, _| {
                    calls += 1;
                    if calls == 1 {
                        buffer.lock().append(&vec![1; 32_000]);
                        Ok(None)
                    } else {
                        Ok(later)
                    }
                });
            assert_eq!(result, None);
            assert_eq!(calls, 2);
        }
    }

    #[test]
    fn microphone_confirmation_expired_source_never_reaches_model() {
        let buffer = source();
        buffer.lock().append(&vec![0; RETAINED_BYTES]);
        let cancel = AtomicBool::new(false);
        let result = confirm_live_candidate(&buffer, &cancel, 0, 32_000, |_, _| {
            panic!("expired audio cannot be used")
        });
        assert_eq!(result, None);
    }
}
