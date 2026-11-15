//! Wall-clock helpers without a date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// UTC calendar date `YYYY-MM-DD` of a Unix timestamp in milliseconds.
pub fn format_utc_date(unix_ms: u64) -> String {
    let (y, m, d) = civil_from_days((unix_ms / 86_400_000) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// UTC time `YYYY-MM-DD HH:MM UTC` of a Unix timestamp in milliseconds.
pub fn format_utc_minute(unix_ms: u64) -> String {
    let secs_of_day = (unix_ms / 1000) % 86_400;
    format!(
        "{} {:02}:{:02} UTC",
        format_utc_date(unix_ms),
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// True for `YYYY-MM-DD` with a plausible month and day.
pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc_date(0), "1970-01-01");
        // 2026-11-15 13:45:10 UTC
        let ms = 1_794_750_310_000;
        assert_eq!(format_utc_date(ms), "2026-11-15");
        assert_eq!(format_utc_minute(ms), "2026-11-15 13:45 UTC");
        // Leap day.
        assert_eq!(format_utc_date(1_709_164_800_000), "2024-02-29");
    }

    #[test]
    fn validates_dates() {
        assert!(is_iso_date("2026-11-13"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("2026-1-01"));
        assert!(!is_iso_date("20261101"));
    }
}
