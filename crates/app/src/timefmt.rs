//! Time axis labels: board time as a clock reading since boot, and wall
//! clock time in UTC once the log's GPS records give a base for it.

use dflog::time::TimeBase;

/// Decimal places that write marks `step` apart exactly: none for a step
/// of a second or more, otherwise the fewest that make the step a whole
/// number of the last place, up to six.
fn decimals(step: f64) -> usize {
    if step.is_nan() || step <= 0.0 || step >= 1.0 {
        return 0;
    }
    // 0.1 -> 1, 0.25 -> 2, 0.05 -> 2, 0.001 -> 3
    (1..=6)
        .find(|&places| {
            let scaled = step * 10f64.powi(places);
            scaled.round() >= 1.0 && (scaled - scaled.round()).abs() < 1e-6 * scaled
        })
        .map_or(6, |places| places as usize)
}

/// `seconds` rounded to `places` decimals, so the split that follows
/// carries into the minute rather than printing 60 seconds.
fn rounded(seconds: f64, places: usize) -> f64 {
    let scale = 10f64.powi(i32::try_from(places).unwrap_or(6));
    (seconds * scale).round() / scale
}

/// Whole hours, minutes and seconds of `seconds`, with the fraction kept on
/// the seconds, and whether the value was negative.
fn split(seconds: f64) -> (bool, u64, u64, f64) {
    let negative = seconds < 0.0;
    let total = seconds.abs();
    let whole = total.floor();
    let fraction = total - whole;
    let whole = whole as u64;
    (
        negative,
        whole / 3600,
        (whole % 3600) / 60,
        (whole % 60) as f64 + fraction,
    )
}

/// Seconds since boot as `m:ss` or `h:mm:ss`, with as many decimals on the
/// seconds as marks `step` apart need.
#[must_use]
pub fn boot_time(seconds: f64, step: f64) -> String {
    if !seconds.is_finite() {
        return String::new();
    }
    let places = decimals(step);
    let (negative, hours, minutes, secs) = split(rounded(seconds, places));
    // "05" or "05.250": two digits before the point
    let width = if places == 0 { 2 } else { places + 3 };
    let sign = if negative { "-" } else { "" };
    if hours > 0 {
        format!("{sign}{hours}:{minutes:02}:{secs:0width$.places$}")
    } else {
        format!("{sign}{minutes}:{secs:0width$.places$}")
    }
}

/// Unix milliseconds as a UTC time of day, `HH:MM:SS`, with decimals on
/// the seconds when marks `step_seconds` apart need them.
#[must_use]
pub fn utc_time(unix_ms: f64, step_seconds: f64) -> String {
    if !unix_ms.is_finite() {
        return String::new();
    }
    let places = decimals(step_seconds);
    // rounded first, then wrapped, so the last instant of a day reads 00:00:00
    let day_seconds = rounded(unix_ms / 1000.0, places).rem_euclid(86_400.0);
    let (_, hours, minutes, secs) = split(day_seconds);
    let width = if places == 0 { 2 } else { places + 3 };
    format!("{hours:02}:{minutes:02}:{secs:0width$.places$}")
}

/// A moment at millisecond resolution: the UTC date and time through
/// `base`, or the boot clock without one.
#[must_use]
pub fn stamp(base: Option<TimeBase>, seconds: f64) -> String {
    match base {
        Some(base) => {
            let ms = base.wall_clock_unix_ms(seconds * 1000.0);
            format!("{} {}", utc_date(ms), utc_time(ms, 0.001))
        }
        None => boot_time(seconds, 0.001),
    }
}

/// `seconds` since boot through `base` in ISO 8601 UTC to the
/// microsecond, `2026-03-04T05:06:07.123456Z`: the CSV's `utc` column,
/// which must never merge two rows that its `time_s` tells apart. It is
/// computed in whole microseconds from the base's two millisecond fields:
/// `wall_clock_unix_ms` works in `f64` milliseconds, which near 1.7e12 are
/// spaced about 0.24 µs apart and could flip the last digit.
#[must_use]
pub fn iso_stamp(base: TimeBase, seconds: f64) -> String {
    const DAY_US: i64 = 86_400_000_000;
    if !seconds.is_finite() {
        return String::new();
    }
    // a corrupt board time can be finite yet far beyond any date: the
    // cast saturates, and the sum then has no room, so the cell is empty
    let board_us = (seconds * 1e6).round() as i64;
    let Some(unix_us) = base
        .gps_start_unix_ms
        .checked_sub(base.ms_offset)
        .and_then(|ms| ms.checked_mul(1000))
        .and_then(|us| us.checked_add(board_us))
    else {
        return String::new();
    };
    let (year, month, day) = civil_from_days(unix_us.div_euclid(DAY_US));
    let in_day = unix_us.rem_euclid(DAY_US);
    let seconds = in_day / 1_000_000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        in_day % 1_000_000
    )
}

