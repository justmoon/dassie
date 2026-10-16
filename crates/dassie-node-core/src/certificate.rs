//! Which certificate the HTTPS listener serves, and when to replace it.
//!
//! The listener always has a certificate. Before a real one is installed it
//! serves a self-signed one, which the original regenerated, with a new
//! fingerprint, every time its HTTPS actor restarted (RT-14). Here it is
//! made once, stored, and replaced only when the names it must cover change
//! or it nears its end.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::DAY_MS;
use crate::config::Hostname;

/// How long a self-signed certificate is made for.
pub const SELF_SIGNED_VALIDITY_MS: u64 = 10 * 365 * DAY_MS;

/// A self-signed certificate this close to its end is made again.
pub const SELF_SIGNED_RENEW_BEFORE_MS: u64 = 30 * DAY_MS;

/// Where the certificate came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificateSource {
    /// Made by the node, until a real one is installed.
    SelfSigned,
    /// Issued through ACME, and renewed by the node.
    Acme,
    /// Installed by the owner; the node does not renew it.
    Manual,
}

impl CertificateSource {
    /// The name stored in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelfSigned => "self-signed",
            Self::Acme => "acme",
            Self::Manual => "manual",
        }
    }

    /// Reads a stored name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "self-signed" => Some(Self::SelfSigned),
            "acme" => Some(Self::Acme),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }

    /// Whether browsers may be told to insist on HTTPS (HSTS). Not with a
    /// self-signed certificate: the owner would have no way past the warning
    /// once a browser has seen the header.
    #[must_use]
    pub const fn allows_hsts(self) -> bool {
        !matches!(self, Self::SelfSigned)
    }
}

/// What the node knows of a certificate, read from it when it was stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    /// Where it came from.
    pub source: CertificateSource,
    /// The DNS names and IP addresses it is valid for, as text.
    pub names: Vec<String>,
    /// Valid from, in ms since the Unix epoch.
    pub not_before_ms: u64,
    /// Valid until, in ms since the Unix epoch.
    pub not_after_ms: u64,
}

impl CertificateInfo {
    /// Whether it is valid for `hostname`: an exact name, or a wildcard
    /// `*.rest` matching exactly one label in front of `rest`.
    #[must_use]
    pub fn covers(&self, hostname: &Hostname) -> bool {
        let name = hostname.as_str();
        self.names.iter().any(|candidate| {
            let candidate = candidate.to_ascii_lowercase();
            match candidate.strip_prefix("*.") {
                Some(rest) => name
                    .split_once('.')
                    .is_some_and(|(label, parent)| !label.is_empty() && parent == rest),
                None => candidate == name,
            }
        })
    }
}

/// The names a self-signed certificate covers: the hostname if there is one,
/// then `localhost` and the loopback addresses.
#[must_use]
pub fn self_signed_names(hostname: Option<&Hostname>) -> Vec<String> {
    let mut names = vec![];
    if let Some(hostname) = hostname {
        names.push(hostname.as_str().to_string());
    }
    for name in ["localhost", "127.0.0.1", "::1"] {
        names.push(name.to_string());
    }
    names
}

/// What to do with the stored certificate, at start or after the hostname
/// changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificateAction {
    /// Serve it.
    Keep,
    /// Make a new self-signed certificate and serve that. The ACME renewal
    /// decides separately whether to replace it with a real one.
    MakeSelfSigned,
}

