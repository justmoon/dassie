//! Where the mirror keeps what: the original's paths
//! ([a frozen surface](../../../spec/overview.md#frozen-surfaces)),
//! plus the signed sums.
//!
//! ```text
//! meta/latest                               the newest release's version
//! meta/canary                               the newest canary's
//! <version>/dassie-<version>-linux-<arch>.tar.gz
//! <version>/dassie-<version>-linux-<arch>.tar.xz
//! <version>/flake.tar.gz                     the flake for NixOS
//! <version>/SHA256SUMS                      the SHA-256 of both
//! <version>/SHA256SUMS.minisig              its signature
//! ```

use alloc::format;
use alloc::string::String;

use crate::{Channel, Version};

/// The mirror the installer and the updater use unless told otherwise.
pub const DEFAULT_MIRROR: &str = "https://get.dassie.land";

/// The flake NixOS takes a release from: its module, and a package of the
/// release's bundles by their sums.
pub const FLAKE: &str = "flake.tar.gz";

/// The sums of a release's bundles and its flake.
pub const SUMS: &str = "SHA256SUMS";

/// Their signature.
pub const SIGNATURE: &str = "SHA256SUMS.minisig";

/// A platform a release has a bundle for, all of them Linux.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    /// x86-64.
    X64,
    /// AArch64.
    Arm64,
}

impl Arch {
    /// Every platform, as each release builds them.
    pub const ALL: [Arch; 2] = [Arch::X64, Arch::Arm64];

    /// The name in bundle names, as the original's.
    pub fn name(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }

    /// The platform this binary runs on, if releases have one for it.
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64)
        } else {
            None
        }
    }
}

/// How a bundle is compressed: each release has both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compression {
    /// gzip, which the updater reads.
    Gz,
    /// xz, which the installer reads: smaller, and `tar` reads it.
    Xz,
}

impl Compression {
    /// The file name's extension, after `.tar.`.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gz => "gz",
            Self::Xz => "xz",
        }
    }
}

/// `meta/<channel>`: the newest version on a channel.
pub fn meta(channel: Channel) -> String {
    format!("meta/{}", channel.name())
}

/// A bundle's file name: `dassie-<version>-linux-<arch>.tar.<gz|xz>`.
pub fn bundle(version: &Version, arch: Arch, compression: Compression) -> String {
    format!(
        "dassie-{version}-linux-{}.tar.{}",
        arch.name(),
        compression.extension()
    )
}

/// The path of a release's file, under `<version>/`.
pub fn in_release(version: &Version, file: &str) -> String {
    format!("{version}/{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_the_originals() {
        let Ok(version) = Version::parse("0.1.0") else {
            panic!("0.1.0 is a version");
        };
        assert_eq!(meta(Channel::Latest), "meta/latest");
        assert_eq!(meta(Channel::Canary), "meta/canary");
        assert_eq!(
            in_release(&version, &bundle(&version, Arch::X64, Compression::Xz)),
            "0.1.0/dassie-0.1.0-linux-x64.tar.xz"
        );
        assert_eq!(
            bundle(&version, Arch::Arm64, Compression::Gz),
            "dassie-0.1.0-linux-arm64.tar.gz"
        );
        assert_eq!(in_release(&version, SIGNATURE), "0.1.0/SHA256SUMS.minisig");
    }

    #[test]
    fn knows_where_it_runs() {
        let expected = match std::env::consts::ARCH {
            "x86_64" => Some(Arch::X64),
            "aarch64" => Some(Arch::Arm64),
            _ => None,
        };
        assert_eq!(Arch::current(), expected);
    }
}
