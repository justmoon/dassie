//! Timestamps as Open Payments writes them: RFC 3339 date-times, held as
//! milliseconds since the Unix epoch.
//!
//! The calendar arithmetic is Howard Hinnant's `days_from_civil` and
//! `civil_from_days`, in integers: no clock, no floating point (D16).

use alloc::format;
use alloc::string::String;

/// The latest instant representable here: 9999-12-31T23:59:59.999Z.
pub const MAX_TIMESTAMP_MS: u64 = 253_402_300_799_999;

const MS_PER_SECOND: i64 = 1_000;
const MS_PER_MINUTE: i64 = 60 * MS_PER_SECOND;
const MS_PER_HOUR: i64 = 60 * MS_PER_MINUTE;
/// Milliseconds in a calendar day (UTC has no leap seconds here).
pub const MS_PER_DAY: i64 = 24 * MS_PER_HOUR;

/// The input is not an RFC 3339 date-time between 1970 and 9999.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTime;

/// A calendar date and time of day, in UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    /// Year, 1970 through 9999 for timestamps.
    pub year: i64,
    /// Month, 1 through 12.
    pub month: u32,
    /// Day of the month, 1 through 31.
    pub day: u32,
    /// Milliseconds since midnight.
    pub time_ms: i64,
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    assert!((1..=12).contains(&month), "month out of range");
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01, as `(year, month, day)`.
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// Days in `month` of `year`.
#[must_use]
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        _ => 28,
    }
}

impl Civil {
    /// The calendar form of a timestamp.
    #[must_use]
    pub fn from_ms(ms: i64) -> Self {
        let (year, month, day) = civil_from_days(ms.div_euclid(MS_PER_DAY));
        Self {
            year,
            month,
            day,
            time_ms: ms.rem_euclid(MS_PER_DAY),
        }
    }

    /// Milliseconds since the epoch.
    #[must_use]
    pub fn to_ms(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * MS_PER_DAY + self.time_ms
    }
}

/// Formats a timestamp as `YYYY-MM-DDTHH:MM:SS.mmmZ`, as JavaScript's
/// `toISOString` does.
///
/// # Panics
///
/// If `ms` is beyond [`MAX_TIMESTAMP_MS`].
#[must_use]
pub fn format_rfc3339(ms: u64) -> String {
    assert!(ms <= MAX_TIMESTAMP_MS, "timestamp beyond year 9999");
    let civil = Civil::from_ms(i64::try_from(ms).unwrap_or(0));
    let time = civil.time_ms;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        civil.year,
        civil.month,
        civil.day,
        time / MS_PER_HOUR,
        time % MS_PER_HOUR / MS_PER_MINUTE,
        time % MS_PER_MINUTE / MS_PER_SECOND,
        time % MS_PER_SECOND,
    )
}

