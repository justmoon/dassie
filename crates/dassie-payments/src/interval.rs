//! ISO 8601 durations and repeating intervals, as grant limits use them:
//! `R[n]/<start>/<duration>`, `R[n]/<start>/<end>` and
//! `R[n]/<duration>/<end>`.
//!
//! The windows follow the reference implementation (Rafiki, over luxon), so
//! clients see the same ones anywhere:
//! - `Rn` is the first interval and `n` repetitions after it (before it,
//!   for `<duration>/<end>`); `R` and `R-1` repeat for ever.
//! - Each repetition starts where the one before ended, and adds the
//!   duration from there: a month from January 31 ends on February 28, and
//!   the next window then starts on March 28. A window contains its start,
//!   not its end.
//!
//! Durations with years or months step one window at a time, at most
//! [`MAX_CALENDAR_STEPS`] of them (a century of months); durations of fixed
//! length are divided out. Nothing here loops further (D16).

use crate::time::{self, Civil, MS_PER_DAY, days_in_month};

/// Most windows stepped through for a duration with years or months.
pub const MAX_CALENDAR_STEPS: u64 = 1_200;

/// The shortest window a limit may have: each window spent in is a row.
pub const MIN_WINDOW_MS: u64 = 60_000;

const MS_PER_SECOND: u64 = 1_000;
const MS_PER_MINUTE: u64 = 60 * MS_PER_SECOND;
const MS_PER_HOUR: u64 = 60 * MS_PER_MINUTE;

/// The input is not an interval this module reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidInterval;

/// An ISO 8601 duration: calendar months, and a fixed length. Weeks and
/// days are fixed: the calendar here is UTC, without leap seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Duration {
    /// Years times twelve, plus months.
    pub months: u32,
    /// The rest, in milliseconds.
    pub fixed_ms: u64,
}

/// Which end of the intervals is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    /// The first interval starts at this time, and the rest follow.
    Start(u64),
    /// The first interval ends at this time, and the rest precede it.
    End(u64),
}

/// A repeating interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    /// Repetitions after the first interval; `None` for ever.
    repetitions: Option<u64>,
    anchor: Anchor,
    duration: Duration,
}

impl Duration {
    /// Parses `PnYnMnWnDTnHnMnS`, with whole numbers, and seconds to the
    /// millisecond.
    ///
    /// # Errors
    ///
    /// If the input is not such a duration, is empty, or is too long.
    pub fn parse(input: &str) -> Result<Self, InvalidInterval> {
        let rest = input.strip_prefix('P').ok_or(InvalidInterval)?;
        let (date, time) = match rest.split_once('T') {
            Some((date, time)) if !time.is_empty() => (date, Some(time)),
            Some(_) => return Err(InvalidInterval),
            None => (rest, None),
        };
        let mut months = 0u64;
        let mut fixed_ms = 0u64;
        let mut any = false;
        // Values come in thousandths; only seconds may have a fraction.
        for (value, unit) in parts(date, b"YMWD")? {
            any = true;
            let whole = value / 1_000;
            match unit {
                b'Y' => months = add(months, mul(whole, 12)?)?,
                b'M' => months = add(months, whole)?,
                b'W' => fixed_ms = add(fixed_ms, mul(whole, 7 * ms(MS_PER_DAY))?)?,
                _ => fixed_ms = add(fixed_ms, mul(whole, ms(MS_PER_DAY))?)?,
            }
        }
        for (value, unit) in parts(time.unwrap_or(""), b"HMS")? {
            any = true;
            let scaled = match unit {
                b'H' => mul(value / 1_000, MS_PER_HOUR)?,
                b'M' => mul(value / 1_000, MS_PER_MINUTE)?,
                _ => value,
            };
            fixed_ms = add(fixed_ms, scaled)?;
        }
        let months = u32::try_from(months).map_err(|_| InvalidInterval)?;
        if !any || (months == 0 && fixed_ms == 0) || fixed_ms > time::MAX_TIMESTAMP_MS {
            return Err(InvalidInterval);
        }
        Ok(Self { months, fixed_ms })
    }

