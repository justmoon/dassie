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

/// A port of 1 to 5 digits, without a sign or leading zeros, so that each
/// port has one spelling.
fn parse_port(text: &str) -> Option<u16> {
    let valid = (1..=5).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && !text.starts_with('0');
    let port = text.parse::<u16>().ok()?;
    (valid && port != 0).then_some(port)
}

/// Text that is nearly an authority, for the properties of every parser of
/// a `Host` or `Origin`: names with dots, hyphens, unicode or overlong
/// labels, IPv4 and IPv6 addresses with or without brackets, and ports with
/// signs, leading zeros, too many digits or none.
#[cfg(test)]
pub(crate) fn near_authority() -> proptest::strategy::BoxedStrategy<String> {
    use alloc::format;
    use core::net::Ipv4Addr;
    use proptest::prelude::*;

    let host = prop_oneof![
        "[a-zA-Z0-9-]{0,10}(\\.[a-zA-Z0-9-]{0,10}){0,3}",
        "[a-zA-Z0-9é_-]{1,8}(\\.[a-zA-Z0-9é_-]{1,8}){0,2}",
        "[a-z]{60,66}\\.[a-z]{2,4}",
        Just("localhost".to_string()),
        any::<Ipv4Addr>().prop_map(|address| address.to_string()),
        "[0-9]{1,4}(\\.[0-9]{1,4}){0,4}",
        any::<Ipv6Addr>().prop_map(|address| format!("[{address}]")),
        any::<Ipv6Addr>().prop_map(|address| address.to_string()),
        "\\[?[0-9a-fA-F:.%]{0,40}\\]?",
    ];
    let port = prop_oneof![
        Just(String::new()),
        Just(":".to_string()),
        any::<u16>().prop_map(|port| format!(":{port}")),
        ":[+-]?0{0,3}[0-9]{0,6}",
        ":[0-9a-z: ]{0,4}",
    ];
    prop_oneof![
        (host, port).prop_map(|(host, port)| format!("{host}{port}")),
        any::<String>(),
    ]
    .boxed()
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
    use alloc::format;
    use proptest::prelude::*;

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

    // Found by `authorities_never_panic`: "a:443", "a:0443" and "a:00443"
    // were the same authority.
    #[test]
    fn ports_with_leading_zeros_are_refused() {
        for refused in ["a:00", "a:01", "a:0443", "a:00443", "[::1]:0443"] {
            assert_eq!(parse_authority(refused), None, "{refused}");
        }
        assert_eq!(
            check(Some("https://node.example:0443"), Some("node.example")),
            Err(OriginError::InvalidOrigin)
        );
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

    /// An authority as the node writes it.
    fn format_authority(authority: &Authority) -> String {
        match authority.port {
            Some(port) => format!("{}:{port}", authority.host),
            None => authority.host.to_string(),
        }
    }

    proptest! {
        // Whatever parses re-formats canonically: the text the node writes
        // parses to the same authority. Names may differ in case and IPv6
        // addresses in notation, but a port has one spelling.
        #[test]
        fn authorities_never_panic(text in near_authority()) {
            if let Some(authority) = parse_authority(&text) {
                prop_assert_eq!(parse_authority(&format_authority(&authority)), Some(authority.clone()));
                if let Some(port) = authority.port {
                    prop_assert!(port != 0);
                    prop_assert!(text.ends_with(&format!(":{port}")), "{text}");
                }
                if let Host::Name(name) = &authority.host {
                    let canonical = Hostname::parse(name).map(|name| name.as_str().to_string());
                    prop_assert_eq!(canonical.as_deref(), Ok(name.as_str()));
                }
            }
        }

        #[test]
        fn ports_never_panic(text in prop_oneof![any::<String>(), "[+-]?0{0,3}[0-9]{0,6}"]) {
            if let Some(port) = parse_port(&text) {
                prop_assert_eq!(port.to_string(), text);
            }
        }

        // The node refuses port 0, signs and leading zeros.
        #[test]
        fn ports_have_one_spelling(port: u16, zeros in "0{1,3}") {
            prop_assert_eq!(parse_port(&port.to_string()), (port != 0).then_some(port));
            prop_assert_eq!(parse_port(&format!("+{port}")), None);
            prop_assert_eq!(parse_port(&format!("{zeros}{port}")), None);
        }

        // A request passes only if its origin is https:// and its host, up
        // to case and the default port, and the node answers to that host.
        #[test]
        fn origin_checks_never_panic(
            origin in prop_oneof![
                near_authority().prop_map(|authority| format!("https://{authority}")),
                near_authority().prop_map(|authority| format!("http://{authority}")),
                any::<String>(),
            ],
            host in near_authority(),
            same: bool,
            configured: bool,
        ) {
            let origin = if same { format!("https://{host}") } else { origin };
            let parsed_host = parse_authority(&host);
            let hostname = match &parsed_host {
                Some(Authority { host: Host::Name(name), .. }) => Hostname::parse(name).ok(),
                _ => Hostname::parse("node.example").ok(),
            }
            .filter(|_| configured);
            let result = check_same_origin(Some(&origin), Some(&host), hostname.as_ref());
            if result.is_ok() {
                let parsed_origin = origin.strip_prefix("https://").and_then(parse_authority);
                prop_assert!(parsed_origin.is_some() && parsed_host.is_some());
                if let (Some(origin), Some(host)) = (parsed_origin, &parsed_host) {
                    prop_assert_eq!(&origin.host, &host.host);
                    prop_assert_eq!(
                        origin.port.unwrap_or(DEFAULT_HTTPS_PORT),
                        host.port.unwrap_or(DEFAULT_HTTPS_PORT)
                    );
                    prop_assert!(host_allowed(&host.host, hostname.as_ref()));
                }
            }
            if same && configured && parsed_host.is_some() {
                prop_assert_eq!(result, Ok(()));
            }
            prop_assert!(check_same_origin(None, Some(&host), hostname.as_ref()).is_err());
        }
    }
}
