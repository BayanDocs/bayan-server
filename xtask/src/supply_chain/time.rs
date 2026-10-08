//! Points in time as whole seconds since 1970-01-01 00:00 UTC ("Unix time"): read from the RFC 3339 timestamps that crates.io writes, and shown to people as dates.

use std::time::{SystemTime, UNIX_EPOCH};

/// The seconds in a day, which is also the minimum age of a dependency version (ADR-0017 rule 4).
pub const DAY: i64 = 86_400;

/// The current time.
pub fn now() -> Result<i64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "the system clock is set to a time before 1970".to_owned())?;
    i64::try_from(elapsed.as_secs()).map_err(|_| "the system clock is out of range".to_owned())
}

/// Reads an RFC 3339 timestamp, such as `2026-10-06T15:57:18Z`, `2026-08-28T12:22:30.892284Z` or `2026-10-06T17:57:18+02:00`. A fraction of a second rounds up to the next second, so that a publish time is never read as earlier than it was.
pub fn parse(text: &str) -> Result<i64, String> {
    let invalid = || format!("`{text}` is not an RFC 3339 timestamp");
    if !text.is_ascii() || text.len() < 20 {
        return Err(invalid());
    }
    let bytes = text.as_bytes();
    let separators = [(4, b"-"), (7, b"-"), (13, b":"), (16, b":")];
    if separators
        .iter()
        .any(|(at, separator)| bytes.get(*at) != Some(&separator[0]))
        || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
    {
        return Err(invalid());
    }
    let field = |start: usize, end: usize| digits(&text[start..end]).ok_or_else(invalid);
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(invalid());
    }
    let mut rest = &text[19..];
    let mut round_up = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 {
            return Err(invalid());
        }
        if fraction[..length].bytes().any(|digit| digit != b'0') {
            round_up = 1;
        }
        rest = &fraction[length..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return Err(invalid()),
            };
            let zone = &rest[1..];
            if zone.len() != 5 || zone.as_bytes()[2] != b':' {
                return Err(invalid());
            }
            let (hours, minutes) = (field_of(zone, 0, 2), field_of(zone, 3, 5));
            match (hours, minutes) {
                (Some(hours), Some(minutes)) if hours <= 23 && minutes <= 59 => {
                    sign * (hours * 3_600 + minutes * 60)
                }
                _ => return Err(invalid()),
            }
        }
    };
    Ok(
        days_from_civil(year, month, day) * DAY + hour * 3_600 + minute * 60 + second + round_up
            - offset,
    )
}

/// The number written by the ASCII digits `text[start..end]`, if they are all digits.
fn field_of(text: &str, start: usize, end: usize) -> Option<i64> {
    text.get(start..end).and_then(digits)
}

/// The number written by `text`, if it is one or more ASCII digits.
fn digits(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The days from 1970-01-01 to a date of the Gregorian calendar (the algorithm of Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of the day that lies `days` days after 1970-01-01, as (year, month, day); the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A point in time for people, to the minute: `2026-10-06 15:57 UTC`.
pub fn display(seconds: i64) -> String {
    let (year, month, day) = civil_from_days(seconds.div_euclid(DAY));
    let of_day = seconds.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

/// A length of time for people, rounded down to the minute: `10 h 41 min`, `27 d 3 h`.
pub fn duration(seconds: u64) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_timestamps_that_crates_io_writes() {
        // The index writes whole seconds, the API microseconds.
        assert_eq!(parse("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(parse("2026-10-06T15:57:18Z"), Ok(1_791_302_238));
        // A fraction rounds up, so that a version can never pass the age check up to a second early.
        assert_eq!(parse("2026-08-28T12:22:30.892284Z"), Ok(1_787_919_751));
        assert_eq!(parse("2026-08-28T12:22:30.000Z"), Ok(1_787_919_750));
        assert_eq!(
            parse("2026-10-06T17:57:18+02:00"),
            parse("2026-10-06T15:57:18Z")
        );
        assert_eq!(
            parse("2026-10-06T10:57:18-05:00"),
            parse("2026-10-06T15:57:18Z")
        );
        assert_eq!(parse("2000-02-29T23:59:59z"), Ok(951_868_799));
        assert_eq!(parse("1969-12-31T23:59:59Z"), Ok(-1));
    }

    #[test]
    fn rejects_what_is_not_a_timestamp() {
        for text in [
            "",
            "2026-10-06",
            "2026-10-06T15:57:18",
            "2026-10-06T15:57Z",
            "2026-13-06T15:57:18Z",
            "2026-02-30T15:57:18Z",
            "2025-02-29T15:57:18Z",
            "2026-10-06T24:00:00Z",
            "2026-10-06T15:57:18.Z",
            "2026-10-06T15:57:18+0200",
            "2026-10-06T15:57:18+24:00",
            "2026-10-06T15:57:18Zjunk",
            "2026/10/06T15:57:18Z",
            "+026-10-06T15:57:18Z",
            "2026-10-06T15:57:１８Z",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn converts_dates_both_ways() {
        for days in [-719_468, -1, 0, 1, 10_957, 11_016, 20_732, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days, "{days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn shows_times_and_durations_for_people() {
        assert_eq!(display(1_791_302_238), "2026-10-06 15:57 UTC");
        assert_eq!(display(-1), "1969-12-31 23:59 UTC");
        assert_eq!(duration(38_465), "10 h 41 min");
        assert_eq!(
            duration(DAY.unsigned_abs() * 27 + 3 * 3_600 + 59),
            "27 d 3 h"
        );
        assert_eq!(duration(59), "0 min");
        assert_eq!(duration(3_600), "1 h 0 min");
    }
}