    /// `at` moved by this duration, forward or back, clamping the day of
    /// the month as months go by; `None` beyond the timestamps there are.
    #[must_use]
    pub fn shift(&self, at: u64, forward: bool) -> Option<u64> {
        let civil = Civil::from_ms(i64::try_from(at).ok()?);
        let months = i64::from(self.months);
        let index = civil.year * 12 + i64::from(civil.month) - 1;
        let index = if forward {
            index + months
        } else {
            index - months
        };
        let year = index.div_euclid(12);
        let month = u32::try_from(index.rem_euclid(12) + 1).ok()?;
        let shifted = Civil {
            year,
            month,
            day: civil.day.min(days_in_month(year, month)),
            time_ms: civil.time_ms,
        };
        let moved = u64::try_from(shifted.to_ms()).ok()?;
        let result = if forward {
            moved.checked_add(self.fixed_ms)?
        } else {
            moved.checked_sub(self.fixed_ms)?
        };
        (result <= time::MAX_TIMESTAMP_MS).then_some(result)
    }
}

impl Interval {
    /// Parses a repeating interval.
    ///
    /// # Errors
    ///
    /// If it is not one of the three forms, or ends before it starts.
    pub fn parse(input: &str) -> Result<Self, InvalidInterval> {
        let mut pieces = input.split('/');
        let (Some(head), Some(first), Some(second), None) =
            (pieces.next(), pieces.next(), pieces.next(), pieces.next())
        else {
            return Err(InvalidInterval);
        };
        let repetitions = match head.strip_prefix('R').ok_or(InvalidInterval)? {
            "" | "-1" => None,
            count if count.bytes().all(|b| b.is_ascii_digit()) && count.len() <= 18 => {
                Some(count.parse().map_err(|_| InvalidInterval)?)
            }
            _ => return Err(InvalidInterval),
        };
        let date = |text: &str| time::parse_rfc3339(text).map_err(|_| InvalidInterval);
        let (anchor, duration) = match (first.starts_with('P'), second.starts_with('P')) {
            (false, true) => (Anchor::Start(date(first)?), Duration::parse(second)?),
            (true, false) => (Anchor::End(date(second)?), Duration::parse(first)?),
            (false, false) => {
                let (start, end) = (date(first)?, date(second)?);
                let length = end.checked_sub(start).filter(|length| *length > 0);
                let fixed_ms = length.ok_or(InvalidInterval)?;
                let duration = Duration {
                    months: 0,
                    fixed_ms,
                };
                (Anchor::Start(start), duration)
            }
            (true, true) => return Err(InvalidInterval),
        };
        Ok(Self {
            repetitions,
            anchor,
            duration,
        })
    }

    /// Whether its windows last at least [`MIN_WINDOW_MS`].
    #[must_use]
    pub const fn windows_are_long_enough(&self) -> bool {
        self.duration.months > 0 || self.duration.fixed_ms >= MIN_WINDOW_MS
    }

    /// The window that contains `now_ms`, as `(start, end)`: `None` before
    /// the first, after the last, or beyond what is stepped through.
    #[must_use]
    pub fn window(&self, now_ms: u64) -> Option<(u64, u64)> {
        match self.anchor {
            Anchor::Start(start) => {
                let end = self.duration.shift(start, true)?;
                self.forward(start, end, now_ms)
            }
            Anchor::End(end) => {
                let start = self.duration.shift(end, false)?;
                self.backward(start, end, now_ms)
            }
        }
    }

