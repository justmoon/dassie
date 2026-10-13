//! The same-origin rule for the owner's endpoints.
//!
//! The original checked no `Origin` on `/rpc` or the `/api/*` POSTs, and
//! relied on `SameSite=Strict` cookies alone (spec 2 §6.6, §16, RT-6). Now a
//! request to those endpoints must carry an `Origin` that is exactly
//! `https://` plus its `Host`, so only pages served by the node itself can
//! use them.
//!
//! That alone would let a DNS-rebinding page through: an attacker's name
//! that resolves to the node gives a matching `Origin` and `Host`. So the
//! `Host` must also be one the node answers to: its configured hostname,
//! `localhost`, or an IP address.

use alloc::string::{String, ToString};
use core::fmt;
use core::net::{IpAddr, Ipv6Addr};

use crate::config::Hostname;
use crate::urls::DEFAULT_HTTPS_PORT;

/// The host part of an authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    /// A DNS name, lowercase.
    Name(String),
    /// An IPv4 or IPv6 address.
    Ip(IpAddr),
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name),
            Self::Ip(IpAddr::V4(address)) => write!(f, "{address}"),
            Self::Ip(IpAddr::V6(address)) => write!(f, "[{address}]"),
        }
    }
}

/// `host[:port]`, as in a `Host` header or an origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    /// The host.
    pub host: Host,
    /// The port, if one was given.
    pub port: Option<u16>,
}

/// Parses `name[:port]`, `ipv4[:port]` or `[ipv6][:port]`.
#[must_use]
pub fn parse_authority(text: &str) -> Option<Authority> {
    if let Some(rest) = text.strip_prefix('[') {
        let (address, after) = rest.split_once(']')?;
        let host = Host::Ip(IpAddr::V6(address.parse::<Ipv6Addr>().ok()?));
        let port = match after {
            "" => None,
            _ => Some(parse_port(after.strip_prefix(':')?)?),
        };
        return Some(Authority { host, port });
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => (host, Some(parse_port(port)?)),
        None => (text, None),
    };
    let host = match host.parse::<core::net::Ipv4Addr>() {
        Ok(address) => Host::Ip(IpAddr::V4(address)),
        Err(_) => Host::Name(Hostname::parse(host).ok()?.as_str().to_string()),
    };
    Some(Authority { host, port })
}

fn parse_port(text: &str) -> Option<u16> {
    let valid = (1..=5).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit());
    let port = text.parse::<u16>().ok()?;
    (valid && port != 0).then_some(port)
}

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginError {
    /// No `Host` header, or not a valid authority.
    InvalidHost,
    /// A name the node does not answer to.
    HostNotAllowed,
    /// No `Origin` header, or not an `https://` origin.
    InvalidOrigin,
    /// The origin is a different site than the host.
    CrossOrigin,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidHost => "missing or invalid Host header",
            Self::HostNotAllowed => "this node does not answer to that host name",
            Self::InvalidOrigin => "missing or invalid Origin header",
            Self::CrossOrigin => "cross-origin requests are not allowed",
        })
    }
}

/// Whether the node answers to `host` at all: its hostname, `localhost`,
/// or an IP address.
#[must_use]
pub fn host_allowed(host: &Host, hostname: Option<&Hostname>) -> bool {
    match host {
        Host::Ip(_) => true,
        Host::Name(name) => name == "localhost" || hostname.is_some_and(|h| h.as_str() == name),
    }
}

/// Checks that `origin` is the node's own origin for a request to `host`.
pub fn check_same_origin(
    origin: Option<&str>,
    host: Option<&str>,
    hostname: Option<&Hostname>,
) -> Result<(), OriginError> {
    let host = host
        .and_then(parse_authority)
        .ok_or(OriginError::InvalidHost)?;
    if !host_allowed(&host.host, hostname) {
        return Err(OriginError::HostNotAllowed);
    }
    let origin = origin
        .and_then(|origin| origin.strip_prefix("https://"))
        .and_then(parse_authority)
        .ok_or(OriginError::InvalidOrigin)?;
    let same = origin.host == host.host
        && origin.port.unwrap_or(DEFAULT_HTTPS_PORT) == host.port.unwrap_or(DEFAULT_HTTPS_PORT);
    if same {
        Ok(())
    } else {
        Err(OriginError::CrossOrigin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostname() -> Hostname {
        Hostname::parse("node.example").unwrap_or_else(|error| panic!("{error}"))
    }

    fn check(origin: Option<&str>, host: Option<&str>) -> Result<(), OriginError> {
        check_same_origin(origin, host, Some(&hostname()))
    }

    #[test]
    fn authorities() {
        let parsed = parse_authority("Node.Example:8443");
        assert_eq!(
            parsed,
            Some(Authority {
                host: Host::Name("node.example".into()),
                port: Some(8443)
            })
        );
        let parsed = parse_authority("[::1]:443").map(|a| a.host.to_string());
        assert_eq!(parsed.as_deref(), Some("[::1]"));
        assert_eq!(parse_authority("10.0.0.1").map(|a| a.port), Some(None));
        for refused in [
            "", "::1", "[::1", "[::1]x", "a:0", "a:65536", "a:+1", "a b", "a:",
        ] {
            assert_eq!(parse_authority(refused), None, "{refused}");
        }
    }

    // RT-6: the original accepted any Origin, or none.
    #[test]
    fn same_origin_passes() {
        assert_eq!(
            check(Some("https://node.example"), Some("node.example")),
            Ok(())
        );
        assert_eq!(
            check(Some("https://node.example:443"), Some("node.example")),
            Ok(())
        );
        assert_eq!(
            check(Some("https://localhost:8443"), Some("localhost:8443")),
            Ok(())
        );
        assert_eq!(
            check(Some("https://[::1]:8443"), Some("[::1]:8443")),
            Ok(())
        );
        assert_eq!(
            check(Some("https://10.0.0.1"), Some("10.0.0.1:443")),
            Ok(())
        );
    }

    #[test]
    fn other_origins_fail() {
        let host = Some("node.example");
        assert_eq!(check(None, host), Err(OriginError::InvalidOrigin));
        assert_eq!(check(Some("null"), host), Err(OriginError::InvalidOrigin));
        assert_eq!(
            check(Some("http://node.example"), host),
            Err(OriginError::InvalidOrigin)
        );
        assert_eq!(
            check(Some("https://node.example/x"), host),
            Err(OriginError::InvalidOrigin)
        );
        assert_eq!(
            check(Some("https://evil.example"), host),
            Err(OriginError::CrossOrigin)
        );
        assert_eq!(
            check(Some("https://node.example:8443"), host),
            Err(OriginError::CrossOrigin)
        );
    }

    #[test]
    fn rebinding_hosts_fail() {
        let origin = Some("https://evil.example");
        assert_eq!(
            check(origin, Some("evil.example")),
            Err(OriginError::HostNotAllowed)
        );
        assert_eq!(check(origin, None), Err(OriginError::InvalidHost));
        assert_eq!(
            check_same_origin(Some("https://node.example"), Some("node.example"), None),
            Err(OriginError::HostNotAllowed)
        );
    }
}
