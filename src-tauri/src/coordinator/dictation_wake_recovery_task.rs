// The live PCM actor must poll completion of the entire recovery, not just
// separation. Phrase/owner inference after separation can also take seconds.
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
struct WakeRecoveryTask<T> {
    extraction_ready: tokio::sync::oneshot::Receiver<()>,
    task: Option<tauri::async_runtime::JoinHandle<Result<T, String>>>,
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
impl<T: Send + 'static> WakeRecoveryTask<T> {
    fn spawn<E, F, V, R>(extract: F, evaluate: V) -> Self
    where
        E: Send + 'static,
        F: FnOnce() -> Result<E, String> + Send + 'static,
        V: FnOnce(E) -> R + Send + 'static,
        R: std::future::Future<Output = Result<T, String>> + Send + 'static,
    {
        let (ready, extraction_ready) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let extracted = tauri::async_runtime::spawn_blocking(extract)
                .await
                .map_err(|error| format!("wake extraction task failed: {error}"));
            let _ = ready.send(());
            evaluate(extracted??).await
        });
        Self { extraction_ready, task: Some(task) }
    }

    fn is_finished(&self) -> bool {
        self.task.as_ref().is_some_and(|task| task.inner().is_finished())
    }

    async fn result(mut self) -> Result<T, String> {
        self.task.take().expect("wake recovery task")
            .await.map_err(|error| format!("wake recovery task failed: {error}"))?
    }

    async fn terminal_result(mut self, extraction_wait_ms: u64) -> Result<T, String> {
        // Preserve the original separation-only terminal budget. Evaluation
        // already overlaps live PCM; including it in this budget would reject
        // valid recoveries whose separation finished just before the deadline.
        if !self.is_finished() {
            tokio::time::timeout(
                Duration::from_millis(extraction_wait_ms), &mut self.extraction_ready,
            ).await.map_err(|_| "wake_extraction_budget_exceeded".to_string())?
                .map_err(|_| "wake extraction readiness closed".to_string())?;
        }
        self.result().await
    }
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
impl<T> Drop for WakeRecoveryTask<T> {
    fn drop(&mut self) {
        // A native inference already running cannot be interrupted here. Its
        // abandoned result must not start another helper/voiceprint request.
        if let Some(task) = self.task.take() { task.abort(); }
    }
}

#[cfg(all(test, target_os = "windows", feature = "target-speaker-extraction"))]
#[path = "dictation_wake_recovery_task_tests.rs"]
mod wake_recovery_task_tests;
