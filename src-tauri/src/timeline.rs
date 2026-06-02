use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static SEQ: AtomicU64 = AtomicU64::new(1);
static START: OnceLock<Instant> = OnceLock::new();

pub fn mark(source: &str, event: &str, detail: impl AsRef<str>) {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let app_ms = START.get_or_init(Instant::now).elapsed().as_millis();
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    log::info!(
        "[timeline] seq={seq} unix_ms={unix_ms} app_ms={app_ms} source={source} event={event} {}",
        detail.as_ref()
    );
}
