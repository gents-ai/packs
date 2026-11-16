//! UTC timestamps and whole-day dates without a time library.
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    iso_utc(secs)
}

/// `2026-10-29T17:39:08Z` for Unix seconds (civil-from-days, proleptic Gregorian).
pub fn iso_utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 of a text that starts with `YYYY-MM-DD` (a date or
/// an ISO-8601 timestamp; the time of day is ignored).
pub fn days(text: &str) -> Option<i64> {
    let date = text.get(..10)?;
    let rest = &text[10..];
    let b = date.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || !(rest.is_empty() || rest.starts_with('T')) {
        return None;
    }
    let year: i64 = date[..4].parse().ok()?;
    let month: i64 = date[5..7].parse().ok()?;
    let day: i64 = date[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // days-from-civil
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_and_days_agree() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(days("1970-01-01"), Some(0));
        assert_eq!(days("2000-02-29T00:00:00Z"), Some(951_782_400 / 86_400));
        assert_eq!(days("2026-10-29T12:00:00Z").unwrap() - days("2026-10-22").unwrap(), 7);
        for bad in ["", "2026", "2026/10/29", "2026-13-01", "2026-10-29 x", "yyyy-mm-dd"] {
            assert_eq!(days(bad), None, "{bad}");
        }
    }
}
