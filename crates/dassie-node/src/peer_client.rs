//! The node's outbound client: the bounds on peer requests in flight, which
//! go over QUIC (spec/areas/peer-protocol.md#outbound-requests), the
//! resolver QUIC dials by, and Open Payments' and other requests to other
//! servers (spec/decisions/open-payments.md), over HTTPS to public
//! addresses only, with bounded concurrency and response sizes, and no
//! redirects.
//!
//! The node owns one client, so its connection pool outlives restarts of
//! the engine.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use dassie_node_core::address::is_public;
use dassie_peer::{Failure, Request};
use http::header::CONTENT_TYPE;
use http_body_util::{BodyExt, Full, Limited};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::tls;

/// Time to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Most requests in flight, in total and to one host.
const MAX_IN_FLIGHT: usize = 128;
const MAX_PER_HOST: usize = 16;

/// A request to another server, for Open Payments.
#[derive(Clone, Debug)]
pub struct WebRequest {
    /// The method.
    pub method: http::Method,
    /// An `https` URL.
    pub url: String,
    /// Extra headers.
    pub headers: Vec<(http::HeaderName, String)>,
    /// The body, if any.
    pub body: Option<Vec<u8>>,
    /// How long the whole exchange may take.
    pub timeout_ms: u64,
    /// The longest response body read.
    pub max_response_bytes: usize,
}

/// What another server answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebResponse {
    /// The status.
    pub status: u16,
    /// The media type, without parameters, in lower case.
    pub media_type: Option<String>,
    /// The body.
    pub body: Vec<u8>,
}

/// Why a request to another server failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebError {
    /// The URL is not `https`, or names a private address.
    Refused,
    /// Too many requests are in flight.
    Busy,
    /// The connection or the exchange failed, or took too long.
    Failed(String),
    /// The response was larger than allowed.
    TooLarge,
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused => f.write_str("the URL is not one the node may fetch"),
            Self::Busy => f.write_str("too many requests in flight"),
            Self::Failed(error) => write!(f, "the request failed: {error}"),
            Self::TooLarge => f.write_str("the response is too large"),
        }
    }
}

/// Resolves names, keeping only addresses the node may send to.
#[derive(Clone, Debug)]
struct GuardedResolver {
    allow_private: bool,
}

type Addresses = std::vec::IntoIter<SocketAddr>;

impl tower_service::Service<Name> for GuardedResolver {
    type Response = Addresses;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Addresses, std::io::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let allow_private = self.allow_private;
        Box::pin(async move {
            if allow_private && is_localhost(name.as_str()) {
                return Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 0))].into_iter());
            }
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let allowed: Vec<SocketAddr> = found
                .filter(|address| allow_private || is_public(address.ip()))
                .collect();
            if allowed.is_empty() {
                return Err(std::io::Error::other("no public address"));
            }
            Ok(allowed.into_iter())
        })
    }
}

type Https = HyperClient<HttpsConnector<HttpConnector<GuardedResolver>>, Full<Bytes>>;

/// The node's peer client.
#[derive(Clone)]
pub struct PeerClient {
    http: Https,
    /// The TLS client config, for connections that are not HTTP requests:
    /// the settlement schemes' WebSockets.
    tls: Arc<rustls::ClientConfig>,
    allow_private: bool,
    in_flight: Arc<Mutex<InFlight>>,
}

#[derive(Default)]
struct InFlight {
    total: usize,
    by_host: BTreeMap<String, usize>,
}

impl fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerClient")
    }
}

/// A request's place in the bounds, given back when it ends.
pub struct Slot {
    in_flight: Arc<Mutex<InFlight>>,
    host: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        in_flight.total = in_flight.total.saturating_sub(1);
        if let Some(count) = in_flight.by_host.get_mut(&self.host) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.by_host.remove(&self.host);
            }
        }
    }
}

