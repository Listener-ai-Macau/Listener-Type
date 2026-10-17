//! Native observations belong to an exact capture, enrollment and PCM input.
//! A late pre-reset worker may finish after reset; its result must never be
//! eligible for another capture merely because audio coordinates are reused.

use sha2::{Digest, Sha256};
use std::collections::{btree_map::Entry, BTreeMap};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ObservationKey {
    scope: (u64, u64),
    profile: [u8; 32],
    start_ms: u64,
    end_ms: u64,
    pcm_sha256: [u8; 32],
}

pub(super) struct NativeObservationCache<T> {
    values: BTreeMap<ObservationKey, T>,
}

impl<T> Default for NativeObservationCache<T> {
    fn default() -> Self { Self { values: BTreeMap::new() } }
}

impl<T> NativeObservationCache<T> {
    pub(super) fn clear(&mut self) { self.values.clear(); }

    pub(super) fn observe(
        &mut self,
        scope: (u64, u64),
        profile: [u8; 32],
        start_ms: u64,
        end_ms: u64,
        pcm: &[u8],
        infer: impl FnOnce(&[u8]) -> Result<T, String>,
    ) -> Result<&T, String> {
        let expected = end_ms.checked_sub(start_ms)
            .filter(|duration| *duration > 0)
            .and_then(|duration| usize::try_from(duration).ok())
            .and_then(|duration| duration.checked_mul(32));
        if expected != Some(pcm.len()) {
            return Err("native identity cache requires exact complete PCM coordinates".into());
        }
        let key = ObservationKey { scope, profile, start_ms, end_ms,
            pcm_sha256: Sha256::digest(pcm).into() };
        Ok(match self.values.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(infer(pcm)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn late_old_capture_work_after_reset_cannot_satisfy_new_capture_observation() {
        let mut cache = NativeObservationCache::default();
        let old = vec![1; 32000];
        let new = vec![2; 32000];
        let calls = Cell::new(0);
        cache.clear();
        // Old work queued before reset starts after the cache was cleared.
        assert_eq!(*cache.observe((11, 1), [3;32], 0, 1000, &old,
            |pcm| { calls.set(calls.get()+1); Ok(pcm[0]) }).unwrap(), 1);
        assert_eq!(*cache.observe((12, 2), [3;32], 0, 1000, &new,
            |pcm| { calls.set(calls.get()+1); Ok(pcm[0]) }).unwrap(), 2);
        assert_eq!(calls.get(), 2);
        // Even byte-identical replay still receives its own capture binding.
        cache.observe((13, 3), [3;32], 0, 1000, &new,
            |_| { calls.set(calls.get()+1); Ok(2) }).unwrap();
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn same_coordinates_require_same_pcm_and_current_enrollment_before_cache_reuse() {
        let mut cache = NativeObservationCache::default();
        let calls = Cell::new(0);
        let a = vec![1; 32000];
        let b = vec![2; 32000];
        let mut observe = |profile, pcm: &[u8]| *cache.observe((20, 1), profile, 1000, 2000, pcm,
            |input| { calls.set(calls.get()+1); Ok(input[0]) }).unwrap();
        assert_eq!(observe([1;32], &a), 1);
        assert_eq!(observe([1;32], &a), 1);
        assert_eq!(calls.get(), 1, "spelling-only revisions reuse acoustic work");
        assert_eq!(observe([1;32], &b), 2);
        assert_eq!(observe([2;32], &b), 2);
        assert_eq!(calls.get(), 3, "different audio or enrollment cannot borrow an observation");
    }

    #[test]
    fn incomplete_pcm_or_failed_inference_does_not_install_a_reusable_observation() {
        let mut cache = NativeObservationCache::<u8>::default();
        let pcm = vec![1; 32000];
        assert!(cache.observe((1,1),[1;32],0,1000,&pcm[..31999], |_| panic!("invalid source reached observer")).is_err());
        assert!(cache.observe((1,1),[1;32],1000,1000,&[], |_| panic!("empty source reached observer")).is_err());
        assert!(cache.observe((1,1),[1;32],0,1000,&pcm, |_| Err("inference unavailable".into())).is_err());
        assert_eq!(*cache.observe((1,1),[1;32],0,1000,&pcm, |_| Ok(7)).unwrap(),7);
    }
}
