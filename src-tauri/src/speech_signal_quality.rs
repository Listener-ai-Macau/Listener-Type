//! Continuous human-speech coverage for the quality of speaker observations.
//!
//! This ledger has no speaker identity. Unprocessed, active and expired audio
//! all count as possibly speech. It can disprove an energy-derived duration;
//! it cannot certify Owner/Other or authorize text.

use std::collections::VecDeque;

const SAMPLES_PER_MS: u64 = 16;
const RETAIN_SAMPLES: u64 = 60_000 * SAMPLES_PER_MS;
const MAX_COMPLETED_SPANS: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct SpeechSignalTimeline {
    known_from: u64,
    known_through: u64,
    completed_speech: VecDeque<(u64, u64)>,
    invalid: bool,
}

impl SpeechSignalTimeline {
    pub(crate) fn is_invalid(&self) -> bool { self.invalid }

    /// Positive quality for a fully observed interval. Unknown samples are
    /// excluded here; the upper bound used by legacy advisory checks cannot
    /// stand in for confirmed human speech in a new identity certificate.
    pub(crate) fn resolved_speech_ms(&self, start_ms: u64, end_ms: u64) -> Option<usize> {
        let start = start_ms.checked_mul(SAMPLES_PER_MS)?;
        let end = end_ms.checked_mul(SAMPLES_PER_MS)?;
        if self.invalid || start >= end || start < self.known_from || end > self.known_through { return None; }
        let speech = self.completed_speech.iter().map(|(a,b)|
            (*b).min(end).saturating_sub((*a).max(start))).sum::<u64>();
        usize::try_from(speech / SAMPLES_PER_MS).ok()
    }

    pub(crate) fn confirmed_speech_end_ms_in(&self, start_ms: u64, end_ms: u64) -> Option<u64> {
        if self.invalid { return None; }
        let (start,end) = (start_ms.checked_mul(SAMPLES_PER_MS)?,end_ms.checked_mul(SAMPLES_PER_MS)?);
        self.completed_speech.iter().filter_map(|(a,b)| {
            let speech_end = (*b).min(end);
            ((*a).max(start) < speech_end).then_some(speech_end / SAMPLES_PER_MS)
        }).max()
    }

    pub(crate) fn completed_speech_ranges_ms(&self) -> Vec<(u64,u64)> {
        if self.invalid { return Vec::new(); }
        self.completed_speech.iter().map(|(start,end)| (*start/SAMPLES_PER_MS,end.div_ceil(SAMPLES_PER_MS))).collect()
    }

    pub(crate) fn begin_at(&mut self, samples: u64) {
        self.known_from = self.known_from.max(samples);
        self.known_through = self.known_through.max(samples);
    }

    pub(crate) fn invalidate(&mut self) {
        self.invalid = true;
        self.completed_speech.clear();
    }

    /// Only the continuous detector may publish its completed, absolute PCM
    /// intervals. Late evidence in previously finalized silence invalidates
    /// this analyzer instead of retroactively asserting that earlier checks
    /// had an accurate bound.
    pub(crate) fn complete_speech(&mut self, start: u64, end: u64) {
        if self.invalid { return; }
        if start >= end || start < self.known_through {
            self.invalidate();
            return;
        }
        if let Some(last) = self.completed_speech.back_mut() {
            if start < last.0 { self.invalidate(); return; }
            if start <= last.1 {
                last.1 = last.1.max(end);
                return;
            }
        }
        self.completed_speech.push_back((start, end));
        while self.completed_speech.len() > MAX_COMPLETED_SPANS {
            if let Some((_, end)) = self.completed_speech.pop_front() {
                self.known_from = self.known_from.max(end);
            }
        }
    }

    /// The adapter must stop before any active/undecided interval and retain
    /// the detector's onset lookback. Advancing a frontier is never speech.
    pub(crate) fn advance_known_through(&mut self, samples: u64) {
        if self.invalid { return; }
        self.known_through = self.known_through.max(samples);
        self.known_from = self.known_from.max(samples.saturating_sub(RETAIN_SAMPLES));
        while self.completed_speech.front().is_some_and(|(_, end)| *end <= self.known_from) {
            self.completed_speech.pop_front();
        }
    }

    pub(crate) fn speech_upper_bound_ms(&self, start_ms: u64, end_ms: u64) -> Option<usize> {
        if self.invalid || start_ms >= end_ms { return None; }
        let start = start_ms.checked_mul(SAMPLES_PER_MS)?;
        let end = end_ms.checked_mul(SAMPLES_PER_MS)?;
        let covered_start = start.max(self.known_from);
        let covered_end = end.min(self.known_through);
        if covered_start >= covered_end { return None; }
        let covered_speech = self.completed_speech.iter().map(|(a, b)| {
            b.min(&covered_end).saturating_sub((*a).max(covered_start))
        }).sum::<u64>();
        let unknown = (end - start).saturating_sub(covered_end - covered_start);
        // Round upward: fractional samples must not strengthen a negative.
        usize::try_from((unknown + covered_speech).div_ceil(SAMPLES_PER_MS)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_pause_cannot_inherit_energy_duration() {
        let mut timeline = SpeechSignalTimeline::default();
        timeline.complete_speech(230 * 16, 5798 * 16);
        timeline.advance_known_through(6100 * 16);
        // Even if ALL 500 ms of unresolved audio is speech, this is <1 s.
        assert_eq!(timeline.speech_upper_bound_ms(5400, 6600), Some(898));
        timeline.advance_known_through(6600 * 16);
        assert_eq!(timeline.speech_upper_bound_ms(5400, 6600), Some(398));
        assert_eq!(timeline.speech_upper_bound_ms(3800, 5000), Some(1200));
    }

    #[test]
    fn pending_active_missing_and_expired_audio_remain_possible_speech() {
        let mut timeline = SpeechSignalTimeline::default();
        assert_eq!(timeline.speech_upper_bound_ms(0, 1200), None);
        timeline.begin_at(2000 * 16);
        assert_eq!(timeline.speech_upper_bound_ms(0, 1200), None);
        timeline.advance_known_through(5000 * 16);
        assert_eq!(timeline.speech_upper_bound_ms(5000, 6200), None);
        timeline.advance_known_through(70_000 * 16);
        assert_eq!(timeline.speech_upper_bound_ms(5400, 6600), None);
        timeline.invalidate();
        assert_eq!(timeline.speech_upper_bound_ms(69_000, 70_000), None);
    }

    #[test]
    fn completed_speech_merges_without_double_counting_or_rewriting_silence() {
        let mut timeline = SpeechSignalTimeline::default();
        timeline.complete_speech(1600, 16_000);
        timeline.complete_speech(15_000, 20_000);
        timeline.advance_known_through(20_000);
        assert_eq!(timeline.speech_upper_bound_ms(0, 1250), Some(1150));
        timeline.complete_speech(18_000, 22_000);
        assert_eq!(timeline.speech_upper_bound_ms(0, 1250), None);
    }
}
