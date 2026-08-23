impl EmbeddedStreamingDictation {
    /// Complete deferred StreamingDetector::new if the init task has finished.
    /// Returns false while still loading (PCM continues to buffer).
    async fn poll_wake_detector_init(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let candidate = match self.speaker_candidate.as_mut() {
            Some(candidate) => candidate,
            None => return Ok(false),
        };
        if candidate.wake_detector.is_some() {
            return Ok(true);
        }
        let Some(handle) = candidate.wake_detector_init.as_ref() else {
            return Ok(false);
        };
        if !handle.inner().is_finished() {
            return Ok(false);
        }
        let handle = candidate
            .wake_detector_init
            .take()
            .expect("wake_detector_init present");
        match handle.await {
            Ok(Ok(detector)) => {
                candidate.wake_detector = Some(detector);
                log::info!(
                    "[wake-phrase] deferred streaming detector ready embedded_session_id={embedded_session_id} buffered_pcm_ms={}",
                    candidate.pcm.len() / 32
                );
                Ok(true)
            }
            Ok(Err(err)) => {
                log::warn!(
                    "[wake-phrase] deferred streaming detector init failed embedded_session_id={embedded_session_id}: {err}"
                );
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                if let Some(sid) = take_early_capsule_session_id(candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                Ok(false)
            }
            Err(err) => {
                log::warn!(
                    "[wake-phrase] deferred streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                );
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                if let Some(sid) = take_early_capsule_session_id(candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                Ok(false)
            }
        }
    }
}

