//! A release's `SHA256SUMS`, as `sha256sum` writes it: a line per bundle,
//! `<64 lowercase hex digits>  <file name>`, each ending in a newline.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// The largest `SHA256SUMS` read: a release has four bundles.
pub const MAX_SIZE: usize = 4096;
/// The most lines it may have.
pub const MAX_ENTRIES: usize = 16;
/// The longest file name in it.
const MAX_NAME: usize = 128;
/// The bytes of a SHA-256 digest.
const DIGEST: usize = 32;

/// The SHA-256 of each file of a release, by file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sums {
    entries: Vec<(String, [u8; DIGEST])>,
}

/// Why `SHA256SUMS` was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SumsError {
    /// Larger than [`MAX_SIZE`].
    TooLarge,
    /// No lines at all.
    Empty,
    /// More than [`MAX_ENTRIES`] lines.
    TooMany,
    /// A line (counted from 1) that is not a digest and a plain file name.
    Malformed(usize),
    /// A line that names a file an earlier line named.
    Duplicate(usize),
}

impl fmt::Display for SumsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "SHA256SUMS is larger than {MAX_SIZE} bytes"),
            Self::Empty => write!(f, "SHA256SUMS is empty"),
            Self::TooMany => write!(f, "SHA256SUMS has more than {MAX_ENTRIES} lines"),
            Self::Malformed(line) => write!(f, "SHA256SUMS line {line} is malformed"),
            Self::Duplicate(line) => write!(f, "SHA256SUMS line {line} names a file twice"),
        }
    }
}

impl Sums {
    /// Reads `SHA256SUMS`.
    pub fn parse(bytes: &[u8]) -> Result<Self, SumsError> {
        if bytes.len() > MAX_SIZE {
            return Err(SumsError::TooLarge);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| SumsError::Malformed(1))?;
        let Some(body) = text.strip_suffix('\n') else {
            return Err(if text.is_empty() {
                SumsError::Empty
            } else {
                SumsError::Malformed(text.lines().count().max(1))
            });
        };
        let mut entries: Vec<(String, [u8; DIGEST])> = Vec::new();
        for (index, line) in body.split('\n').enumerate() {
            let number = index.saturating_add(1);
            if entries.len() == MAX_ENTRIES {
                return Err(SumsError::TooMany);
            }
            let (name, digest) = parse_line(line).ok_or(SumsError::Malformed(number))?;
            if entries.iter().any(|(known, _)| known == name) {
                return Err(SumsError::Duplicate(number));
            }
            entries.push((name.to_string(), digest));
        }
        Ok(Self { entries })
    }

    /// The SHA-256 of the file of this name, if the sums list it.
    pub fn digest(&self, name: &str) -> Option<&[u8; DIGEST]> {
        self.entries
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, digest)| digest)
    }

    /// The files listed, in their order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }
}

/// `<digest>  <name>`, or `<digest> *<name>` as `sha256sum --binary`
/// writes it.
fn parse_line(line: &str) -> Option<(&str, [u8; DIGEST])> {
    let hex = line.get(..DIGEST * 2)?;
    let name = line
        .get(DIGEST * 2..)?
        .strip_prefix("  ")
        .or_else(|| line.get(DIGEST * 2..)?.strip_prefix(" *"))?;
    let mut digest = [0u8; DIGEST];
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    for (byte, [high, low]) in digest.iter_mut().zip(pairs) {
        *byte = (nibble(*high)? << 4) | nibble(*low)?;
    }
    plain_name(name).then_some((name, digest))
}

/// A lowercase hex digit's value.
fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

/// A file name of letters, digits, dots, dashes and underscores, which
/// does not start with a dot: never a path.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use proptest::prelude::*;

    const SUMS: &str = include_str!("../../../testdata/release/SHA256SUMS");

    #[test]
    fn reads_what_sha256sum_writes() {
        let Ok(sums) = Sums::parse(SUMS.as_bytes()) else {
            panic!("the test sums parse");
        };
        assert_eq!(sums.names().count(), 4);
        let Some(digest) = sums.digest("dassie-0.1.0-linux-x64.tar.gz") else {
            panic!("the x64 bundle is listed");
        };
        assert_eq!(digest.first(), Some(&0x4b));
        assert_eq!(digest.last(), Some(&0x6e));
        assert_eq!(sums.digest("dassie-0.1.0-linux-riscv64.tar.gz"), None);
        let binary = SUMS.replace("  dassie", " *dassie");
        assert_eq!(Sums::parse(binary.as_bytes()), Ok(sums));
    }

    #[test]
    fn refuses_what_it_does_not_expect() {
        let digest = "a".repeat(64);
        let line = |name: &str| format!("{digest}  {name}\n");
        let cases = [
            (String::new(), SumsError::Empty),
            (format!("{digest}  bundle"), SumsError::Malformed(1)),
            (line("../etc/passwd"), SumsError::Malformed(1)),
            (line("dir/bundle"), SumsError::Malformed(1)),
            (line(".hidden"), SumsError::Malformed(1)),
            (line(""), SumsError::Malformed(1)),
            (line("a b"), SumsError::Malformed(1)),
            (
                format!("{} bundle\n", "a".repeat(64)),
                SumsError::Malformed(1),
            ),
            (
                format!("{}  bundle\n", "A".repeat(64)),
                SumsError::Malformed(1),
            ),
            (
                format!("{}  bundle\n", "a".repeat(63)),
                SumsError::Malformed(1),
            ),
            (
                format!("{}\n\n", line("one").trim_end()),
                SumsError::Malformed(2),
            ),
            (
                format!("{}{}", line("one"), line("one")),
                SumsError::Duplicate(2),
            ),
            ("x".repeat(MAX_SIZE + 1), SumsError::TooLarge),
        ];
        for (text, expected) in cases {
            assert_eq!(Sums::parse(text.as_bytes()), Err(expected), "{text:?}");
        }
        let many: String = (0..=MAX_ENTRIES).map(|i| line(&format!("f{i}"))).collect();
        assert_eq!(Sums::parse(many.as_bytes()), Err(SumsError::TooMany));
    }

    proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let _ = Sums::parse(&bytes);
        }

        #[test]
        fn lines_round_trip(digest: [u8; 32], name in "[a-z0-9][a-z0-9._-]{0,40}") {
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            let sums = Sums::parse(format!("{hex}  {name}\n").as_bytes());
            prop_assert_eq!(sums.ok().and_then(|s| s.digest(&name).copied()), Some(digest));
        }
    }
}
