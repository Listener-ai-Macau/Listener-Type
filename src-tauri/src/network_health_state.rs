//! Reachability observations are ordered by when they began, not when DNS returned.
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkHealthClass {
    AllGood,
    AsrUnreachable,
    GeneralUnreachable,
    AllBad,
}

#[derive(Default)]
pub struct NetworkHealthState {
    asr: Option<(Instant, bool)>,
    general: Option<(Instant, bool)>,
    pub revision: u64,
}

impl NetworkHealthState {
    pub const fn new() -> Self { Self { asr: None, general: None, revision: 0 } }

    pub fn class(&self) -> Option<NetworkHealthClass> {
        match (self.asr.map(|v| v.1), self.general.map(|v| v.1)) {
            (None, None) => None,
            (Some(true), Some(false)) => Some(NetworkHealthClass::GeneralUnreachable),
            (Some(true), _) => Some(NetworkHealthClass::AllGood),
            (_, Some(true)) => Some(NetworkHealthClass::AsrUnreachable),
            _ => Some(NetworkHealthClass::AllBad),
        }
    }

    pub fn observe(&mut self, at: Instant, asr: Option<bool>, general: Option<bool>) -> bool {
        let before = self.class();
        for (current, observation) in [(&mut self.asr, asr), (&mut self.general, general)] {
            if let Some(ok) = observation {
                if current.is_none_or(|(previous, _)| at >= previous) {
                    *current = Some((at, ok));
                }
            }
        }
        let changed = before != self.class();
        if changed {
            self.revision += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn actual_service_response_clears_old_outage_without_claiming_control_site_recovered() {
        let mut state = NetworkHealthState::default();
        let t = Instant::now();
        assert!(state.observe(t, Some(false), Some(false)));
        assert_eq!(state.class(), Some(NetworkHealthClass::AllBad));
        assert!(state.observe(t + Duration::from_millis(1), Some(true), None));
        assert_eq!(state.class(), Some(NetworkHealthClass::GeneralUnreachable));
        assert_eq!(state.general.unwrap().1, false);
        assert_eq!(state.revision, 2);
    }

    #[test]
    fn old_probe_cannot_turn_success_back_into_outage_or_overwrite_newer_probe() {
        let mut state = NetworkHealthState::default();
        let t = Instant::now();
        state.observe(t + Duration::from_secs(2), Some(true), None);
        state.observe(t, Some(false), Some(false));
        assert_eq!(state.class(), Some(NetworkHealthClass::GeneralUnreachable));
        state.observe(t + Duration::from_secs(3), Some(true), Some(true));
        assert!(!state.observe(t + Duration::from_secs(1), Some(false), Some(false)));
        assert_eq!(state.class(), Some(NetworkHealthClass::AllGood));
    }

    #[test]
    fn recovery_does_not_mask_a_subsequent_real_outage() {
        let mut state = NetworkHealthState::default();
        let t = Instant::now();
        state.observe(t, Some(true), None);
        assert!(state.observe(t + Duration::from_secs(1), Some(false), Some(true)));
        assert_eq!(state.class(), Some(NetworkHealthClass::AsrUnreachable));
        state.observe(t + Duration::from_secs(2), Some(false), Some(false));
        assert_eq!(state.class(), Some(NetworkHealthClass::AllBad));
    }
}
