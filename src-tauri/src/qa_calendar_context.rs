//! Current local calendar facts for question answering, sampled per request.
//! This never parses a user's words or replaces model output.
use chrono::{DateTime, Days, FixedOffset, Local};

pub(super) fn current_calendar_context() -> String {
    calendar_context_at(Local::now().fixed_offset())
}

fn calendar_context_at(now: DateTime<FixedOffset>) -> String {
    let today=now.date_naive();
    let day=|date:Option<chrono::NaiveDate>| date.map(|d| d.format("%Y-%m-%d %A").to_string()).unwrap_or_else(|| "outside supported calendar".into());
    format!("Current local date: {} (UTC{}). Tomorrow: {}.",day(Some(today)),now.format("%:z"),day(today.checked_add_days(Days::new(1))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_calendar_resolves_the_reported_monday_question() {
        let text=calendar_context_at(DateTime::parse_from_rfc3339("2026-10-26T10:51:00+08:00").unwrap());
        assert!(text.contains("Current local date: 2026-10-26 Monday"));
        assert!(text.contains("Tomorrow: 2026-10-27 Tuesday"));
        assert!(text.len()<130);
        assert!(!text.contains("10:51"));
    }
    #[test]
    fn same_local_day_does_not_change_prompt_or_accumulate_context() {
        let a=DateTime::parse_from_rfc3339("2026-10-26T00:00:00+08:00").unwrap();
        let b=DateTime::parse_from_rfc3339("2026-10-26T23:59:59+08:00").unwrap();
        assert_eq!(calendar_context_at(a),calendar_context_at(b));
        assert_eq!(calendar_context_at(a).matches("Current local date:").count(),1);
    }
    #[test]
    fn local_midnight_year_and_leap_day_use_calendar_not_24_hour_offsets() {
        for (now,today,tomorrow) in [
            ("2026-12-31T23:59:59-12:00","2026-12-31 Thursday","2027-01-01 Friday"),
            ("2024-02-28T00:00:00+14:00","2024-02-28 Wednesday","2024-02-29 Thursday"),
            ("2024-02-29T23:59:59+08:00","2024-02-29 Thursday","2024-03-01 Friday"),
        ] {
            let text=calendar_context_at(DateTime::parse_from_rfc3339(now).unwrap());
            assert!(text.contains(&format!("Current local date: {today}")));
            assert!(text.contains(&format!("Tomorrow: {tomorrow}")));
            assert!(text.contains(&now[now.len()-6..]));
        }
    }
}
