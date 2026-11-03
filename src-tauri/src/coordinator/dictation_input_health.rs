// Numeric observations only. This clock owns no audio and has no endpoint or
// speaker authority; late source observations cannot renew its logging budget.
#[derive(Default)]
struct InputHealthClock {
    last_source_ms: Option<u64>,
}

impl InputHealthClock {
    fn should_emit(&mut self, audio_end_ms: u64) -> bool {
        if self.last_source_ms.is_some_and(|last| audio_end_ms.saturating_sub(last) < 1_000) {
            return false;
        }
        self.last_source_ms = Some(audio_end_ms);
        true
    }
}

#[cfg(test)]
mod input_health_tests {
    use super::*;

    #[test]
    fn input_health_is_bounded_and_cannot_be_refreshed_by_old_audio() {
        let mut clock = InputHealthClock::default();
        assert!(clock.should_emit(100));
        for end in 101..1_100 { assert!(!clock.should_emit(end)); }
        assert!(clock.should_emit(1_100));
        assert!(!clock.should_emit(100));
        assert!(!clock.should_emit(2_099));
        assert!(clock.should_emit(2_100));
    }

    #[test]
    fn input_health_first_observation_is_independent_per_session() {
        let mut old = InputHealthClock::default();
        assert!(old.should_emit(40_000));
        let mut new = InputHealthClock::default();
        assert!(new.should_emit(100));
        assert!(!old.should_emit(100));
    }
}
