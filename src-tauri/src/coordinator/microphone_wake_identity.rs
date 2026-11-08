//! Identity input belongs to the detected keyword, not the whole KWS segment.
use super::microphone_wake_buffer::WakeBuffer;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) enum KeywordOwnerInput {
    Focused { start: u64, end: u64, pcm: Vec<u8> },
    LegacyShort,
    Unavailable,
}

pub(super) fn snapshot(
    buffer: &Mutex<WakeBuffer>, cancel: &AtomicBool, start: u64, end: u64,
) -> Option<Vec<u8>> {
    let deadline=Instant::now()+Duration::from_secs(5);
    loop {
        if cancel.load(Ordering::SeqCst) { return None; }
        {
            let buffer=buffer.lock();
            if start<buffer.start { return None; }
            if buffer.end>=end { return buffer.range(start,end).ok(); }
        }
        if Instant::now()>=deadline { return None; }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn keyword_owner_input(
    buffer: &Mutex<WakeBuffer>, cancel: &AtomicBool, found: &crate::wake_phrase::Match,
) -> KeywordOwnerInput {
    if cancel.load(Ordering::SeqCst) { return KeywordOwnerInput::Unavailable; }
    let (Some(segment),Some(first),Some(last))=(found.start_seconds,found.keyword_token_start_seconds,found.audio_end_seconds)
        else { return KeywordOwnerInput::Unavailable; };
    if !segment.is_finite() || !first.is_finite() || !last.is_finite()
        || segment<0.0 || first<segment || first>last { return KeywordOwnerInput::Unavailable; }
    let bytes=|seconds:f32|(seconds as f64*16000.0).round() as u64*2;
    let keyword_end=bytes(last);
    {
        let buffer=buffer.lock();
        if bytes(first)<buffer.start || keyword_end>buffer.end { return KeywordOwnerInput::Unavailable; }
    }
    // Retain the existing 250 ms acoustic tail. A later body speaker cannot
    // extend this window to the old 1.8/2.4 s verification rungs.
    let end=keyword_end.saturating_add(250*32);
    if snapshot(buffer,cancel,keyword_end,end).is_none() { return KeywordOwnerInput::Unavailable; }
    let buffer=buffer.lock();
    if let Some(range)=found.keyword_identity_source_range(buffer.start,buffer.end) {
        return match buffer.range(range.start,range.end) {
            Ok(pcm)=>KeywordOwnerInput::Focused { start:range.start,end:range.end,pcm },
            Err(_)=>KeywordOwnerInput::Unavailable,
        };
    }
    // Preserve the old capability only for genuinely short, valid spans.
    // Missing, future, malformed or expired native clocks never use this path.
    let start=bytes(first).saturating_sub(200*32).max(bytes(segment)).max(buffer.start);
    if bytes(first)>=buffer.start && end<=buffer.end && start<end && end-start<32_000 {
        KeywordOwnerInput::LegacyShort
    } else { KeywordOwnerInput::Unavailable }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn found()->crate::wake_phrase::Match {
        crate::wake_phrase::Match { start_seconds:Some(0.0),keyword_token_start_seconds:Some(1.0),end_seconds:2.0,audio_end_seconds:Some(2.0),matched_keyword:Some("开始录音".into()),origin:crate::wake_phrase::KeywordEvidenceOrigin::PrimaryDetector }
    }
    #[test]
    fn keyword_identity_never_takes_earlier_owner_or_later_body_and_keeps_source() {
        let pcm=[vec![77;800*32],vec![11;1200*32],vec![0;300*32],vec![77;2000*32]].concat();
        let mut source=WakeBuffer::default();source.append(&pcm);
        let buffer=Mutex::new(source);
        let KeywordOwnerInput::Focused {start,end,pcm:input}=keyword_owner_input(&buffer,&AtomicBool::new(false),&found()) else {panic!("proven keyword input")};
        assert_eq!((start,end),(800*32,2250*32));
        assert!(!input.contains(&77),"earlier owner and owner >250ms into body are outside identity input");
        assert_eq!(buffer.lock().body_after(2000*32).unwrap(),pcm[2000*32..]);
        assert_eq!(buffer.lock().range(0,pcm.len()as u64).unwrap(),pcm);
    }
    #[test]
    fn keyword_identity_rejects_cancelled_unproven_future_and_expired_sources() {
        let mut source=WakeBuffer::default();source.append(&vec![0;3000*32]);let buffer=Mutex::new(source);
        assert!(matches!(keyword_owner_input(&buffer,&AtomicBool::new(true),&found()),KeywordOwnerInput::Unavailable));
        let mut missing=found();missing.keyword_token_start_seconds=None;
        assert!(matches!(keyword_owner_input(&buffer,&AtomicBool::new(false),&missing),KeywordOwnerInput::Unavailable));
        let mut future=found();future.audio_end_seconds=Some(4.0);
        assert!(matches!(keyword_owner_input(&buffer,&AtomicBool::new(false),&future),KeywordOwnerInput::Unavailable));
        buffer.lock().append(&vec![0;12000*32]);
        assert!(matches!(keyword_owner_input(&buffer,&AtomicBool::new(false),&found()),KeywordOwnerInput::Unavailable));
    }
    #[test]
    fn keyword_identity_retains_legacy_short_source_without_inventing_speech() {
        let mut source=WakeBuffer::default();source.append(&vec![0;1500*32]);let buffer=Mutex::new(source);
        let mut short=found();short.keyword_token_start_seconds=Some(0.2);short.audio_end_seconds=Some(0.6);
        assert!(matches!(keyword_owner_input(&buffer,&AtomicBool::new(false),&short),KeywordOwnerInput::LegacyShort));
    }
}