impl PeerClient {
    /// A client trusting the platform's roots and, for dev and tests, the
    /// roots in `extra_roots`; refusing private addresses unless
    /// `allow_private`.
    ///
    /// # Errors
    ///
    /// The roots file cannot be read, or the platform's roots cannot be
    /// loaded.
    pub fn new(extra_roots: Option<&Path>, allow_private: bool) -> Result<Self, String> {
        let mut roots = Vec::new();
        if let Some(path) = extra_roots {
            for certificate in CertificateDer::pem_file_iter(path).map_err(|e| e.to_string())? {
                roots.push(certificate.map_err(|e| e.to_string())?);
            }
        }
        let verifier =
            rustls_platform_verifier::Verifier::new_with_extra_roots(roots, tls::provider())
                .map_err(|e| e.to_string())?;
        let config = rustls::ClientConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let tls = Arc::new(config.clone());
        let mut http = HttpConnector::new_with_resolver(GuardedResolver { allow_private });
        http.set_connect_timeout(Some(CONNECT_TIMEOUT));
        http.enforce_http(false);
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_only()
            .enable_http1()
            .wrap_connector(http);
        let http = HyperClient::builder(TokioExecutor::new())
            .pool_max_idle_per_host(4)
            .build(connector);
        Ok(Self {
            http,
            tls,
            allow_private,
            in_flight: Arc::new(Mutex::new(InFlight::default())),
        })
    }

    /// The TLS config the client trusts servers by.
    #[must_use]
    pub fn tls(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(&self.tls)
    }

    /// Whether the client may reach private addresses (dev and tests).
    #[must_use]
    pub const fn allows_private(&self) -> bool {
        self.allow_private
    }

    /// The addresses of `host` the node may connect to, as for its
    /// requests: public ones only, unless private ones are allowed.
    ///
    /// # Errors
    ///
    /// The name does not resolve, or only to addresses refused.
    pub async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        let mut resolver = GuardedResolver {
            allow_private: self.allow_private,
        };
        let name = Name::from_str(host).map_err(std::io::Error::other)?;
        let found = tower_service::Service::call(&mut resolver, name).await?;
        Ok(found
            .map(|address| SocketAddr::new(address.ip(), port))
            .collect())
    }

    /// Takes a place for a request to `host`, if the bounds allow.
    fn slot(&self, host: &str) -> Option<Slot> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let for_host = in_flight.by_host.get(host).copied().unwrap_or(0);
        if in_flight.total >= MAX_IN_FLIGHT || for_host >= MAX_PER_HOST {
            return None;
        }
        in_flight.total += 1;
        in_flight.by_host.insert(host.to_owned(), for_host + 1);
        Some(Slot {
            in_flight: Arc::clone(&self.in_flight),
            host: host.to_owned(),
        })
    }

    /// Takes `request`'s place in the bounds on requests in flight, which
    /// requests to other servers share; `None` for a packet, which takes
    /// none.
    ///
    /// # Errors
    ///
    /// [`Failure::NotDelivered`] when the URL names a private address, or
    /// the bounds are full.
    pub fn hold(&self, request: &Request) -> Result<Option<Slot>, Failure> {
        let host = request.url.strip_prefix("https://").unwrap_or_default();
        // A periphery node has no URL, and so no address to check: it is
        // reached only over the connection it opened to us.
        if !request.url.is_empty() && !self.allow_private && literal_is_private(host) {
            return Err(Failure::NotDelivered);
        }
        // ILP packets are bounded by the connector's tables, and their
        // responses must get through: only the protocol's own requests
        // count against the limits here.
        if request.kind.carries_packet() {
            return Ok(None);
        }
        // A periphery node has no URL: each counts by its ID, not all
        // together under the empty host.
        let slot = if request.url.is_empty() {
            self.slot(&format!("node:{}", request.to))
        } else {
            self.slot(host)
        };
        slot.map(Some).ok_or(Failure::NotDelivered)
    }
}

