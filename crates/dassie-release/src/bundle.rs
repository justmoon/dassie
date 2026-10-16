//! What the updater lets out of a bundle.
//!
//! A bundle is checked against its signed sum before it is opened, but the
//! updater runs as root, so what it unpacks is bounded and plain all the
//! same: directories and regular files under `dassie/`, with plain names,
//! nothing else, and not too many or too large.

use alloc::string::String;
use core::fmt;

/// The largest bundle the updater downloads.
pub const MAX_BUNDLE: usize = 64 * 1024 * 1024;
/// The most entries it unpacks.
pub const MAX_ENTRIES: usize = 64;
/// The most bytes it unpacks.
pub const MAX_UNPACKED: u64 = 256 * 1024 * 1024;
/// The longest path in it.
const MAX_PATH: usize = 256;
/// The deepest an entry can be: each directory takes a byte and a `/`.
pub const MAX_DEPTH: usize = MAX_PATH.div_ceil(2);
/// The directory every entry is in, which becomes `/opt/dassie/<version>`.
pub const ROOT: &str = "dassie";
/// The binary, under the root.
pub const BINARY: &str = "bin/dassie";

/// What an entry of a bundle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
    /// Anything else: links, devices, FIFOs.
    Other,
}

/// Why a bundle was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleError {
    /// An entry that is not a directory or a regular file.
    Kind(String),
    /// An entry outside `dassie/`, or with a name that is not plain.
    Path(String),
    /// More than [`MAX_ENTRIES`] entries.
    TooManyEntries,
    /// More than [`MAX_UNPACKED`] bytes.
    TooLarge,
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind(path) => write!(
                f,
                "the bundle holds {path:?}, which is not a file or directory"
            ),
            Self::Path(path) => write!(
                f,
                "the bundle holds {path:?}, outside {ROOT}/ or not a plain name"
            ),
            Self::TooManyEntries => write!(f, "the bundle has more than {MAX_ENTRIES} entries"),
            Self::TooLarge => write!(f, "the bundle unpacks to more than {MAX_UNPACKED} bytes"),
        }
    }
}

/// Admits a bundle's entries one by one, within the bounds.
#[derive(Debug, Default)]
pub struct Unpacking {
    entries: usize,
    bytes: u64,
}

impl Unpacking {
    /// A bundle about to be unpacked.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admits an entry, and returns its path under `dassie/`: empty for
    /// `dassie/` itself, which needs no unpacking but is allowed.
    pub fn admit<'a>(
        &mut self,
        path: &'a str,
        kind: Kind,
        size: u64,
    ) -> Result<&'a str, BundleError> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > MAX_ENTRIES {
            return Err(BundleError::TooManyEntries);
        }
        let inner = under_root(path).ok_or_else(|| BundleError::Path(path.into()))?;
        match kind {
            Kind::Directory => Ok(inner),
            Kind::File if !inner.is_empty() && !path.ends_with('/') => {
                self.bytes = self.bytes.saturating_add(size);
                if self.bytes > MAX_UNPACKED {
                    return Err(BundleError::TooLarge);
                }
                Ok(inner)
            }
            Kind::File | Kind::Other => Err(BundleError::Kind(path.into())),
        }
    }
}

/// `dassie/<plain names>`, as the path under `dassie/`.
fn under_root(path: &str) -> Option<&str> {
    if path.len() > MAX_PATH {
        return None;
    }
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    let inner = match trimmed.strip_prefix(ROOT)? {
        "" => "",
        rest => rest.strip_prefix('/')?,
    };
    let plain = inner.is_empty() || inner.split('/').all(plain_name);
    plain.then_some(inner)
}

