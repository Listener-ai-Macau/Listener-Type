// A detached device takeover must not outlive failed/cancelled ASR startup.
// Dropping this scope cancels a still-queued write immediately; the detached
// observer also retires an already-sent request using its original request ID.
struct AcceptedWakeCaptureStartup {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    decision: Option<tokio::sync::oneshot::Sender<()>>,
}

impl AcceptedWakeCaptureStartup {
    fn new() -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (decision, outcome) = tokio::sync::oneshot::channel();
        (Self {
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            decision: Some(decision),
        }, outcome)
    }

    fn cancellation(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    fn commit(mut self) {
        if let Some(decision) = self.decision.take() {
            let _ = decision.send(());
        }
    }
}

impl Drop for AcceptedWakeCaptureStartup {
    fn drop(&mut self) {
        if self.decision.is_some() {
            self.cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

#[cfg(test)]
mod accepted_wake_capture_startup_tests {
    use super::*;

    #[tokio::test]
    async fn failed_startup_cancels_queued_takeover_and_notifies_its_observer() {
        let (startup, outcome) = AcceptedWakeCaptureStartup::new();
        let cancelled = startup.cancellation();
        // Represents any early return while preparing the actual ASR session.
        drop(startup);
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
        assert!(outcome.await.is_err());
    }

    #[tokio::test]
    async fn successful_startup_leaves_takeover_live_and_later_failure_is_scoped() {
        let (startup, outcome) = AcceptedWakeCaptureStartup::new();
        let first_cancelled = startup.cancellation();
        startup.commit();
        assert!(outcome.await.is_ok());
        let (later_startup, later_outcome) = AcceptedWakeCaptureStartup::new();
        let later_cancelled = later_startup.cancellation();
        drop(later_startup);
        assert!(later_outcome.await.is_err());
        assert!(later_cancelled.load(std::sync::atomic::Ordering::Acquire));
        assert!(!first_cancelled.load(std::sync::atomic::Ordering::Acquire));
    }
}
