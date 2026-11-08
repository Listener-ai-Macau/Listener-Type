//! Native KWS can reset its token clock after silence without advancing its
//! public segment start. Replay from a measured speech onset; only fresh KWS
//! token coordinates can authorize the handoff boundary.
use super::microphone_wake_buffer::{WakeBuffer, PCM_BYTES_PER_SECOND};
use crate::wake_phrase::{Match, StreamingDetector};
use parking_lot::Mutex;

pub(super) fn recover_source_clock(
    phrase: &str,
    buffer: &Mutex<WakeBuffer>,
    through: u64,
) -> Result<Option<(u64, Match)>, String> {
    let started = std::time::Instant::now();
    let (origin, pcm) = {
        let b = buffer.lock();
        let start = through
            .saturating_sub(9 * PCM_BYTES_PER_SECOND)
            .max(b.start);
        (start, b.range(start, through)?)
    };
    let ranges = crate::speech_vad_windows::cached_wake_speech_ranges(&pcm)?;
    #[cfg(test)]
    eprintln!(
        "onset snapshot_origin_ms={} through_ms={} speech_ranges={ranges:?}",
        origin / 32,
        through / 32
    );
    for (start, _end) in ranges.into_iter().rev().take(3) {
        // Existing VAD chooses an input crop only. The 200ms pre-roll protects
        // the first syllable; it does not become a guessed keyword timestamp.
        let crop = (start * 2).saturating_sub(200 * 32);
        let samples = &pcm[crop as usize..];
        let mut fresh = StreamingDetector::new_strict(phrase)?;
        let mut detected = None;
        for chunk in samples.chunks(5120) {
            if let Some(found) = fresh.accept_pcm(chunk)? {
                detected = Some(found);
                break;
            }
        }
        // This immutable snapshot is a complete replay input. Flush the
        // decoder's right context without admitting timestamps past real PCM.
        if detected.is_none() {
            detected = fresh.finish()?;
        }
        if let Some(found) = detected {
            #[cfg(test)]
            eprintln!(
                "onset crop_ms={} replay_ms={} native_start={:?} native_end={:?}",
                crop / 32,
                samples.len() / 32,
                found.start_seconds,
                found.audio_end_seconds
            );
            let proven = found.audio_end_seconds.is_some_and(|seconds| {
                let end = origin + crop + (seconds as f64 * 16000.0).round() as u64 * 2;
                end <= through && through - end <= PCM_BYTES_PER_SECOND
            });
            if proven && found.matched_keyword.as_deref() == Some(phrase) {
                log::info!("[microphone-wake] keyword clock recovered source=vad_onset snapshot_ms={} onset_ms={} origin_ms={} replay_ms={} elapsed_ms={}",pcm.len()/32,start/16,(origin+crop)/32,samples.len()/32,started.elapsed().as_millis());
                return Ok(Some((origin + crop, found)));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit existing synthetic fixtures with native VAD/KWS/helper; no microphone or cloud"]
    fn microphone_wake_source_onset_recovers_the_first_unproven_snapshot_without_another_phrase() {
        let root = std::path::PathBuf::from(std::env::var("LISTENER_WAKE_DIAG_DIR").unwrap());
        crate::asr::local::wake_helper::preload().unwrap();
        let mut missed_fixed = 0;
        for (idle_ms, name) in [
            (10000, "wake_huihui"),
            (10080, "wake_huihui"),
            (18000, "wake_huihui"),
            (18080, "wake_huihui"),
            (18000, "neg_kaishi_luxiang"),
            (0, "neg_kaishi_luxiang"),
            (0, "neg_kaishi_luyan"),
            (0, "neg_kaishi_luru"),
        ] {
            let wav = std::fs::read(root.join(format!("{name}.wav"))).unwrap();
            let offset = wav.windows(4).position(|w| w == b"data").unwrap() + 8;
            let len = u32::from_le_bytes(wav[offset - 4..offset].try_into().unwrap()) as usize;
            let mut audio = vec![0; idle_ms * 32];
            audio.extend_from_slice(&wav[offset..offset + len]);
            audio.extend(vec![0; 32000]);
            let buffer = Mutex::new(WakeBuffer::default());
            let mut primary = StreamingDetector::new_strict("开始录音").unwrap();
            let mut initial = None;
            for pcm in audio.chunks(5120) {
                buffer.lock().append(pcm);
                if let Some(found) = primary.accept_pcm(pcm).unwrap() {
                    initial = Some(found);
                    break;
                }
            }
            let Some(first) = initial else {
                assert!(
                    name.starts_with("neg_"),
                    "positive needs strict primary label"
                );
                eprintln!("source-onset case={name} idle_ms={idle_ms} primary_label=false rejected_before_recovery=true");
                continue;
            };
            if name == "wake_huihui" {
                assert!(
                    first.audio_end_seconds.is_none(),
                    "exercise the reset-relative native clock"
                );
            }
            let through = buffer.lock().end;
            let mut fixed_proven = false;
            for seconds in [3, 6, 9] {
                let start = through
                    .saturating_sub(seconds * PCM_BYTES_PER_SECOND)
                    .max(buffer.lock().start);
                let pcm = buffer.lock().range(start, through).unwrap();
                let mut fresh = StreamingDetector::new_strict("开始录音").unwrap();
                for chunk in pcm.chunks(5120) {
                    if let Some(found) = fresh.accept_pcm(chunk).unwrap() {
                        fixed_proven |= found.audio_end_seconds.is_some();
                        break;
                    }
                }
            }
            let Some((origin, found)) = recover_source_clock("开始录音", &buffer, through).unwrap()
            else {
                assert!(
                    name.starts_with("neg_"),
                    "first snapshot has measured source speech onset"
                );
                eprintln!("source-onset case={name} idle_ms={idle_ms} onset_proven=false rejected_before_confirmation=true");
                continue;
            };
            let end =
                origin + (found.audio_end_seconds.unwrap() as f64 * 16000.0).round() as u64 * 2;
            assert!(end > idle_ms as u64 * 32 && end < (idle_ms as u64 + 3500) * 32);
            let now = buffer.lock().end;
            let target = end + 32000;
            if now < target {
                buffer.lock().append(&audio[now as usize..target as usize]);
            }
            let start =
                origin + (found.start_seconds.unwrap_or(0.0) as f64 * 16000.0).round() as u64 * 2;
            let pcm = {
                let b = buffer.lock();
                b.range(start, b.end.min(target)).unwrap()
            };
            let confirmed =
                super::super::microphone_wake_decoder::confirm_candidate(&pcm, "开始录音").unwrap();
            eprintln!("source-onset case={name} idle_ms={idle_ms} first_native_proven=false fixed_proven={fixed_proven} onset_proven=true confirmed={confirmed:?} end_ms={} replay_origin_ms={}",end/32,origin/32);
            if name == "wake_huihui" {
                assert!(confirmed.is_some());
                if !fixed_proven {
                    missed_fixed += 1;
                }
            } else {
                assert!(confirmed.is_none());
            }
            let (body, through) = {
                let b = buffer.lock();
                (b.body_after(end).unwrap(), b.end)
            };
            assert_eq!(body, audio[end as usize..through as usize]);
        }
        assert!(missed_fixed>0,"same first snapshot must demonstrate a fixed-window miss recovered without fresh speech");
    }
}