/// Unix milliseconds as a UTC calendar date, `YYYY-MM-DD`.
#[must_use]
pub fn utc_date(unix_ms: f64) -> String {
    if !unix_ms.is_finite() {
        return String::new();
    }
    let days = (unix_ms / 86_400_000.0).floor() as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date of a day count from 1970-01-01, by
/// Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_time_reads_as_a_clock() {
        assert_eq!(boot_time(0.0, 1.0), "0:00");
        assert_eq!(boot_time(65.0, 10.0), "1:05");
        assert_eq!(boot_time(3725.0, 60.0), "1:02:05");
        assert_eq!(boot_time(65.27, 0.1), "1:05.3");
        assert_eq!(boot_time(65.25, 0.25), "1:05.25");
        assert_eq!(boot_time(65.25, 0.05), "1:05.25");
        assert_eq!(boot_time(5.0, 0.001), "0:05.000");
        assert_eq!(boot_time(-3.5, 0.5), "-0:03.5");
        assert_eq!(boot_time(f64::NAN, 1.0), "");
        // a value just under a minute carries into it instead of reading 60
        assert_eq!(boot_time(59.9996, 0.001), "1:00.000");
        assert_eq!(boot_time(59.9996, 1.0), "1:00");
        assert_eq!(boot_time(3599.9999, 1.0), "1:00:00");
        assert_eq!(boot_time(59.4, 1.0), "0:59");
    }

    #[test]
    fn utc_time_and_date_come_from_unix_milliseconds() {
        // 2023-11-14 22:13:20 UTC
        let ms = 1_700_000_000_000.0;
        assert_eq!(utc_time(ms, 1.0), "22:13:20");
        assert_eq!(utc_time(ms + 250.0, 0.25), "22:13:20.25");
        assert_eq!(utc_date(ms), "2023-11-14");
        // the epoch, a leap day and the end of a century
        assert_eq!(utc_date(0.0), "1970-01-01");
        assert_eq!(utc_date(951_782_400_000.0), "2000-02-29");
        assert_eq!(utc_date(-86_400_000.0), "1969-12-31");
        assert_eq!(utc_time(-1000.0, 1.0), "23:59:59");
        // the last instant of a day rounds into the next one
        assert_eq!(utc_time(86_399_999.6, 1.0), "00:00:00");
        assert_eq!(utc_time(ms + 59_999.6, 0.001), "22:14:20.000");

        // a stamp reads through the base when there is one
        assert_eq!(stamp(None, 65.25), "1:05.250");
        let base = TimeBase {
            gps_start_unix_ms: 1_700_000_000_000,
            ms_offset: 60_000,
        };
        assert_eq!(stamp(Some(base), 65.25), "2023-11-14 22:13:25.250");
    }

    #[test]
    fn iso_stamps_are_exact_to_the_microsecond() {
        let base = TimeBase {
            gps_start_unix_ms: 1_700_000_000_000,
            ms_offset: 60_000,
        };
        assert_eq!(iso_stamp(base, 65.250_001), "2023-11-14T22:13:25.250001Z");
        // a bit over half a microsecond past rounds up
        assert_eq!(iso_stamp(base, 65.250_000_6), "2023-11-14T22:13:25.250001Z");
        // a board time before the base
        assert_eq!(iso_stamp(base, 0.0), "2023-11-14T22:12:20.000000Z");
        let epoch = TimeBase {
            gps_start_unix_ms: 0,
            ms_offset: 0,
        };
        assert_eq!(iso_stamp(epoch, 0.0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(iso_stamp(epoch, -0.000_001), "1969-12-31T23:59:59.999999Z");
        assert_eq!(iso_stamp(epoch, f64::NAN), "");
        // a corrupt TimeUS near u64::MAX, in seconds
        assert_eq!(iso_stamp(base, 1.8e13), "");
    }

    #[test]
    fn decimals_follow_the_step() {
        assert_eq!(decimals(10.0), 0);
        assert_eq!(decimals(1.0), 0);
        assert_eq!(decimals(0.5), 1);
        assert_eq!(decimals(0.1), 1);
        assert_eq!(decimals(0.25), 2);
        assert_eq!(decimals(0.02), 2);
        assert_eq!(decimals(0.001), 3);
        assert_eq!(decimals(1.0 / 3.0), 6);
        assert_eq!(decimals(1e-9), 6);
        assert_eq!(decimals(0.0), 0);
        assert_eq!(decimals(f64::NAN), 0);
    }
}
