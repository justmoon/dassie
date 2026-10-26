//! Versions of Dassie.
//!
//! A version names a directory on the mirror and in `/opt/dassie`, so the
//! parser admits only `major.minor.patch` in decimal digits: nothing that
//! could leave a directory.

use core::fmt;

/// The longest version accepted.
pub const MAX_LEN: usize = 80;

/// A release of Dassie: `major.minor.patch`, which `meta/latest` names
/// when it is the newest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    /// The major version.
    pub major: u32,
    /// The minor version.
    pub minor: u32,
    /// The patch version.
    pub patch: u32,
}

/// Why a version was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionError {
    /// Longer than [`MAX_LEN`].
    TooLong,
    /// Not a release version.
    Malformed,
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(f, "the version is longer than {MAX_LEN} bytes"),
            Self::Malformed => write!(f, "the version is not of the form 1.2.3"),
        }
    }
}

impl Version {
    /// Parses a version as the release build writes it.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        if text.len() > MAX_LEN {
            return Err(VersionError::TooLong);
        }
        let mut parts = text.split('.');
        let mut number = || parse_number(parts.next()?);
        let (Some(major), Some(minor), Some(patch)) = (number(), number(), number()) else {
            return Err(VersionError::Malformed);
        };
        match parts.next() {
            Some(_) => Err(VersionError::Malformed),
            None => Ok(Self {
                major,
                minor,
                patch,
            }),
        }
    }

    /// Whether the updater may replace `installed` with this version on its
    /// own: a later release. Nothing else does: `meta/latest` is not
    /// signed, so a mirror that offers an older version does not take a
    /// node there. Only the owner moves a node back.
    pub fn supersedes(&self, installed: &Version) -> bool {
        (self.major, self.minor, self.patch) > (installed.major, installed.minor, installed.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::ToString;
    use proptest::prelude::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parses_what_the_build_makes() {
        assert_eq!(
            version("0.1.0"),
            Version {
                major: 0,
                minor: 1,
                patch: 0
            }
        );
        assert_eq!(version("4294967295.0.10").to_string(), "4294967295.0.10");
    }

    #[test]
    fn refuses_anything_else() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.",
            ".1.2",
            "1..2",
            "1.2.3.4",
            "01.2.3",
            "1.2.+3",
            "1.2.-3",
            "1.2.3-rc.1",
            "1.2.3-dirty",
            "4294967296.0.0",
            "v1.2.3",
            "../1.2.3",
            "1.2.3/",
            " 1.2.3",
            "canary-2026-10-15-041530-15bd7aa",
        ] {
            assert!(Version::parse(text).is_err(), "{text:?} was accepted");
        }
        let long = format!("1.2.{}", "1".repeat(MAX_LEN));
        assert_eq!(Version::parse(&long), Err(VersionError::TooLong));
    }

    #[test]
    fn later_releases_supersede() {
        let cases = [
            ("0.1.1", "0.1.0", true),
            ("0.2.0", "0.1.9", true),
            ("1.0.0", "0.10.10", true),
            ("0.10.0", "0.9.0", true),
            ("0.1.0", "0.1.0", false),
            ("0.1.0", "0.1.1", false),
            ("0.9.9", "1.0.0", false),
        ];
        for (offered, installed, expected) in cases {
            assert_eq!(
                version(offered).supersedes(&version(installed)),
                expected,
                "{offered} over {installed}"
            );
        }
    }

    proptest! {
        #[test]
        fn releases_round_trip(major: u32, minor: u32, patch: u32) {
            let release = Version { major, minor, patch };
            prop_assert_eq!(Version::parse(&release.to_string()), Ok(release));
        }

        /// Superseding is a strict order.
        #[test]
        fn supersedes_is_strict(a in (0u32..3, 0u32..3, 0u32..3), b in (0u32..3, 0u32..3, 0u32..3)) {
            let a = Version { major: a.0, minor: a.1, patch: a.2 };
            let b = Version { major: b.0, minor: b.1, patch: b.2 };
            prop_assert!(!(a.supersedes(&b) && b.supersedes(&a)));
            prop_assert!(!a.supersedes(&a));
            prop_assert!(a == b || a.supersedes(&b) || b.supersedes(&a));
        }

        /// Whatever it accepts is a safe path component, and reads back as
        /// it was written.
        #[test]
        fn never_panics(text in prop_oneof![
            1 => "\\PC{0,90}",
            4 => "[0-9+.-]{0,12}",
        ]) {
            if let Ok(version) = Version::parse(&text) {
                prop_assert_eq!(version.to_string(), text.clone());
                prop_assert!(text.bytes().all(|b| b.is_ascii_digit() || b == b'.'));
                prop_assert!(!text.contains(".."));
            }
        }
    }
}
