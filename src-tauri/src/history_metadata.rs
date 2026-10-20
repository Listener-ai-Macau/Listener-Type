//! Saved media duration is independent of ASR/network processing time.

pub(crate) fn capture_duration_ms(
    provider_duration_ms: Option<u64>,
    stats: Option<&crate::embedded_audio::SessionStats>,
) -> Option<u64> {
    stats
        .filter(|stats| stats.reconstructed_pcm_bytes > 0)
        .map(|stats| {
            (stats.reconstructed_pcm_bytes as u64).saturating_mul(1_000)
                / crate::embedded_audio::PCM_BYTES_PER_SECOND as u64
        })
        .or(provider_duration_ms)
}

#[cfg(test)]
mod tests {
    #[test]
    fn history_media_duration_uses_source_bytes() {
        use super::capture_duration_ms;
        let mut stats = crate::embedded_audio::collect_notifications(std::iter::empty::<&[u8]>())
            .expect("empty collector").stats();
        assert_eq!(capture_duration_ms(Some(2_527), None), Some(2_527));
        assert_eq!(capture_duration_ms(None, None), None);
        assert_eq!(capture_duration_ms(Some(2_527), Some(&stats)), Some(2_527));
        stats.reconstructed_pcm_bytes = 926_498;
        stats.duration_seconds = 999.0;
        assert_eq!(capture_duration_ms(Some(2_527), Some(&stats)), Some(28_953));
        assert_eq!(capture_duration_ms(Some(120_000), Some(&stats)), Some(28_953));
        assert_eq!(capture_duration_ms(None, Some(&stats)), Some(28_953));
        stats.reconstructed_pcm_bytes = 32_000;
        assert_eq!(capture_duration_ms(Some(8_000), Some(&stats)), Some(1_000));
    }
}
