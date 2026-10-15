//! Versions and the channels they are published on.
//!
//! A version names a directory on the mirror and in `/opt/dassie`, so the
//! parser admits only digits, lowercase letters, dots and dashes, in the two
//! shapes the release build makes: nothing that could leave a directory.

use alloc::string::{String, ToString};
use core::fmt;

/// The longest version accepted.
pub const MAX_LEN: usize = 64;

/// The shortest commit hash a canary names, as `git rev-parse --short`.
const MIN_COMMIT: usize = 7;
/// The longest: a full SHA-1 hash.
const MAX_COMMIT: usize = 40;
/// What a canary built from a tree with uncommitted changes ends with.
const DIRTY: &str = "-dirty";

/// A version of Dassie.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Version {
    /// `major.minor.patch`, published on [`Channel::Latest`].
    Release {
        /// The major version.
        major: u32,
        /// The minor version.
        minor: u32,
        /// The patch version.
        patch: u32,
    },
    /// `canary-YYYY-MM-DD-<commit>`, published on [`Channel::Canary`]: the
    /// day of the commit it was built from, and its hash (7 to 40 lowercase
    /// hex digits, then `-dirty` for a local build with uncommitted changes).
    Canary {
        /// The year of the commit.
        year: u16,
        /// Its month, 1 to 12.
        month: u8,
        /// Its day, 1 to 31.
        day: u8,
        /// The commit's hash.
        commit: String,
    },
}

/// Why a version was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionError {
    /// Longer than [`MAX_LEN`].
    TooLong,
    /// Neither a release nor a canary version.
    Malformed,
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(f, "the version is longer than {MAX_LEN} bytes"),
            Self::Malformed => write!(
                f,
                "the version is not of the form 1.2.3 or canary-YYYY-MM-DD-<commit>"
            ),
        }
    }
}

impl Version {
    /// Parses a version as the release build writes it.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        if text.len() > MAX_LEN {
            return Err(VersionError::TooLong);
        }
        match text.strip_prefix("canary-") {
            Some(rest) => parse_canary(rest),
            None => parse_release(text),
        }
        .ok_or(VersionError::Malformed)
    }

    /// The channel this version is published on.
    pub fn channel(&self) -> Channel {
        match self {
            Self::Release { .. } => Channel::Latest,
            Self::Canary { .. } => Channel::Canary,
        }
    }

    /// Whether the updater may replace `installed` with this version on its
    /// own. A release supersedes an older release, and a canary a canary of
    /// an earlier day, or another commit of the same day, since the day is
    /// all that orders them. Moving between channels is the owner's choice:
    /// a version of the other channel supersedes, as the owner asked for it.
    /// So does nothing else: a mirror that offers an older version does not
    /// take a node back to it.
    pub fn supersedes(&self, installed: &Version) -> bool {
        match (self, installed) {
            (
                Self::Release {
                    major,
                    minor,
                    patch,
                },
                Self::Release {
                    major: old_major,
                    minor: old_minor,
                    patch: old_patch,
                },
            ) => (major, minor, patch) > (old_major, old_minor, old_patch),
            (
                Self::Canary {
                    year,
                    month,
                    day,
                    commit,
                },
                Self::Canary {
                    year: old_year,
                    month: old_month,
                    day: old_day,
                    commit: old_commit,
                },
            ) => {
                let (date, old_date) = ((year, month, day), (old_year, old_month, old_day));
                date > old_date || (date == old_date && commit != old_commit)
            }
            _ => true,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Release {
                major,
                minor,
                patch,
            } => write!(f, "{major}.{minor}.{patch}"),
            Self::Canary {
                year,
                month,
                day,
                commit,
            } => write!(f, "canary-{year:04}-{month:02}-{day:02}-{commit}"),
        }
    }
}

fn parse_release(text: &str) -> Option<Version> {
    let mut parts = text.split('.');
    let major = parse_number(parts.next()?)?;
    let minor = parse_number(parts.next()?)?;
    let patch = parse_number(parts.next()?)?;
    match parts.next() {
        Some(_) => None,
        None => Some(Version::Release {
            major,
            minor,
            patch,
        }),
    }
}

/// A decimal number without a sign or leading zeros.
fn parse_number(text: &str) -> Option<u32> {
    let digits = text.bytes().all(|byte| byte.is_ascii_digit());
    let leading_zero = text.len() > 1 && text.starts_with('0');
    if text.is_empty() || !digits || leading_zero {
        return None;
    }
    text.parse().ok()
}

/// `YYYY-MM-DD-<commit>`, after `canary-`.
fn parse_canary(text: &str) -> Option<Version> {
    let mut parts = text.splitn(4, '-');
    let year = fixed_digits(parts.next()?, 4)?;
    let month = fixed_digits(parts.next()?, 2)?;
    let day = fixed_digits(parts.next()?, 2)?;
    let commit = parts.next()?;
    let hash = commit.strip_suffix(DIRTY).unwrap_or(commit);
    let hex = hash
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (MIN_COMMIT..=MAX_COMMIT).contains(&hash.len())
        && hex;
    if !valid {
        return None;
    }
    Some(Version::Canary {
        year: u16::try_from(year).ok()?,
        month: u8::try_from(month).ok()?,
        day: u8::try_from(day).ok()?,
        commit: commit.to_string(),
    })
}

