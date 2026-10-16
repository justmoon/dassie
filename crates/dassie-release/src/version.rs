//! Versions and the channels they are published on.
//!
//! A version names a directory on the mirror and in `/opt/dassie`, so the
//! parser admits only digits, lowercase letters, dots and dashes, in the two
//! shapes the release build makes: nothing that could leave a directory.

use alloc::string::{String, ToString};
use core::fmt;

/// The longest version accepted.
pub const MAX_LEN: usize = 80;

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
    /// `canary-YYYY-MM-DD-HHMMSS-<commit>`, published on
    /// [`Channel::Canary`]: when the commit it was built from was made, in
    /// UTC, and its hash (7 to 40 lowercase hex digits, then `-dirty` for a
    /// local build with uncommitted changes).
    Canary {
        /// The commit's date: year, month (1 to 12), day (1 to 31).
        date: (u16, u8, u8),
        /// Its time of day: hour (0 to 23), minute, second (0 to 59).
        time: (u8, u8, u8),
        /// Its hash.
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
                "the version is not of the form 1.2.3 or canary-YYYY-MM-DD-HHMMSS-<commit>"
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
    /// own: a later version of the same channel. A release supersedes an
    /// older release, and a canary one of an earlier commit time. Nothing
    /// else does: `meta/<channel>` is not signed, so a mirror that offers
    /// an older version, or one of the other channel, does not take a node
    /// there. Only the owner moves a node back or to another channel.
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
                Self::Canary { date, time, .. },
                Self::Canary {
                    date: old_date,
                    time: old_time,
                    ..
                },
            ) => (date, time) > (old_date, old_time),
            _ => false,
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
                date: (year, month, day),
                time: (hour, minute, second),
                commit,
            } => write!(
                f,
                "canary-{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{second:02}-{commit}"
            ),
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

/// `YYYY-MM-DD-HHMMSS-<commit>`, after `canary-`.
fn parse_canary(text: &str) -> Option<Version> {
    let mut parts = text.splitn(5, '-');
    let year = fixed_digits(parts.next()?)?;
    let month = fixed_digits(parts.next()?)?;
    let day = fixed_digits(parts.next()?)?;
    let [hour, minute, second] = time_of_day(parts.next()?)?;
    let commit = parts.next()?;
    let hash = commit.strip_suffix(DIRTY).unwrap_or(commit);
    let hex = hash
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let valid = year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && (MIN_COMMIT..=MAX_COMMIT).contains(&hash.len())
        && hex;
    let date = (
        year.parse().ok()?,
        month.parse().ok().filter(|m| (1..=12).contains(m))?,
        day.parse().ok().filter(|d| (1..=31).contains(d))?,
    );
    valid.then(|| Version::Canary {
        date,
        time: (hour, minute, second),
        commit: commit.to_string(),
    })
}

/// `HHMMSS`, a time of day.
fn time_of_day(text: &str) -> Option<[u8; 3]> {
    if text.len() != 6 {
        return None;
    }
    let field = |range: core::ops::Range<usize>, max: u8| {
        fixed_digits(text.get(range)?)?
            .parse::<u8>()
            .ok()
            .filter(|value| *value <= max)
    };
    Some([field(0..2, 23)?, field(2..4, 59)?, field(4..6, 59)?])
}

