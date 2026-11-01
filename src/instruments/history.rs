//! Date-range handling for historical candles. Kite caps how many days one request may
//! span per interval, so long ranges are split into consecutive windows.

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};

use crate::error::{KiteError, Result};

pub const INTERVALS: &[&str] = &[
    "minute", "3minute", "5minute", "10minute", "15minute", "30minute", "60minute", "day",
];

const FMT: &str = "%Y-%m-%d %H:%M:%S";

/// Maximum days per request for an interval.
pub fn max_days(interval: &str) -> Result<i64> {
    Ok(match interval {
        "minute" => 60,
        "3minute" | "5minute" | "10minute" => 100,
        "15minute" | "30minute" => 200,
        "60minute" => 400,
        "day" => 2000,
        other => {
            return Err(KiteError::InvalidInput(format!(
                "unknown interval `{other}`; use one of {}",
                INTERVALS.join(", ")
            )));
        }
    })
}

/// Parses `YYYY-MM-DD` or `YYYY-MM-DD HH:MM[:SS]` (IST). A bare date means start of day,
/// or end of day when `end_of_day` is set.
pub fn parse_datetime(s: &str, end_of_day: bool) -> Result<NaiveDateTime> {
    let s = s.trim().replace('T', " ");
    if let Ok(dt) = NaiveDateTime::parse_from_str(&s, FMT) {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        let t = if end_of_day {
            NaiveTime::from_hms_opt(23, 59, 59)
        } else {
            NaiveTime::from_hms_opt(0, 0, 0)
        };
        return Ok(d.and_time(t.expect("valid time")));
    }
    Err(KiteError::InvalidInput(format!(
        "bad date `{s}`; use YYYY-MM-DD or YYYY-MM-DD HH:MM:SS"
    )))
}

/// Splits `[from, to]` into windows no longer than the interval's limit.
/// Returns `(from, to)` pairs formatted for the API.
pub fn windows(
    interval: &str,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> Result<Vec<(String, String)>> {
    if from > to {
        return Err(KiteError::InvalidInput("`from` is after `to`".into()));
    }
    let span = Duration::days(max_days(interval)?);
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = (start + span - Duration::seconds(1)).min(to);
        out.push((start.format(FMT).to_string(), end.format(FMT).to_string()));
        start = end + Duration::seconds(1);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dates() {
        assert_eq!(
            parse_datetime("2026-01-05", false).unwrap().to_string(),
            "2026-01-05 00:00:00"
        );
        assert_eq!(
            parse_datetime("2026-01-05", true).unwrap().to_string(),
            "2026-01-05 23:59:59"
        );
        assert_eq!(
            parse_datetime("2026-01-05 09:15", false)
                .unwrap()
                .to_string(),
            "2026-01-05 09:15:00"
        );
        assert!(parse_datetime("05/01/2026", false).is_err());
    }

    #[test]
    fn single_window_when_short() {
        let w = windows(
            "day",
            parse_datetime("2026-01-01", false).unwrap(),
            parse_datetime("2026-03-01", true).unwrap(),
        )
        .unwrap();
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn splits_minute_ranges_into_60_day_windows() {
        let from = parse_datetime("2026-01-01", false).unwrap();
        let to = parse_datetime("2026-06-30", true).unwrap();
        let w = windows("minute", from, to).unwrap();
        assert_eq!(w.len(), 4); // 181 days / 60
        assert_eq!(w[0].0, "2026-01-01 00:00:00");
        assert_eq!(w[0].1, "2026-03-01 23:59:59");
        assert_eq!(w[1].0, "2026-03-02 00:00:00");
        assert_eq!(w.last().unwrap().1, "2026-06-30 23:59:59");
    }

    #[test]
    fn rejects_bad_interval_and_order() {
        let a = parse_datetime("2026-01-02", false).unwrap();
        let b = parse_datetime("2026-01-01", false).unwrap();
        assert!(windows("2minute", b, a).is_err());
        assert!(windows("day", a, b).is_err());
    }
}