/// Exactly `len` decimal digits.
fn fixed_digits(text: &str, len: usize) -> Option<u32> {
    let digits = text.len() == len && text.bytes().all(|byte| byte.is_ascii_digit());
    digits.then(|| text.parse().ok()).flatten()
}

/// Where a version is published: the mirror's `meta/<channel>` names the
/// newest version of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Releases, `meta/latest`.
    Latest,
    /// Canaries, built from every change to the main branch, `meta/canary`.
    Canary,
}

impl Channel {
    /// The channel's name, as in `meta/<name>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Canary => "canary",
        }
    }

    /// The channel of this name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "latest" => Some(Self::Latest),
            "canary" => Some(Self::Canary),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use proptest::prelude::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parses_what_the_build_makes() {
        assert_eq!(
            version("0.1.0"),
            Version::Release {
                major: 0,
                minor: 1,
                patch: 0
            }
        );
        assert_eq!(
            version("canary-2026-10-15-15bd7aa"),
            Version::Canary {
                year: 2026,
                month: 10,
                day: 15,
                commit: "15bd7aa".into()
            }
        );
        assert_eq!(
            version("canary-2026-10-15-15bd7aa-dirty").channel(),
            Channel::Canary
        );
        assert_eq!(version("4294967295.0.10").channel(), Channel::Latest);
    }

    #[test]
    fn refuses_anything_else() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.+3",
            "1.2.3-rc.1",
            "4294967296.0.0",
            "v1.2.3",
            "../1.2.3",
            "1.2.3/",
            "canary",
            "canary-2026-10-15",
            "canary-2026-10-15-",
            "canary-2026-10-15-15bd7a",
            "canary-2026-10-15-15BD7AA",
            "canary-2026-13-15-15bd7aa",
            "canary-2026-10-00-15bd7aa",
            "canary-2026-1-15-15bd7aa",
            "canary-2026-10-15-15bd7aa-",
            "canary-2026-10-15-15bd7aa-clean",
            "canary-2026-10-15-15bd7aa/..",
            "canary-2026-10-15-0123456789012345678901234567890123456789a",
        ] {
            assert!(Version::parse(text).is_err(), "{text:?} was accepted");
        }
        let long = format!("canary-2026-10-15-{}", "a".repeat(MAX_LEN));
        assert_eq!(Version::parse(&long), Err(VersionError::TooLong));
    }

    #[test]
    fn newer_versions_supersede() {
        let cases = [
            ("0.1.1", "0.1.0", true),
            ("0.2.0", "0.1.9", true),
            ("1.0.0", "0.10.10", true),
            ("0.1.0", "0.1.0", false),
            ("0.1.0", "0.1.1", false),
            (
                "canary-2026-10-16-aaaaaaa",
                "canary-2026-10-15-bbbbbbb",
                true,
            ),
            (
                "canary-2026-10-15-aaaaaaa",
                "canary-2026-10-15-bbbbbbb",
                true,
            ),
            (
                "canary-2026-10-15-aaaaaaa",
                "canary-2026-10-15-aaaaaaa",
                false,
            ),
            (
                "canary-2026-09-30-aaaaaaa",
                "canary-2026-10-15-bbbbbbb",
                false,
            ),
            ("canary-2025-12-31-aaaaaaa", "0.1.0", true),
            ("0.0.1", "canary-2026-10-15-aaaaaaa", true),
        ];
        for (offered, installed, expected) in cases {
            assert_eq!(
                version(offered).supersedes(&version(installed)),
                expected,
                "{offered} over {installed}"
            );
        }
    }

    #[test]
    fn channels_round_trip() {
        for channel in [Channel::Latest, Channel::Canary] {
            assert_eq!(Channel::parse(channel.name()), Some(channel));
        }
        assert_eq!(Channel::parse("stable"), None);
    }

    proptest! {
        #[test]
        fn releases_round_trip(major: u32, minor: u32, patch: u32) {
            let release = Version::Release { major, minor, patch };
            prop_assert_eq!(Version::parse(&release.to_string()), Ok(release));
        }

        #[test]
        fn canaries_round_trip(
            year in 1000u16..=9999,
            month in 1u8..=12,
            day in 1u8..=31,
            commit in "[0-9a-f]{7,40}(-dirty)?",
        ) {
            let canary = Version::Canary { year, month, day, commit };
            prop_assert_eq!(Version::parse(&canary.to_string()), Ok(canary));
        }

        /// Whatever it accepts is a safe path component, and reads back as
        /// it was written.
        #[test]
        fn never_panics(text in "\\PC{0,80}") {
            if let Ok(version) = Version::parse(&text) {
                prop_assert_eq!(version.to_string(), text.clone());
                prop_assert!(text.bytes().all(|b| b.is_ascii_digit()
                    || b.is_ascii_lowercase() || b == b'.' || b == b'-'));
                prop_assert!(!text.contains(".."));
            }
        }
    }
}