/// Parses an RFC 3339 `date-time` into milliseconds since the epoch.
/// Fractions beyond milliseconds are truncated; leap seconds and dates
/// before 1970 are refused.
///
/// # Errors
///
/// If the input is not such a date-time.
pub fn parse_rfc3339(input: &str) -> Result<u64, InvalidTime> {
    let mut cursor = Cursor(input.as_bytes());
    let year = cursor.digits(4)?;
    cursor.expect(b'-')?;
    let month = cursor.digits(2)?;
    cursor.expect(b'-')?;
    let day = cursor.digits(2)?;
    if !matches!(cursor.next(), Some(b'T' | b't')) {
        return Err(InvalidTime);
    }
    let time_ms = cursor.time_of_day()?;
    let offset_ms = cursor.offset()?;
    if !cursor.0.is_empty() {
        return Err(InvalidTime);
    }
    let month = u32::try_from(month).map_err(|_| InvalidTime)?;
    let day = u32::try_from(day).map_err(|_| InvalidTime)?;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return Err(InvalidTime);
    }
    let local = Civil {
        year,
        month,
        day,
        time_ms,
    };
    let utc = local.to_ms() - offset_ms;
    u64::try_from(utc)
        .ok()
        .filter(|&ms| ms <= MAX_TIMESTAMP_MS)
        .ok_or(InvalidTime)
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn next(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }

    fn expect(&mut self, byte: u8) -> Result<(), InvalidTime> {
        if self.next() == Some(byte) {
            Ok(())
        } else {
            Err(InvalidTime)
        }
    }

    fn digits(&mut self, count: usize) -> Result<i64, InvalidTime> {
        let mut value = 0i64;
        for _ in 0..count {
            match self.next() {
                Some(digit @ b'0'..=b'9') => value = value * 10 + i64::from(digit - b'0'),
                _ => return Err(InvalidTime),
            }
        }
        Ok(value)
    }

    fn time_of_day(&mut self) -> Result<i64, InvalidTime> {
        let hour = self.digits(2)?;
        self.expect(b':')?;
        let minute = self.digits(2)?;
        self.expect(b':')?;
        let second = self.digits(2)?;
        if hour > 23 || minute > 59 || second > 59 {
            return Err(InvalidTime);
        }
        let fraction_ms = self.fraction_ms()?;
        Ok(hour * MS_PER_HOUR + minute * MS_PER_MINUTE + second * MS_PER_SECOND + fraction_ms)
    }

    /// At most nine fractional digits, truncated to milliseconds.
    fn fraction_ms(&mut self) -> Result<i64, InvalidTime> {
        if self.0.first() != Some(&b'.') {
            return Ok(0);
        }
        self.next();
        let mut value = 0i64;
        let mut count = 0usize;
        while let Some(&digit @ b'0'..=b'9') = self.0.first() {
            self.next();
            count += 1;
            if count > 9 {
                return Err(InvalidTime);
            }
            if count <= 3 {
                value = value * 10 + i64::from(digit - b'0');
            }
        }
        match count {
            0 => Err(InvalidTime),
            1 => Ok(value * 100),
            2 => Ok(value * 10),
            _ => Ok(value),
        }
    }

    fn offset(&mut self) -> Result<i64, InvalidTime> {
        let sign = match self.next() {
            Some(b'Z' | b'z') => return Ok(0),
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Err(InvalidTime),
        };
        let hours = self.digits(2)?;
        self.expect(b':')?;
        let minutes = self.digits(2)?;
        if hours > 23 || minutes > 59 {
            return Err(InvalidTime);
        }
        Ok(sign * (hours * MS_PER_HOUR + minutes * MS_PER_MINUTE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_calendar() {
        for days in [-719_468, -1, 0, 59, 365, 11_016, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn parses_the_spec_examples() {
        assert_eq!(
            parse_rfc3339("2022-04-12T23:20:50.52Z"),
            Ok(1_649_805_650_520)
        );
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Ok(0));
        assert_eq!(
            parse_rfc3339("2022-04-13T01:20:50.520123+02:00"),
            Ok(1_649_805_650_520)
        );
        assert_eq!(
            parse_rfc3339("9999-12-31T23:59:59.999Z"),
            Ok(MAX_TIMESTAMP_MS)
        );
    }

    #[test]
    fn refuses_what_is_not_a_date_time() {
        for input in [
            "",
            "2022-04-12",
            "2022-04-12T23:20:50",
            "2022-02-29T00:00:00Z",
            "2022-04-12T24:00:00Z",
            "2022-04-12T23:59:60Z",
            "2022-04-12T23:20:50.Z",
            "2022-04-12T23:20:50.1234567890Z",
            "1969-12-31T23:59:59Z",
            "2022-04-12T23:20:50Zjunk",
            "２022-04-12T23:20:50Z",
        ] {
            assert_eq!(parse_rfc3339(input), Err(InvalidTime), "{input}");
        }
    }

    #[test]
    fn formats_as_javascript_does() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_rfc3339(1_649_805_650_520),
            "2022-04-12T23:20:50.520Z"
        );
        assert_eq!(format_rfc3339(MAX_TIMESTAMP_MS), "9999-12-31T23:59:59.999Z");
    }
}
