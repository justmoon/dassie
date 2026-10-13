//! The node's configuration, as validated values.
//!
//! The original stored whatever the RPC was given: an unvalidated hostname,
//! ports only the database could change, and a `url` computed once at load
//! that went stale when the hostname changed (spec 2 §4.2, RT-3, RT-4).
//! Here each value is checked when it is made, and nothing derived is stored:
//! URLs are computed from the current values every time ([`crate::urls`]).

use alloc::string::{String, ToString};
use core::fmt;
use core::net::IpAddr;

/// Let's Encrypt's production directory, the default.
pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// Let's Encrypt's staging directory, for testing.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Longest DNS name, in bytes (RFC 1035 §2.3.4).
pub const MAX_HOSTNAME_BYTES: usize = 253;
/// Longest DNS label, in bytes.
pub const MAX_LABEL_BYTES: usize = 63;
/// Longest alias, in bytes of UTF-8.
pub const MAX_ALIAS_BYTES: usize = 64;
/// Longest ACME directory URL, in bytes.
pub const MAX_DIRECTORY_URL_BYTES: usize = 2048;

/// A configuration value was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Not a DNS name (RFC 1123), or an IP address.
    InvalidHostname,
    /// Too long, or contains control characters.
    InvalidAlias,
    /// Port 0.
    InvalidPort,
    /// The HTTP and HTTPS ports are the same.
    SamePorts,
    /// Not an `https://` URL of reasonable length.
    InvalidDirectoryUrl,
    /// Not `test` or `live`.
    InvalidRealm,
    /// The live realm is refused until decision D8's conditions are met.
    LiveRealmDisabled,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidHostname => "not a valid DNS name",
            Self::InvalidAlias => "the alias must be at most 64 bytes without control characters",
            Self::InvalidPort => "ports must be between 1 and 65535",
            Self::SamePorts => "the HTTP and HTTPS ports must differ",
            Self::InvalidDirectoryUrl => "the ACME directory must be an https:// URL",
            Self::InvalidRealm => "the realm must be test or live",
            Self::LiveRealmDisabled => "the live realm is not supported yet",
        })
    }
}

/// The node's public DNS name: lowercase, RFC 1123, not an IP address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hostname(String);

impl Hostname {
    /// Validates `text` and lowercases it.
    ///
    /// Labels are 1 to 63 letters, digits and hyphens, without a leading or
    /// trailing hyphen. A last label of only digits is refused, since the
    /// name would then read as an IPv4 address. Single-label names such as
    /// `localhost` are allowed.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let valid = !text.is_empty()
            && text.len() <= MAX_HOSTNAME_BYTES
            && text.split('.').all(valid_label)
            && text
                .rsplit('.')
                .next()
                .is_some_and(|last| !last.bytes().all(|byte| byte.is_ascii_digit()))
            && text.parse::<IpAddr>().is_err();
        if valid {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(ConfigError::InvalidHostname)
        }
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Hostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_LABEL_BYTES
        && bytes
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
}

/// Which network the node belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Realm {
    /// The test network, with test assets. The only realm enabled.
    #[default]
    Test,
    /// The real network. Refused by [`Realm::parse_settable`] (D8).
    Live,
}

impl Realm {
    /// The name stored in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Live => "live",
        }
    }

    /// Reads a stored name.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        match text {
            "test" => Ok(Self::Test),
            "live" => Ok(Self::Live),
            _ => Err(ConfigError::InvalidRealm),
        }
    }

    /// Checks that the owner may switch to this realm.
    pub const fn check_settable(self) -> Result<Self, ConfigError> {
        match self {
            Self::Test => Ok(self),
            Self::Live => Err(ConfigError::LiveRealmDisabled),
        }
    }

    /// The first segment of the node's ILP address: `test` or `g`.
    #[must_use]
    pub const fn ilp_allocation_scheme(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Live => "g",
        }
    }
}

/// A name the node shows next to its ID: at most [`MAX_ALIAS_BYTES`] of
/// UTF-8 without control characters. Empty by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Alias(String);

impl Alias {
    /// Validates `text`.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        if text.len() <= MAX_ALIAS_BYTES && !text.chars().any(char::is_control) {
            Ok(Self(text.to_string()))
        } else {
            Err(ConfigError::InvalidAlias)
        }
    }

    /// The alias as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The ports the standalone runtime listens on. Under systemd the socket
/// units decide, and these are not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    http: u16,
    https: u16,
}

impl Ports {
    /// The defaults, 80 and 443.
    pub const DEFAULT: Self = Self {
        http: 80,
        https: 443,
    };

