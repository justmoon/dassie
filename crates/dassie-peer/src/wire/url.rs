//! The syntax of a node's URL (spec 7 §4.7). It is part of verifying a link
//! state, so every node agrees on which URLs are valid: `https://`, a
//! lowercase hostname or an IP literal, an optional port, and nothing else.

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
}
