//! The syntax of a node's URL (spec/areas/peer-protocol.md#verification).
//! It is part of verifying a link state, so every node agrees on which URLs
//! are valid: `https://`, a lowercase hostname or an IP literal, an optional
//! port, and nothing else.

use alloc::string::{String, ToString};
use core::fmt;

use super::MAX_URL_BYTES;

/// Most bytes in a hostname.
const MAX_HOST: usize = 253;

/// Most bytes in a hostname's label.
const MAX_LABEL: usize = 63;

/// Whether `url` is a valid node URL.
#[must_use]
pub fn is_valid_url(url: &str) -> bool {
    if url.len() > MAX_URL_BYTES {
        return false;
    }
    let Some(authority) = url.strip_prefix("https://") else {
        return false;
    };
    let (host, port) = split_port(authority);
    port.is_none_or(is_valid_port) && (is_hostname(host) || is_ip_literal(host))
}

/// Where a node takes QUIC: the host and port of its URL, read once
/// (spec/areas/peer-protocol.md#quic). The host is a lowercase hostname or
/// an IP literal, an IPv6 one without its brackets. It is written
/// `host:port`, an IPv6 host in brackets, as a hub's acceptances and the
/// configuration's entries write it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint {
    host: String,
    port: u16,
}

impl Endpoint {
    /// The endpoint of a valid node URL ([`is_valid_url`]); `None` for any
    /// other.
    #[must_use]
    pub fn of_url(url: &str) -> Option<Self> {
        if !is_valid_url(url) {
            return None;
        }
        let (host, port) = split_port(url.strip_prefix("https://")?);
        let host = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host);
        Some(Self {
            host: host.to_string(),
            port: port.map_or(Some(443), |port| port.parse().ok())?,
        })
    }

    /// Reads `host:port`, or a host alone for port 443, under the rules of
    /// a node URL's host and port.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::of_url(&alloc::format!("https://{text}"))
    }

    /// The host, an IPv6 one without its brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The node URL of the endpoint, its port left out when it is 443.
    #[must_use]
    pub fn url(&self) -> String {
        if self.port == 443 {
            alloc::format!("https://{}", self.authority_host())
        } else {
            alloc::format!("https://{self}")
        }
    }

    /// The host as a URL's authority writes it.
    fn authority_host(&self) -> String {
        if self.host.contains(':') {
            alloc::format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.authority_host(), self.port)
    }
}

/// Splits off `:port`, minding the colons of an IPv6 literal.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    if authority.starts_with('[') {
        return match authority.find("]:") {
            Some(end) => (
                authority.get(..=end).unwrap_or_default(),
                authority.get(end + 2..),
            ),
            None => (authority, None),
        };
    }
    match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

fn is_valid_port(port: &str) -> bool {
    !port.starts_with('0')
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port > 0)
}

fn is_hostname(host: &str) -> bool {
    host.len() <= MAX_HOST
        && !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        // A name of digits and dots is an IPv4 literal, checked as one.
        && !host.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
}

fn is_ip_literal(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        // Lowercase only, so one address has one spelling.
        return inner.parse::<core::net::Ipv6Addr>().is_ok()
            && !inner.bytes().any(|byte| byte.is_ascii_uppercase());
    }
    host.parse::<core::net::Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_an_origin_and_nothing_else() {
        for valid in [
            "https://a.example",
            "https://d1.localhost:5001",
            "https://localhost:18943",
            "https://127.0.0.1:443",
            "https://[::1]:8443",
            "https://xn--bcher-kva.example",
        ] {
            assert!(is_valid_url(valid), "{valid}");
        }
        for invalid in [
            "http://a.example",
            "https://A.example",
            "https://a.example/",
            "https://a.example/peer",
            "https://user@a.example",
            "https://a.example?x",
            "https://a.example:0",
            "https://a.example:080",
            "https://a.example:65536",
            "https://-a.example",
            "https://a..example",
            "https://999.1.1.1",
            "https://[::G]",
            "https://[::A]",
            "https://",
        ] {
            assert!(!is_valid_url(invalid), "{invalid}");
        }
        assert!(!is_valid_url(&alloc::format!(
            "https://{}.example",
            "a".repeat(300)
        )));
    }

    // An endpoint is read from a node URL alone, and written back to it.
    #[test]
    fn endpoints_are_a_urls_host_and_port() {
        for (url, host, port, text) in [
            ("https://a.example", "a.example", 443, "a.example:443"),
            (
                "https://a.example:8443",
                "a.example",
                8443,
                "a.example:8443",
            ),
            ("https://192.0.2.1:7", "192.0.2.1", 7, "192.0.2.1:7"),
            (
                "https://[2001:db8::1]",
                "2001:db8::1",
                443,
                "[2001:db8::1]:443",
            ),
            ("https://[::1]:8443", "::1", 8443, "[::1]:8443"),
        ] {
            let endpoint = Endpoint::of_url(url).unwrap();
            assert_eq!((endpoint.host(), endpoint.port()), (host, port), "{url}");
            assert_eq!(endpoint.to_string(), text);
            assert_eq!(Endpoint::parse(text), Some(endpoint.clone()));
            assert_eq!(Endpoint::of_url(&endpoint.url()), Some(endpoint));
        }
        assert_eq!(
            Endpoint::of_url("https://a.example:443").unwrap().url(),
            "https://a.example"
        );
        for invalid in [
            "http://a.example",
            "https://A.example",
            "https://a.example/",
            "",
        ] {
            assert_eq!(Endpoint::of_url(invalid), None, "{invalid}");
        }
        assert_eq!(Endpoint::parse("a.example:0"), None);
        assert_eq!(Endpoint::parse("a.example/x"), None);
    }
}
