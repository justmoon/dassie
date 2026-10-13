//! The URLs the node prints and redirects to.
//!
//! The original got these wrong in three ways (spec 2 §4.3, §6.5):
//!
//! - the HTTP→HTTPS redirect used the request URL's host, which its HTTP
//!   adapter always set to `0.0.0.0`, and dropped the query (RT-1);
//! - the setup URL without a certificate was `http://127.0.0.1/…`, which the
//!   HTTP listener never served, and ignored a non-443 port (RT-2);
//! - `url` was computed once at load, so it read `https://undefined` until a
//!   restart after the hostname was set (RT-3).
//!
//! Every URL here is computed from the current values, and the port is the
//! one the HTTPS listener actually has, which under systemd may differ from
//! the configured one.

use alloc::format;
use alloc::string::String;

use crate::config::Hostname;
use crate::origin::{self, Host};

/// The default port of `https`, left out of URLs.
pub const DEFAULT_HTTPS_PORT: u16 = 443;

/// `https://<host>[:<port>]`, with IPv6 hosts in brackets.
#[must_use]
pub fn https_origin(host: &Host, https_port: u16) -> String {
    if https_port == DEFAULT_HTTPS_PORT {
        format!("https://{host}")
    } else {
        format!("https://{host}:{https_port}")
    }
}

/// The node's own base URL, once it has a hostname.
#[must_use]
pub fn node_url(hostname: Option<&Hostname>, https_port: u16) -> Option<String> {
    let host = Host::Name(hostname?.as_str().into());
    Some(https_origin(&host, https_port))
}

/// Where the owner completes setup: `https://<hostname>[:port]/setup/<token>`.
///
/// Before a hostname is configured it uses `localhost`, which reaches the
/// node from its own machine, or through an SSH tunnel. The HTTPS listener
/// always has a certificate, self-signed at first, so the URL always works.
#[must_use]
pub fn setup_url(hostname: Option<&Hostname>, https_port: u16, token_hex: &str) -> String {
    let host = Host::Name(hostname.map_or("localhost", Hostname::as_str).into());
    format!("{}/setup/{token_hex}", https_origin(&host, https_port))
}

/// Where the HTTP listener redirects a request, or `None` if the request
/// names no usable host.
///
/// The configured hostname wins; before there is one, the request's `Host`
/// header (its host part, validated) is used. The path and query are kept.
#[must_use]
pub fn redirect_location(
    hostname: Option<&Hostname>,
    host_header: Option<&str>,
    https_port: u16,
    path_and_query: &str,
) -> Option<String> {
    let host = match hostname {
        Some(hostname) => Host::Name(hostname.as_str().into()),
        None => origin::parse_authority(host_header?)?.host,
    };
    let path = if path_and_query.starts_with('/') {
        path_and_query
    } else {
        "/"
    };
    Some(format!("{}{path}", https_origin(&host, https_port)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostname(text: &str) -> Hostname {
        Hostname::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    #[test]
    fn node_url_follows_the_current_hostname() {
        // RT-3: the original's url stayed "https://undefined".
        assert_eq!(node_url(None, 443), None);
        let name = hostname("node.example");
        assert_eq!(
            node_url(Some(&name), 443).as_deref(),
            Some("https://node.example")
        );
        assert_eq!(
            node_url(Some(&name), 8443).as_deref(),
            Some("https://node.example:8443")
        );
    }

    #[test]
    fn setup_url_is_https_and_keeps_the_port() {
        // RT-2: the original printed http://127.0.0.1/setup/<token> without
        // TLS, and dropped a non-443 port with it.
        assert_eq!(setup_url(None, 443, "ab"), "https://localhost/setup/ab");
        assert_eq!(
            setup_url(None, 8443, "ab"),
            "https://localhost:8443/setup/ab"
        );
        let name = hostname("node.example");
        assert_eq!(
            setup_url(Some(&name), 8443, "ab"),
            "https://node.example:8443/setup/ab"
        );
    }

    #[test]
    fn redirect_uses_the_hostname_and_keeps_the_query() {
        // RT-1: the original redirected to https://0.0.0.0/<path>, without the
        // query.
        let name = hostname("node.example");
        let location = redirect_location(Some(&name), Some("1.2.3.4"), 443, "/a?b=c");
        assert_eq!(location.as_deref(), Some("https://node.example/a?b=c"));
        let location = redirect_location(None, Some("1.2.3.4:80"), 8443, "/x?y");
        assert_eq!(location.as_deref(), Some("https://1.2.3.4:8443/x?y"));
        let location = redirect_location(None, Some("[::1]:80"), 443, "*");
        assert_eq!(location.as_deref(), Some("https://[::1]/"));
        assert_eq!(redirect_location(None, None, 443, "/"), None);
        assert_eq!(redirect_location(None, Some("bad host"), 443, "/"), None);
    }
}