/// Decimal digits only, as they are.
fn fixed_digits(text: &str) -> Option<&str> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits.then_some(text)
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
            version("canary-2026-10-15-041530-15bd7aa"),
            Version::Canary {
                date: (2026, 10, 15),
                time: (4, 15, 30),
                commit: "15bd7aa".into()
            }
        );
        assert_eq!(
            version("canary-2026-10-15-235959-15bd7aa-dirty").channel(),
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
            "canary-2026-10-15-041530",
            "canary-2026-10-15-041530-",
            "canary-2026-10-15-15bd7aa",
            "canary-2026-10-15-041530-15bd7a",
            "canary-2026-10-15-041530-15BD7AA",
            "canary-2026-13-15-041530-15bd7aa",
            "canary-2026-10-00-041530-15bd7aa",
            "canary-2026-10-32-041530-15bd7aa",
            "canary-2026-1-15-041530-15bd7aa",
            "canary-2026-10-15-240000-15bd7aa",
            "canary-2026-10-15-046000-15bd7aa",
            "canary-2026-10-15-041560-15bd7aa",
            "canary-2026-10-15-04153-15bd7aa",
            "canary-2026-10-15-+41530-15bd7aa",
            "canary-2026-10-15-041530-15bd7aa-",
            "canary-2026-10-15-041530-15bd7aa-clean",
            "canary-2026-10-15-041530-15bd7aa/..",
            "canary-2026-10-15-041530-0123456789012345678901234567890123456789a",
        ] {
            assert!(Version::parse(text).is_err(), "{text:?} was accepted");
        }
        let long = format!("canary-2026-10-15-041530-{}", "a".repeat(MAX_LEN));
        assert_eq!(Version::parse(&long), Err(VersionError::TooLong));
        let longest = format!("canary-2026-10-15-041530-{}-dirty", "a".repeat(MAX_COMMIT));
        assert!(Version::parse(&longest).is_ok());
    }

    #[test]
    fn later_versions_of_a_channel_supersede() {
        let cases = [
            ("0.1.1", "0.1.0", true),
            ("0.2.0", "0.1.9", true),
            ("1.0.0", "0.10.10", true),
            ("0.1.0", "0.1.0", false),
            ("0.1.0", "0.1.1", false),
            (
                "canary-2026-10-16-000000-aaaaaaa",
                "canary-2026-10-15-235959-bbbbbbb",
                true,
            ),
            (
                "canary-2026-10-15-041531-aaaaaaa",
                "canary-2026-10-15-041530-bbbbbbb",
                true,
            ),
            (
                "canary-2026-10-15-041530-aaaaaaa",
                "canary-2026-10-15-041530-bbbbbbb",
                false,
            ),
            (
                "canary-2026-10-15-041529-aaaaaaa",
                "canary-2026-10-15-041530-bbbbbbb",
                false,
            ),
            (
                "canary-2026-09-30-120000-aaaaaaa",
                "canary-2026-10-15-000000-bbbbbbb",
                false,
            ),
            // Another channel, whatever its age: only the owner moves there.
            ("canary-2099-12-31-000000-aaaaaaa", "0.1.0", false),
            ("9.0.0", "canary-2026-10-15-041530-aaaaaaa", false),
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

    /// A field of `width` digits: mostly in range, sometimes out of it,
    /// of another width, or anything.
    fn field(in_range: &'static str, width: usize) -> impl Strategy<Value = String> {
        prop_oneof![
            8 => in_range.boxed(),
            1 => proptest::collection::vec(proptest::char::range('0', '9'), width)
                .prop_map(String::from_iter)
                .boxed(),
            1 => "[0-9+]{0,8}".boxed(),
            1 => "\\PC{0,6}".boxed(),
        ]
    }

    /// `canary-` and something like a date, a time and a commit, with
    /// dashes or anything between them.
    fn canary() -> impl Strategy<Value = String> {
        let date = (
            field("[0-9]{4}", 4),
            field("0[1-9]|1[0-2]", 2),
            field("0[1-9]|[12][0-9]|3[01]", 2),
        );
        let time = field("([01][0-9]|2[0-3])[0-5][0-9][0-5][0-9]", 6);
        let commit = prop_oneof![
            8 => "[0-9a-f]{7,40}(-dirty)?",
            1 => "[0-9a-f]{0,42}(-dirty)?",
            1 => "[0-9a-fA-F-]{0,45}",
            1 => "\\PC{0,45}",
        ];
        let dash = prop_oneof![16 => Just("-".into()), 1 => "\\PC{0,2}"];
        let dashes = [dash.clone(), dash.clone(), dash.clone(), dash];
        (date, time, commit, dashes).prop_map(|((year, month, day), time, commit, [a, b, c, d])| {
            format!("canary-{year}{a}{month}{b}{day}{c}{time}{d}{commit}")
        })
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
            hour in 0u8..=23,
            minute in 0u8..=59,
            second in 0u8..=59,
            commit in "[0-9a-f]{7,40}(-dirty)?",
        ) {
            let canary = Version::Canary {
                date: (year, month, day),
                time: (hour, minute, second),
                commit,
            };
            prop_assert_eq!(Version::parse(&canary.to_string()), Ok(canary));
        }

        /// Superseding is a strict order within a channel.
        #[test]
        fn supersedes_is_strict(a in "canary-2026-10-1[0-9]-0[0-9]0000-aaaaaaa",
                                b in "canary-2026-10-1[0-9]-0[0-9]0000-bbbbbbb") {
            let (a, b) = (version(&a), version(&b));
            prop_assert!(!(a.supersedes(&b) && b.supersedes(&a)));
            prop_assert!(!a.supersedes(&a));
        }

        /// Whatever it accepts is a safe path component, and reads back as
        /// it was written.
        #[test]
        fn never_panics(text in prop_oneof![
            1 => "\\PC{0,90}",
            1 => "canary-\\PC{0,80}",
            4 => canary(),
        ]) {
            if let Ok(version) = Version::parse(&text) {
                prop_assert_eq!(version.to_string(), text.clone());
                prop_assert!(text.bytes().all(|b| b.is_ascii_digit()
                    || b.is_ascii_lowercase() || b == b'.' || b == b'-'));
                prop_assert!(!text.contains(".."));
            }
        }

        /// A channel's name is one of the two, exactly.
        #[test]
        fn channels_never_panic(name in prop_oneof![
            "\\PC{0,10}",
            "(latest|canary|Latest|CANARY)\\PC{0,2}",
        ]) {
            if let Some(channel) = Channel::parse(&name) {
                prop_assert_eq!(channel.name(), name);
            }
        }
    }
}
