//! One current local timestamp for question answering; no inferred calendar facts.
use chrono::{DateTime, FixedOffset, Local, SecondsFormat};

pub(super) fn current_calendar_context() -> String {
    time_context_at(Local::now().fixed_offset())
}

fn time_context_at(now: DateTime<FixedOffset>) -> String {
    format!("Current time: {}", now.to_rfc3339_opts(SecondsFormat::Secs, false))
}
