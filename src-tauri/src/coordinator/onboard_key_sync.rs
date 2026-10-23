//! Read-only, connection-owned recovery of the board's host-action profile.
//! A transient read failure must not require a reconnect or opening Settings.
use std::future::Future;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(250);
const IDLE_SETTLE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Readiness {
    Cancelled,
    Busy,
    Idle,
}

/// All waits and device access are injected so the same production driver can
/// be exercised without a radio, preferences, clipboard or the input desktop.
pub(super) async fn synchronize<T, E, C, S, SF, G, L, LF, A, D>(
    clock: C,
    sleep: S,
    readiness: G,
    mut load: L,
    mut accept: A,
    mut deferred: D,
) where
    C: Fn() -> Instant,
    S: Fn(Duration) -> SF,
    SF: Future<Output = ()>,
    G: Fn() -> Readiness,
    L: FnMut() -> LF,
    LF: Future<Output = Result<T, E>>,
    A: FnMut(T),
    D: FnMut(E, Duration),
{
    let mut idle_since = None;
    let mut next_attempt = clock();
    let mut failures = 0u32;
    loop {
        match readiness() {
            Readiness::Cancelled => return,
            Readiness::Busy => idle_since = None,
            Readiness::Idle => {
                let now = clock();
                let since = *idle_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= IDLE_SETTLE && now >= next_attempt {
                    match load().await {
                        Ok(value) => {
                            // In-flight native work cannot be forcibly cancelled.
                            // Never publish its result into a replacement link.
                            if readiness() != Readiness::Cancelled { accept(value); }
                            return;
                        }
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            let delay = Duration::from_secs((1u64 << failures.min(5)).min(30));
                            next_attempt = clock() + delay;
                            deferred(error, delay);
                        }
                    }
                }
            }
        }
        // Keep one task per link, not queued read requests. A long recording
        // has no expiry: it yields the radio and resumes only after quiet.
        sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::future::{ready, Future};
    use std::task::{Context, Poll, Waker};

    fn run(future: impl Future<Output = ()>) {
        let mut future = std::pin::pin!(future);
        assert!(matches!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(())));
    }

    #[test]
    fn transient_profile_read_recovers_without_reconnect_or_settings() {
        let base = Instant::now();
        let now = Cell::new(base);
        let reads = Cell::new(0);
        let accepted = Cell::new(None);
        let retries = RefCell::new(Vec::new());
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || Readiness::Idle,
            || { let attempt = reads.get() + 1; reads.set(attempt); ready(if attempt < 3 { Err("temporary read error") } else { Ok(37) }) },
            |value| accepted.set(Some(value)),
            |_, delay| retries.borrow_mut().push(delay),
        ));
        assert_eq!(accepted.get(), Some(37), "a one-shot read leaves the connected keyboard without its stored host actions");
        assert_eq!(reads.get(), 3);
        assert_eq!(*retries.borrow(), vec![Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn recording_longer_than_thirty_seconds_defers_instead_of_abandoning() {
        let base = Instant::now();
        let now = Cell::new(base);
        let read_at = Cell::new(None);
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || if now.get() < base + Duration::from_secs(90) { Readiness::Busy } else { Readiness::Idle },
            || { read_at.set(Some(now.get().duration_since(base))); ready(Ok::<_, ()>(())) },
            |_| (),
            |_, _| panic!("no I/O should be issued during the recording"),
        ));
        assert_eq!(read_at.get(), Some(Duration::from_secs(92)));
    }

    #[test]
    fn connection_change_during_read_discards_the_old_result() {
        let base = Instant::now();
        let now = Cell::new(base);
        let cancelled = Cell::new(false);
        let accepted = Cell::new(false);
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || if cancelled.get() { Readiness::Cancelled } else { Readiness::Idle },
            || { cancelled.set(true); ready(Ok::<_, ()>(())) },
            |_| accepted.set(true),
            |_, _| (),
        ));
        assert!(!accepted.get());
    }

    #[test]
    fn cancellation_during_backoff_never_reads_a_new_connection() {
        let base = Instant::now();
        let now = Cell::new(base);
        let attempts = Cell::new(0);
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || if now.get() >= base + Duration::from_secs(3) { Readiness::Cancelled } else { Readiness::Idle },
            || { attempts.set(attempts.get() + 1); ready(Err::<(), _>("temporary")) },
            |_| panic!("failed value must not be cached"),
            |_, _| (),
        ));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn repeated_failures_are_paced_and_backoff_is_capped() {
        let base = Instant::now();
        let now = Cell::new(base);
        let reads = RefCell::new(Vec::new());
        let delays = RefCell::new(Vec::new());
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || if now.get() > base + Duration::from_secs(180) { Readiness::Cancelled } else { Readiness::Idle },
            || { reads.borrow_mut().push(now.get()); ready(Err::<(), _>("offline")) },
            |_| (),
            |_, delay| delays.borrow_mut().push(delay),
        ));
        assert!((6..=10).contains(&reads.borrow().len()));
        assert!(delays.borrow().iter().all(|d| *d <= Duration::from_secs(30)));
        assert!(delays.borrow().contains(&Duration::from_secs(30)));
        assert!(reads.borrow().windows(2).all(|w| w[1].duration_since(w[0]) >= Duration::from_secs(2)));
    }

    #[test]
    fn legacy_success_without_a_profile_finishes_once() {
        let base = Instant::now();
        let now = Cell::new(base);
        let reads = Cell::new(0);
        let accepted = Cell::new(false);
        run(synchronize(
            || now.get(),
            |duration| { now.set(now.get() + duration); ready(()) },
            || Readiness::Idle,
            || { reads.set(reads.get() + 1); ready(Ok::<Option<()>, ()>(None)) },
            |value| { assert_eq!(value, None); accepted.set(true) },
            |_, _| (),
        ));
        assert!(accepted.get());
        assert_eq!(reads.get(), 1);
    }
}
