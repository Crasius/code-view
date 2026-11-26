use time::OffsetDateTime;

/// "just now", "5 min ago", "3 h ago", "2 d ago", or a date for anything older than a month.
pub fn relative(moment: OffsetDateTime) -> String {
    relative_to(moment, OffsetDateTime::now_utc())
}

pub fn relative_to(moment: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (now - moment).whole_seconds();
    match seconds {
        ..60 => "just now".into(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        86_400..2_592_000 => format!("{} d ago", seconds / 86_400),
        _ => format!("{:04}-{:02}-{:02}", moment.year(), u8::from(moment.month()), moment.day()),
    }
}

/// "19 Oct 2026, 00:41 UTC": the exact moment behind a relative time, for its tooltip.
pub fn exact(moment: OffsetDateTime) -> String {
    let utc = moment.to_offset(time::UtcOffset::UTC);
    let month = &format!("{:?}", utc.month())[..3];
    format!("{} {month} {}, {:02}:{:02} UTC", utc.day(), utc.year(), utc.hour(), utc.minute())
}

/// "1h 4m", "14m 25s", "12s": short enough for table columns, the same everywhere.
pub fn duration(start: OffsetDateTime, end: OffsetDateTime) -> String {
    let seconds = (end - start).whole_seconds().max(0);
    match seconds {
        ..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60),
    }
}

pub fn from_unix(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn formats_relative_times() {
        let now = datetime!(2026-10-18 12:00 UTC);
        assert_eq!(relative_to(datetime!(2026-10-18 11:59:30 UTC), now), "just now");
        assert_eq!(relative_to(datetime!(2026-10-18 11:15 UTC), now), "45 min ago");
        assert_eq!(relative_to(datetime!(2026-10-18 09:00 UTC), now), "3 h ago");
        assert_eq!(relative_to(datetime!(2026-10-16 12:00 UTC), now), "2 d ago");
        assert_eq!(relative_to(datetime!(2026-01-02 12:00 UTC), now), "2026-01-02");
    }

    #[test]
    fn formats_the_exact_moment_in_utc() {
        assert_eq!(exact(datetime!(2026-10-19 02:41 +2)), "19 Oct 2026, 00:41 UTC");
    }

    #[test]
    fn formats_durations() {
        let start = datetime!(2026-10-18 12:00 UTC);
        assert_eq!(duration(start, datetime!(2026-10-18 12:14:25 UTC)), "14m 25s");
        assert_eq!(duration(start, datetime!(2026-10-18 12:00:07 UTC)), "7s");
        assert_eq!(duration(start, datetime!(2026-10-18 13:04 UTC)), "1h 4m");
    }
}