    /// Windows after the first, from `[start, end)`.
    fn forward(&self, start: u64, end: u64, now_ms: u64) -> Option<(u64, u64)> {
        if now_ms < start {
            return None;
        }
        let last = self.repetitions.unwrap_or(u64::MAX);
        if self.duration.months == 0 {
            // Each window is as long as the first.
            let length = end - start;
            let index = (now_ms - start) / length;
            if index > last {
                return None;
            }
            let from = start.checked_add(index.checked_mul(length)?)?;
            return Some((from, from.checked_add(length)?));
        }
        let (mut from, mut to) = (start, end);
        for index in 0..=last.min(MAX_CALENDAR_STEPS) {
            if now_ms < to {
                return Some((from, to));
            }
            if index == last {
                break;
            }
            (from, to) = (to, self.duration.shift(to, true)?);
        }
        None
    }

    /// Windows before the first, from `[start, end)`.
    fn backward(&self, start: u64, end: u64, now_ms: u64) -> Option<(u64, u64)> {
        if now_ms >= end {
            return None;
        }
        let last = self.repetitions.unwrap_or(u64::MAX);
        if self.duration.months == 0 {
            let length = end - start;
            let index = (end - 1 - now_ms) / length;
            if index > last {
                return None;
            }
            let to = end.checked_sub(index.checked_mul(length)?)?;
            return Some((to.checked_sub(length)?, to));
        }
        let (mut from, mut to) = (start, end);
        for index in 0..=last.min(MAX_CALENDAR_STEPS) {
            if now_ms >= from {
                return Some((from, to));
            }
            if index == last {
                break;
            }
            (from, to) = (self.duration.shift(from, false)?, from);
        }
        None
    }
}

/// The `<number><unit>` pairs of one part of a duration, in the order
/// `units` lists them, each at most once. Values are in thousandths, so
/// seconds may carry a fraction of up to three digits.
fn parts(text: &str, units: &[u8]) -> Result<alloc::vec::Vec<(u64, u8)>, InvalidInterval> {
    let mut found = alloc::vec::Vec::new();
    let mut rest = text.as_bytes();
    let mut next_unit = 0usize;
    while !rest.is_empty() {
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let (whole, after) = rest.split_at(digits);
        let (fraction, after) = match after.split_first() {
            Some((b'.', tail)) => {
                let count = tail.iter().take_while(|b| b.is_ascii_digit()).count();
                tail.split_at(count)
            }
            _ => (&[][..], after),
        };
        let (&unit, tail) = after.split_first().ok_or(InvalidInterval)?;
        let position = units
            .iter()
            .skip(next_unit)
            .position(|&u| u == unit)
            .ok_or(InvalidInterval)?;
        next_unit += position + 1;
        if whole.is_empty() || whole.len() > 12 || fraction.len() > 3 {
            return Err(InvalidInterval);
        }
        if !fraction.is_empty() && unit != b'S' {
            return Err(InvalidInterval);
        }
        let mut value = 0u64;
        for &digit in whole.iter().chain(fraction) {
            value = value * 10 + u64::from(digit - b'0');
        }
        for _ in fraction.len()..3 {
            value *= 10;
        }
        found.push((value, unit));
        rest = tail;
    }
    Ok(found)
}

fn add(a: u64, b: u64) -> Result<u64, InvalidInterval> {
    a.checked_add(b).ok_or(InvalidInterval)
}

fn mul(a: u64, b: u64) -> Result<u64, InvalidInterval> {
    a.checked_mul(b).ok_or(InvalidInterval)
}