/// Decides [`CertificateAction`] for the stored certificate.
///
/// - Without one, or with a self-signed one that no longer covers the right
///   names or is near its end, make a self-signed one.
/// - A certificate from ACME that does not cover the hostname any more (the
///   owner changed it) is no use to browsers: fall back to self-signed until
///   the renewal gets one for the new name.
/// - A manual certificate is the owner's business and is kept.
#[must_use]
pub fn decide(
    stored: Option<&CertificateInfo>,
    hostname: Option<&Hostname>,
    now_ms: u64,
) -> CertificateAction {
    let Some(stored) = stored else {
        return CertificateAction::MakeSelfSigned;
    };
    let fine = match stored.source {
        CertificateSource::SelfSigned => {
            stored.names == self_signed_names(hostname)
                && stored.not_after_ms.saturating_sub(now_ms) > SELF_SIGNED_RENEW_BEFORE_MS
        }
        CertificateSource::Acme => hostname.is_some_and(|hostname| stored.covers(hostname)),
        CertificateSource::Manual => true,
    };
    if fine {
        CertificateAction::Keep
    } else {
        CertificateAction::MakeSelfSigned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::near_hostname;
    use alloc::format;
    use proptest::prelude::*;

    fn hostname(text: &str) -> Hostname {
        Hostname::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    fn info(source: CertificateSource, names: &[&str], not_after_ms: u64) -> CertificateInfo {
        CertificateInfo {
            source,
            names: names.iter().map(|name| (*name).to_string()).collect(),
            not_before_ms: 0,
            not_after_ms,
        }
    }

    #[test]
    fn coverage() {
        let cert = info(
            CertificateSource::Acme,
            &["node.example", "*.Wild.Example"],
            0,
        );
        assert!(cert.covers(&hostname("node.example")));
        assert!(cert.covers(&hostname("a.wild.example")));
        assert!(!cert.covers(&hostname("wild.example")));
        assert!(!cert.covers(&hostname("a.b.wild.example")));
        assert!(!cert.covers(&hostname("other.example")));
    }

    // RT-14: the original made a new self-signed certificate at every start.
    #[test]
    fn a_current_self_signed_certificate_is_kept() {
        let name = hostname("node.example");
        let names = self_signed_names(Some(&name));
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let cert = info(
            CertificateSource::SelfSigned,
            &names,
            SELF_SIGNED_VALIDITY_MS,
        );
        assert_eq!(decide(Some(&cert), Some(&name), 0), CertificateAction::Keep);
        let other = hostname("other.example");
        assert_eq!(
            decide(Some(&cert), Some(&other), 0),
            CertificateAction::MakeSelfSigned
        );
        let late = SELF_SIGNED_VALIDITY_MS - SELF_SIGNED_RENEW_BEFORE_MS;
        assert_eq!(
            decide(Some(&cert), Some(&name), late),
            CertificateAction::MakeSelfSigned
        );
        assert_eq!(decide(None, None, 0), CertificateAction::MakeSelfSigned);
    }

    #[test]
    fn an_acme_certificate_for_another_name_is_dropped() {
        let cert = info(CertificateSource::Acme, &["old.example"], u64::MAX);
        let old = hostname("old.example");
        let new = hostname("new.example");
        assert_eq!(decide(Some(&cert), Some(&old), 0), CertificateAction::Keep);
        assert_eq!(
            decide(Some(&cert), Some(&new), 0),
            CertificateAction::MakeSelfSigned
        );
        let manual = info(CertificateSource::Manual, &["old.example"], 0);
        assert_eq!(
            decide(Some(&manual), Some(&new), 0),
            CertificateAction::Keep
        );
    }

    #[test]
    fn hsts_only_for_real_certificates() {
        assert!(!CertificateSource::SelfSigned.allows_hsts());
        assert!(CertificateSource::Acme.allows_hsts());
        for source in [
            CertificateSource::SelfSigned,
            CertificateSource::Acme,
            CertificateSource::Manual,
        ] {
            assert_eq!(CertificateSource::parse(source.as_str()), Some(source));
        }
    }

    /// Names as a certificate might carry them: wildcards of any shape,
    /// host names in either case, and arbitrary text.
    fn certificate_name() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<String>(),
            near_hostname(),
            "\\*(\\.[a-zA-Z0-9*-]{0,8}){0,3}\\.?",
            "[a-z*]{0,3}\\*[a-z.]{0,10}",
        ]
    }

    proptest! {
        // A certificate covers a host name only through a name that is it,
        // in any case, or `*.` and the host name less its first label.
        #[test]
        fn coverage_never_panics(
            hostname in near_hostname().prop_filter_map("not a host name", |text| Hostname::parse(&text).ok()),
            mut names in proptest::collection::vec(certificate_name(), 0..5),
            exact: bool,
            wildcard: bool,
        ) {
            let name = hostname.as_str();
            let parent = name.split_once('.').map(|(_, parent)| parent);
            if exact {
                names.push(name.to_ascii_uppercase());
            }
            if let (true, Some(parent)) = (wildcard, parent) {
                names.push(format!("*.{}", parent.to_ascii_uppercase()));
            }
            let expected = names.iter().map(|candidate| candidate.to_ascii_lowercase()).any(|candidate| {
                candidate == name || parent.is_some_and(|parent| candidate.strip_prefix("*.") == Some(parent))
            });
            let certificate = CertificateInfo {
                source: CertificateSource::Acme,
                names,
                not_before_ms: 0,
                not_after_ms: 0,
            };
            prop_assert_eq!(certificate.covers(&hostname), expected);
        }
    }
}
