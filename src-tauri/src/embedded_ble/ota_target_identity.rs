//! An OTA handoff belongs to one live board, independently of cached names.
pub(super) fn matches_handoff(expected: Option<u64>, candidate: Option<u64>) -> bool {
    match expected {
        Some(address) => address != 0 && candidate == Some(address),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::matches_handoff;
    #[test]
    fn shared_service_uuid_does_not_allow_another_product_or_board() {
        assert!(!matches_handoff(Some(0xE1B2075312B8), Some(0xC8ED45EB95B4)));
        assert!(!matches_handoff(Some(0xE1B2075312B8), Some(0xF3371BC799AF)));
    }
    #[test]
    fn same_identity_remains_eligible_despite_rename_cache_lag() {
        assert!(matches_handoff(Some(0xE1B2075312B8), Some(0xE1B2075312B8)));
    }
    #[test]
    fn owned_handoff_rejects_missing_or_zero_identity() {
        assert!(!matches_handoff(Some(0xE1B2075312B8), None));
        assert!(!matches_handoff(Some(0), Some(0)));
    }
    #[test]
    fn unowned_discovery_keeps_its_existing_name_and_trust_policy() {
        assert!(matches_handoff(None, Some(0xE1B2075312B8)));
        assert!(matches_handoff(None, None));
    }
}