impl PeerClient {
    /// Sends a request to another server and reads its response, bounded
    /// in time and size.
    ///
    /// # Errors
    ///
    /// See [`WebError`].
    pub async fn web(&self, request: WebRequest) -> Result<WebResponse, WebError> {
        let authority = authority(&request.url).ok_or(WebError::Refused)?;
        if !self.allow_private && literal_is_private(authority) {
            return Err(WebError::Refused);
        }
        let _slot = self.slot(authority).ok_or(WebError::Busy)?;
        let mut builder = http::Request::builder()
            .method(request.method)
            .uri(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let body = Full::new(Bytes::from(request.body.unwrap_or_default()));
        let http_request = builder
            .body(body)
            .map_err(|error| WebError::Failed(error.to_string()))?;
        let timeout = Duration::from_millis(request.timeout_ms.max(1));
        let exchange = self.web_exchange(http_request, request.max_response_bytes);
        tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| WebError::Failed("timed out".into()))?
    }

    async fn web_exchange(
        &self,
        request: http::Request<Full<Bytes>>,
        max_bytes: usize,
    ) -> Result<WebResponse, WebError> {
        let response = self
            .http
            .request(request)
            .await
            .map_err(|error| WebError::Failed(error.to_string()))?;
        let status = response.status().as_u16();
        let media_type = media_type(response.headers());
        let body = Limited::new(response.into_body(), max_bytes)
            .collect()
            .await
            .map_err(|_| WebError::TooLarge)?
            .to_bytes()
            .to_vec();
        Ok(WebResponse {
            status,
            media_type,
            body,
        })
    }
}

/// The authority of an `https` URL, as the guard reads it: none if the URL
/// has none, or user info, which no request of the node's carries.
fn authority(url: &str) -> Option<&str> {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|authority| !authority.is_empty() && !authority.contains('@'))
}

/// A response's media type, without its parameters, lowercased.
fn media_type(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|media| media.trim().to_ascii_lowercase())
}

/// Whether `name` is `localhost` or a name under it, which always means
/// this machine (RFC 6761 §6.3): the dev environment's `dN.localhost`,
/// whatever the system's resolver makes of them.
fn is_localhost(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    name.eq_ignore_ascii_case("localhost")
        || name
            .len()
            .checked_sub(".localhost".len())
            .and_then(|start| name.get(start..))
            .is_some_and(|tail| tail.eq_ignore_ascii_case(".localhost"))
}