fn ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::time::parse_rfc3339;

    fn at(text: &str) -> u64 {
        parse_rfc3339(text).unwrap()
    }

    fn window(interval: &str, now: &str) -> Option<(u64, u64)> {
        Interval::parse(interval).unwrap().window(at(now))
    }

    #[test]
    fn durations_parse_as_iso_8601_writes_them() {
        let day = ms(MS_PER_DAY);
        assert_eq!(
            Duration::parse("P1Y2M10DT2H30M"),
            Ok(Duration {
                months: 14,
                fixed_ms: 10 * day + 2 * MS_PER_HOUR + 30 * MS_PER_MINUTE,
            })
        );
        assert_eq!(
            Duration::parse("P2W"),
            Ok(Duration {
                months: 0,
                fixed_ms: 14 * day
            })
        );
        assert_eq!(
            Duration::parse("PT1.5S"),
            Ok(Duration {
                months: 0,
                fixed_ms: 1_500
            })
        );
        for bad in [
            "", "P", "PT", "P1", "1M", "P1H", "PT1D", "P1M1Y", "P1.5M", "P0D", "P1M1M",
        ] {
            assert_eq!(Duration::parse(bad), Err(InvalidInterval), "{bad}");
        }
    }

    // The examples of the Open Payments specs.
    #[test]
    fn the_spec_examples_have_windows() {
        let monthly = "R11/2022-08-24T14:15:22Z/P1M";
        assert_eq!(
            window(monthly, "2022-10-01T00:00:00Z"),
            Some((at("2022-09-24T14:15:22Z"), at("2022-10-24T14:15:22Z")))
        );
        assert_eq!(window(monthly, "2022-08-24T14:15:21Z"), None);
        // The first interval and eleven repetitions: a year of months.
        assert_eq!(
            window(monthly, "2023-08-24T14:15:21Z"),
            Some((at("2023-07-24T14:15:22Z"), at("2023-08-24T14:15:22Z")))
        );
        assert_eq!(window(monthly, "2023-08-24T14:15:22Z"), None);
        let spans = "R/2017-03-01T13:00:00Z/2018-05-11T15:30:00Z";
        let first = window(spans, "2017-03-01T13:00:00Z").unwrap();
        let later = window(spans, "2030-01-01T00:00:00Z").unwrap();
        assert_eq!(first.0, at("2017-03-01T13:00:00Z"));
        assert_eq!(later.1 - later.0, first.1 - first.0);
        let back = "R-1/P1Y2M10DT2H30M/2022-05-11T15:30:00Z";
        assert_eq!(
            window(back, "2022-05-11T15:29:59Z").map(|(_, end)| end),
            Some(at("2022-05-11T15:30:00Z"))
        );
        assert_eq!(window(back, "2022-05-11T15:30:00Z"), None);
        assert!(window(back, "2019-01-01T00:00:00Z").is_some());
    }

    // As luxon does: each month counts from where the last ended.
    #[test]
    fn month_ends_clamp_and_stay_clamped() {
        let interval = "R/2023-01-31T00:00:00Z/P1M";
        assert_eq!(
            window(interval, "2023-03-01T00:00:00Z"),
            Some((at("2023-02-28T00:00:00Z"), at("2023-03-28T00:00:00Z")))
        );
        assert_eq!(
            window(interval, "2023-03-30T00:00:00Z"),
            Some((at("2023-03-28T00:00:00Z"), at("2023-04-28T00:00:00Z")))
        );
    }

    #[test]
    fn r0_is_the_first_interval_only() {
        let interval = "R0/2023-01-01T00:00:00Z/P1D";
        assert!(window(interval, "2023-01-01T12:00:00Z").is_some());
        assert_eq!(window(interval, "2023-01-02T00:00:00Z"), None);
    }

    #[test]
    fn calendar_steps_are_bounded() {
        // A century and more of months later: no window, no long loop.
        let interval = "R/1970-01-01T00:00:00Z/P1M";
        assert!(window(interval, "2069-12-15T00:00:00Z").is_some());
        assert_eq!(window(interval, "2100-01-15T00:00:00Z"), None);
    }

    #[test]
    fn refuses_what_is_not_a_repeating_interval() {
        for bad in [
            "",
            "P1M",
            "2022-01-01T00:00:00Z/P1M",
            "R/P1M/P1M",
            "R/2022-01-02T00:00:00Z/2022-01-01T00:00:00Z",
            "R1.5/2022-01-01T00:00:00Z/P1M",
            "R-2/2022-01-01T00:00:00Z/P1M",
            "R/2022-01-01T00:00:00Z/P1M/x",
            "X/2022-01-01T00:00:00Z/P1M",
        ] {
            assert_eq!(Interval::parse(bad), Err(InvalidInterval), "{bad}");
        }
    }
}