/// Letters, digits, dots, dashes and underscores, not starting with a dot.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
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

    #[test]
    fn admits_a_bundle_as_the_release_build_makes_it() {
        let mut unpacking = Unpacking::new();
        assert_eq!(unpacking.admit("dassie/", Kind::Directory, 0), Ok(""));
        assert_eq!(
            unpacking.admit("dassie/bin/", Kind::Directory, 0),
            Ok("bin")
        );
        assert_eq!(
            unpacking.admit("dassie/bin/dassie", Kind::File, 14_000_000),
            Ok(BINARY)
        );
        assert_eq!(
            unpacking.admit("dassie/share/systemd/dassie.service", Kind::File, 900),
            Ok("share/systemd/dassie.service")
        );
    }

    #[test]
    fn refuses_what_could_leave_the_directory() {
        for path in [
            "",
            "/",
            "/etc/passwd",
            "dassie/../etc/passwd",
            "dassie/bin/../../etc",
            "dassie//bin",
            "dassie/./bin",
            "dassie/.hidden",
            "dassie/bin dassie",
            "dassie\\bin",
            "dassiex/bin",
            "other/bin/dassie",
            "./dassie/bin/dassie",
        ] {
            let mut unpacking = Unpacking::new();
            assert_eq!(
                unpacking.admit(path, Kind::File, 1),
                Err(BundleError::Path(path.into())),
                "{path:?}"
            );
        }
    }

    #[test]
    fn refuses_links_and_misplaced_files() {
        let mut unpacking = Unpacking::new();
        assert_eq!(
            unpacking.admit("dassie/bin/dassie", Kind::Other, 0),
            Err(BundleError::Kind("dassie/bin/dassie".into()))
        );
        assert_eq!(
            unpacking.admit("dassie", Kind::File, 1),
            Err(BundleError::Kind("dassie".into()))
        );
        assert_eq!(
            unpacking.admit("dassie/bin/", Kind::File, 1),
            Err(BundleError::Kind("dassie/bin/".into()))
        );
    }

    #[test]
    fn bounds_entries_and_bytes() {
        let mut unpacking = Unpacking::new();
        for i in 0..MAX_ENTRIES {
            assert!(
                unpacking
                    .admit(&format!("dassie/f{i}"), Kind::File, 1)
                    .is_ok()
            );
        }
        assert_eq!(
            unpacking.admit("dassie/one-more", Kind::File, 1),
            Err(BundleError::TooManyEntries)
        );
        let mut unpacking = Unpacking::new();
        assert!(
            unpacking
                .admit("dassie/a", Kind::File, MAX_UNPACKED)
                .is_ok()
        );
        assert_eq!(
            unpacking.admit("dassie/b", Kind::File, 1),
            Err(BundleError::TooLarge)
        );
        let mut unpacking = Unpacking::new();
        assert_eq!(
            unpacking.admit("dassie/huge", Kind::File, u64::MAX),
            Err(BundleError::TooLarge)
        );
    }

    /// A path component: plain, `.` or `..`, empty, anything, or long.
    fn component() -> impl Strategy<Value = String> {
        prop_oneof![
            8 => "[A-Za-z0-9_.-]{1,12}",
            2 => prop::sample::select(&["", ".", "..", "dassie", "bin", ".hidden"][..])
                .prop_map(String::from),
            1 => "\\PC{0,6}",
            1 => "[a-z]{60,200}",
        ]
    }

    /// A path in a bundle: mostly under `dassie/`, with any components
    /// and separators.
    fn path() -> impl Strategy<Value = String> {
        let root = prop_oneof![
            8 => Just("dassie/"),
            1 => Just("dassie"),
            1 => prop::sample::select(
                &["dassie-0.1.0/", "dassie//", "Dassie/", "/dassie/", "./dassie/", "dassie\\", ""][..]
            ),
        ];
        let separator = prop_oneof![8 => Just("/"), 1 => Just("//"), 1 => Just("\\")];
        let components = proptest::collection::vec((component(), separator), 0..6);
        (root, components, any::<bool>()).prop_map(|(root, components, directory)| {
            let mut path = String::from(root);
            for (index, (component, separator)) in components.iter().enumerate() {
                if index > 0 {
                    path.push_str(separator);
                }
                path.push_str(component);
            }
            if directory {
                path.push('/');
            }
            path
        })
    }

    /// An entry: a path, any kind, and no, few or any number of bytes.
    fn entry() -> impl Strategy<Value = (String, Kind, u64)> {
        let kind = prop::sample::select(&[Kind::Directory, Kind::File, Kind::Other][..]);
        let size = prop_oneof![Just(0), 0..4096u64, 0..=MAX_UNPACKED, any::<u64>()];
        (path(), kind, size)
    }

    proptest! {
        /// Whatever it admits stays under the root: no `..`, no absolute
        /// path, no empty or hidden component, nothing but plain names;
        /// and no more entries or bytes than allowed.
        #[test]
        fn admitted_paths_stay_inside(
            entries in proptest::collection::vec(entry(), 1..=MAX_ENTRIES + 8),
        ) {
            let mut unpacking = Unpacking::new();
            let (mut admitted, mut bytes) = (0, 0u64);
            for (path, kind, size) in &entries {
                let Ok(inner) = unpacking.admit(path, *kind, *size) else {
                    continue;
                };
                admitted += 1;
                if *kind == Kind::File {
                    bytes = bytes.saturating_add(*size);
                }
                prop_assert_ne!(*kind, Kind::Other);
                prop_assert!(path.len() <= MAX_PATH);
                prop_assert!(path.strip_suffix('/').unwrap_or(path).ends_with(inner));
                prop_assert!(!inner.starts_with('/'));
                prop_assert!(inner.split('/').all(|c| inner.is_empty() || (!c.is_empty() && !c.starts_with('.'))));
                prop_assert!(inner.bytes().all(|b| b.is_ascii_alphanumeric() || b"./-_".contains(&b)));
            }
            prop_assert!(admitted <= MAX_ENTRIES);
            prop_assert!(bytes <= MAX_UNPACKED);
        }
    }
}
