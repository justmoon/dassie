use core::fmt;

/// Length of an Interledger timestamp in characters (`YYYYMMDDHHmmssSSS`).
pub const TIMESTAMP_LENGTH: usize = 17;

const MS_PER_SECOND: u64 = 1000;
const MS_PER_DAY: u64 = 86_400 * MS_PER_SECOND;
const FIRST_YEAR: u64 = 1970;
const LAST_YEAR: u64 = 9999;

/// A point in time in milliseconds since the Unix epoch, UTC.
///
/// This is the time type of all Dassie protocol code. Nothing in the core
/// crates reads a clock: callers pass the current time in
/// (spec/decisions/tigerstyle.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Milliseconds since 1970-01-01T00:00:00Z.
    pub unix_ms: u64,
}

/// Why a string is not a valid Interledger timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTimestamp;

impl Timestamp {
    /// The latest time an Interledger timestamp can express,
    /// 9999-12-31T23:59:59.999Z.
    pub const MAX: Self = Self {
        unix_ms: days_from_civil(LAST_YEAR, 12, 31) * MS_PER_DAY + MS_PER_DAY - 1,
    };

    /// Creates a timestamp from milliseconds since the Unix epoch.
    #[must_use]
    pub const fn from_unix_ms(unix_ms: u64) -> Self {
        Self { unix_ms }
    }

    /// This time plus `duration_ms`, saturating at [`Timestamp::MAX`].
    #[must_use]
    pub const fn saturating_add_ms(self, duration_ms: u64) -> Self {
        let unix_ms = self.unix_ms.saturating_add(duration_ms);
        if unix_ms > Self::MAX.unix_ms {
            Self::MAX
        } else {
            Self { unix_ms }
        }
    }

    /// Milliseconds from `earlier` to this time, or zero if `earlier` is later.
    #[must_use]
    pub const fn saturating_ms_since(self, earlier: Self) -> u64 {
        self.unix_ms.saturating_sub(earlier.unix_ms)
    }

    /// Parses the Interledger format `YYYYMMDDHHmmssSSS` (ILPv4, RFC 27).
    ///
    /// Every character must be a digit and the date must exist, so a bad
    /// timestamp fails here and not later. Years
    /// before 1970 are rejected: an ILP packet that old has expired anyway.
    pub fn parse(text: &[u8; TIMESTAMP_LENGTH]) -> Result<Self, InvalidTimestamp> {
        let field = |start: usize, end: usize| digits(text.get(start..end).unwrap_or_default());
        let year = field(0, 4)?;
        let month = field(4, 6)?;
        let day = field(6, 8)?;
        let hour = field(8, 10)?;
        let minute = field(10, 12)?;
        let second = field(12, 14)?;
        let millisecond = field(14, 17)?;
        let valid = (FIRST_YEAR..=LAST_YEAR).contains(&year)
            && (1..=12).contains(&month)
            && (1..=days_in_month(year, month)).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        if !valid {
            return Err(InvalidTimestamp);
        }
        let seconds = hour * 3600 + minute * 60 + second;
        Ok(Self {
            unix_ms: days_from_civil(year, month, day) * MS_PER_DAY
                + seconds * MS_PER_SECOND
                + millisecond,
        })
    }

    /// Formats the time as `YYYYMMDDHHmmssSSS`.
    #[must_use]
    pub fn to_ilp_bytes(self) -> [u8; TIMESTAMP_LENGTH] {
        let time = self.min(Self::MAX).unix_ms;
        let (year, month, day) = civil_from_days(time / MS_PER_DAY);
        let of_day = time % MS_PER_DAY;
        let fields = [
            (year, 4),
            (month, 2),
            (day, 2),
            (of_day / 3_600_000, 2),
            (of_day / 60_000 % 60, 2),
            (of_day / 1000 % 60, 2),
            (of_day % 1000, 3),
        ];
        let mut output = [b'0'; TIMESTAMP_LENGTH];
        let mut end = 0;
        for (value, width) in fields {
            end += width;
            let mut remainder = value;
            for slot in output
                .get_mut(end - width..end)
                .unwrap_or_default()
                .iter_mut()
                .rev()
            {
                *slot = b'0' + low_digit(remainder);
                remainder /= 10;
            }
        }
        output
    }
}

#[allow(clippy::cast_possible_truncation)]
const fn low_digit(value: u64) -> u8 {
    // A remainder of 10 always fits.
    (value % 10) as u8
}

fn digits(text: &[u8]) -> Result<u64, InvalidTimestamp> {
    text.iter().try_fold(0u64, |value, &byte| {
        if byte.is_ascii_digit() {
            Ok(value * 10 + u64::from(byte - b'0'))
        } else {
            Err(InvalidTimestamp)
        }
    })
}

const fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

const fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given date, for years from 1970 on
/// (Howard Hinnant's `days_from_civil`, restricted to non-negative results).
const fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    // 719_468 is the day number of 1970-01-01 counted from 0000-03-01.
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.to_ilp_bytes();
        // Always ASCII digits.
        f.write_str(core::str::from_utf8(&bytes).unwrap_or_default())
    }
}

impl fmt::Display for InvalidTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid Interledger timestamp")
    }
}

impl core::error::Error for InvalidTimestamp {}