/// Whether a URL's authority names an IP literal that is not public. Names
/// are checked when resolved; literals never are. The authority is read as
/// the HTTP client reads it, which ends an IPv6 literal at its bracket,
/// whatever follows: `[::1]a` is `::1`. One the client cannot read counts
/// as private, so the guard never passes what it reads differently.
fn literal_is_private(authority: &str) -> bool {
    let Ok(authority) = authority.parse::<http::uri::Authority>() else {
        return true;
    };
    // As hyper-util's connector strips the brackets.
    let literal = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    literal
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| !is_public(address))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn private_literals_are_refused_before_sending() {
        assert!(literal_is_private("127.0.0.1:8443"));
        assert!(literal_is_private("[::1]:8443"));
        assert!(literal_is_private("10.0.0.1"));
        assert!(!literal_is_private("1.1.1.1:443"));
        assert!(!literal_is_private("example.com:443"));
        assert!(!literal_is_private("localhost"));
    }

    // Found in review: periphery nodes have no URL, so every downlink's
    // requests shared the bound of one host, the empty one. Each counts by
    // its ID. And the empty host read as a private literal, so a node that
    // allows no private addresses refused them all.
    #[tokio::test]
    async fn each_node_without_a_url_has_its_own_bound() {
        // Roots of its own: the build sandbox has none of the system's.
        let roots = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/pebble/root.pem"
        );
        let client = PeerClient::new(Some(Path::new(roots)), false).unwrap();
        let request = |seed: u8| dassie_peer::Request {
            id: dassie_peer::RequestId(0),
            to: dassie_identity::NodeKey::from_bytes(&[seed; 32]).node_id(),
            url: String::new(),
            kind: dassie_peer::wire::Kind::LinkStateUpdate,
            body: Vec::new(),
            timeout_ms: 1_000,
        };
        let mut held = Vec::new();
        for seed in 1..=u8::try_from(MAX_PER_HOST).unwrap() + 1 {
            held.push(client.hold(&request(seed)).unwrap());
        }
        for _ in 1..MAX_PER_HOST {
            held.push(client.hold(&request(1)).unwrap());
        }
        assert_eq!(client.hold(&request(1)).err(), Some(Failure::NotDelivered));
    }

    #[test]
    fn localhost_names_are_this_machine() {
        assert!(is_localhost("localhost"));
        assert!(is_localhost("d1.localhost"));
        assert!(is_localhost("D12.LocalHost."));
        assert!(!is_localhost("notlocalhost"));
        assert!(!is_localhost("localhost.example.com"));
        assert!(!is_localhost(".localhostx"));
        assert!(!is_localhost(""));
    }

    #[tokio::test]
    async fn localhost_resolves_to_loopback_only_when_private_is_allowed() {
        use tower_service::Service as _;
        let name: Name = "d7.localhost".parse().unwrap();
        let mut allowed = GuardedResolver {
            allow_private: true,
        };
        let found: Vec<SocketAddr> = allowed.call(name.clone()).await.unwrap().collect();
        assert_eq!(found, [SocketAddr::from((Ipv4Addr::LOCALHOST, 0))]);
        // Without it, loopback is refused like any private address.
        let mut guarded = GuardedResolver {
            allow_private: false,
        };
        assert!(guarded.call(name).await.is_err());
    }

    // The HTTP client reads `[::1]a` as the IPv6 literal `::1`, and
    // connects to it without asking the resolver. The guard read it as no
    // literal at all, so a URL from another wallet reached loopback and
    // private addresses.
    #[test]
    fn an_ipv6_literal_ends_at_its_bracket() {
        for authority in [
            "[::1]a",
            "[::1]a:8443",
            "[::1]:8443x",
            "[::ffff:10.0.0.1]x",
            "[fe80::1]]",
        ] {
            assert!(literal_is_private(authority), "{authority}");
        }
        assert!(!literal_is_private("[2606:4700::1111]:443"));
    }

    // So such a URL is refused before connecting, as `[::1]` is.
    #[tokio::test]
    async fn a_url_past_an_ipv6_literal_is_refused() {
        // Roots of its own: the build sandbox has none of the system's.
        let roots = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/pebble/root.pem"
        );
        let client = PeerClient::new(Some(Path::new(roots)), false).unwrap();
        let result = client
            .web(WebRequest {
                method: http::Method::GET,
                url: "https://[::1]a:8443/x".into(),
                headers: Vec::new(),
                body: None,
                timeout_ms: 2_000,
                max_response_bytes: 1_000,
            })
            .await;
        assert!(matches!(result, Err(WebError::Refused)), "{result:?}");
    }

    mod properties {
        use std::net::IpAddr;

        use proptest::prelude::*;

        use super::*;

        /// A response's `Content-Type`s: none, several, or any bytes a
        /// header may hold.
        fn content_types() -> impl Strategy<Value = http::HeaderMap> {
            let known = proptest::sample::select(vec![
                "application/json",
                "Application/JSON; charset=utf-8",
                " application/json ;",
                "text/html",
                "",
                ";",
                "a;b;c",
            ]);
            let byte = prop_oneof![b' '..=b'~', 0x80_u8..=0xff, Just(b'\t')];
            let value = prop_oneof![
                known.prop_map(|value| value.as_bytes().to_vec()),
                proptest::collection::vec(byte, 0..40),
            ];
            proptest::collection::vec(value, 0..3).prop_map(|values| {
                let mut headers = http::HeaderMap::new();
                for value in values {
                    if let Ok(value) = http::HeaderValue::from_bytes(&value) {
                        headers.append(CONTENT_TYPE, value);
                    }
                }
                headers
            })
        }

        /// An IP literal as a URL writes it: IPv6 in brackets.
        fn literal(address: IpAddr) -> String {
            match address {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => format!("[{v6}]"),
            }
        }

        /// Hosts: IP literals, private ones often, names under
        /// `localhost`, and any text.
        fn host() -> impl Strategy<Value = String> {
            let private = proptest::sample::select(vec![
                "127.0.0.1",
                "10.1.2.3",
                "169.254.169.254",
                "[::1]",
                "[::ffff:127.0.0.1]",
                "[fe80::1]",
                "[fd00::1]",
                "0.0.0.0",
            ]);
            prop_oneof![
                any::<IpAddr>().prop_map(literal),
                private.prop_map(String::from),
                proptest::sample::select(vec![
                    "localhost",
                    "D1.LocalHost.",
                    "localhost.example",
                    "a.localhostx"
                ])
                .prop_map(String::from),
                "[ -~]{0,24}",
                any::<String>(),
            ]
        }

        /// A URL a wallet or a peer named: any scheme, host, port, user
        /// info and path.
        fn url() -> impl Strategy<Value = String> {
            let scheme = proptest::sample::select(vec!["https://", "HTTPS://", "http://", ""]);
            let user = proptest::sample::select(vec!["", "user@", "1.1.1.1@", ":@"]);
            let port = prop_oneof![
                Just(String::new()),
                any::<u16>().prop_map(|port| format!(":{port}")),
                "[:0-9a-z]{0,8}",
            ];
            let rest = prop_oneof![
                proptest::sample::select(vec![
                    "",
                    "/",
                    "/x?y#z",
                    "?@1.1.1.1",
                    "#@1.1.1.1",
                    "\\@1.1.1.1/"
                ])
                .prop_map(String::from),
                "\\PC{0,24}",
            ];
            (scheme, user, host(), port, rest).prop_map(|(scheme, user, host, port, rest)| {
                format!("{scheme}{user}{host}{port}{rest}")
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

            // URLs come from other wallets and peers. The guard reads any of
            // them without a panic, and refuses every private literal the
            // HTTP client would connect to without resolving a name, where
            // the resolver's filter never sees it
            // (spec/areas/peer-protocol.md#outbound-requests).
            #[test]
            fn any_url_is_read_or_refused(url in url()) {
                let authority = authority(&url);
                if let Some(authority) = authority {
                    prop_assert!(url.starts_with("https://"));
                    prop_assert!(!authority.is_empty());
                    prop_assert!(!authority.contains(['@', '/', '?', '#']));
                }
                let _ = authority.map(literal_is_private);
                let Ok(uri) = url.parse::<http::Uri>() else { return Ok(()) };
                let Some(host) = uri.host() else { return Ok(()) };
                // As hyper-util's connector reads it.
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                if uri.scheme_str() == Some("https")
                    && bare.parse::<IpAddr>().is_ok_and(|address| !is_public(address))
                {
                    prop_assert!(authority.is_none_or(literal_is_private), "{} reaches {}", url, host);
                }
            }

            #[test]
            fn private_literals_are_refused(address in any::<IpAddr>(), port in proptest::option::of(any::<u16>())) {
                let host = match port {
                    Some(port) => format!("{}:{port}", literal(address)),
                    None => literal(address),
                };
                prop_assert_eq!(literal_is_private(&host), !is_public(address));
            }

            #[test]
            fn any_name_is_localhost_or_not(name in host()) {
                if is_localhost(&name) {
                    let name = name.to_ascii_lowercase();
                    prop_assert!(name.trim_end_matches('.').ends_with("localhost"));
                }
            }

            #[test]
            fn any_media_type_is_read(headers in content_types()) {
                if let Some(media) = media_type(&headers) {
                    prop_assert_eq!(media.trim(), media.as_str());
                    prop_assert!(!media.contains(';'));
                    prop_assert_eq!(media.to_ascii_lowercase(), media.as_str());
                }
            }
        }
    }
}