    /// Validates a pair of ports.
    pub fn new(http: u32, https: u32) -> Result<Self, ConfigError> {
        let http = u16::try_from(http).map_err(|_| ConfigError::InvalidPort)?;
        let https = u16::try_from(https).map_err(|_| ConfigError::InvalidPort)?;
        if http == 0 || https == 0 {
            return Err(ConfigError::InvalidPort);
        }
        if http == https {
            return Err(ConfigError::SamePorts);
        }
        Ok(Self { http, https })
    }

    /// The HTTP port.
    #[must_use]
    pub const fn http(self) -> u16 {
        self.http
    }

    /// The HTTPS port.
    #[must_use]
    pub const fn https(self) -> u16 {
        self.https
    }
}

/// An ACME directory URL: `https://`, no whitespace or control characters,
/// at most [`MAX_DIRECTORY_URL_BYTES`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryUrl(String);

impl DirectoryUrl {
    /// Validates `text`.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let rest = text.strip_prefix("https://").unwrap_or_default();
        let valid = !rest.is_empty()
            && !rest.starts_with('/')
            && text.len() <= MAX_DIRECTORY_URL_BYTES
            && text.bytes().all(|byte| byte.is_ascii_graphic());
        if valid {
            Ok(Self(text.to_string()))
        } else {
            Err(ConfigError::InvalidDirectoryUrl)
        }
    }

    /// The URL as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DirectoryUrl {
    fn default() -> Self {
        Self(LETS_ENCRYPT_PRODUCTION.to_string())
    }
}

/// Everything the owner configures, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeConfig {
    /// The network.
    pub realm: Realm,
    /// The public DNS name, once configured.
    pub hostname: Option<Hostname>,
    /// The standalone runtime's ports.
    pub ports: Ports,
    /// Shown to other nodes.
    pub alias: Alias,
    /// Where certificates are renewed.
    pub acme_directory: DirectoryUrl,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            realm: Realm::Test,
            hostname: None,
            ports: Ports::DEFAULT,
            alias: Alias::default(),
            acme_directory: DirectoryUrl::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames() {
        let accepted = [
            "example.com",
            "D1.localhost",
            "localhost",
            "a-b.c0",
            "xn--bcher-kva.example",
        ];
        for name in accepted {
            let parsed = Hostname::parse(name);
            assert_eq!(parsed.map(|h| h.0), Ok(name.to_ascii_lowercase()), "{name}");
        }
        let long_label = "a".repeat(64);
        let refused = [
            "",
            ".",
            "example.com.",
            "-a.com",
            "a-.com",
            "a..com",
            "a_b.com",
            "1.2.3.4",
            "host.123",
            "::1",
            "exa mple.com",
            "über.com",
            &long_label,
        ];
        for name in refused {
            assert_eq!(
                Hostname::parse(name),
                Err(ConfigError::InvalidHostname),
                "{name}"
            );
        }
    }

    // The original accepted any string from `dassie init`, including an empty
    // one after Ctrl+D handling, and stored it (spec 2 §4.5).
    #[test]
    fn hostname_of_253_bytes_but_not_254() {
        let label = "a".repeat(63);
        let name = [label.as_str(); 4].join(".");
        assert_eq!(name.len(), 255);
        assert!(Hostname::parse(&name).is_err());
        assert!(Hostname::parse(name.get(2..).unwrap_or_default()).is_ok());
    }

    #[test]
    fn realms() {
        assert_eq!(Realm::parse("test"), Ok(Realm::Test));
        assert_eq!(Realm::Test.check_settable(), Ok(Realm::Test));
        assert_eq!(
            Realm::Live.check_settable(),
            Err(ConfigError::LiveRealmDisabled)
        );
        assert_eq!(Realm::Live.ilp_allocation_scheme(), "g");
        assert_eq!(Realm::parse("main"), Err(ConfigError::InvalidRealm));
    }

    #[test]
    fn ports() {
        assert_eq!(Ports::new(8080, 8443).map(Ports::https), Ok(8443));
        assert_eq!(Ports::new(0, 443), Err(ConfigError::InvalidPort));
        assert_eq!(Ports::new(80, 65_536), Err(ConfigError::InvalidPort));
        assert_eq!(Ports::new(443, 443), Err(ConfigError::SamePorts));
    }

    #[test]
    fn aliases_and_directories() {
        assert!(Alias::parse("Stefan's node").is_ok());
        assert!(Alias::parse("tab\there").is_err());
        assert!(Alias::parse(&"x".repeat(65)).is_err());
        assert!(DirectoryUrl::parse(LETS_ENCRYPT_STAGING).is_ok());
        assert!(DirectoryUrl::parse("https://localhost:14000/dir").is_ok());
        for refused in [
            "http://example.com/dir",
            "https://",
            "https:///dir",
            "https://a b",
        ] {
            assert!(DirectoryUrl::parse(refused).is_err(), "{refused}");
        }
    }
}
