//! The node's outbound connections to servers that are not nodes
//! (spec/areas/peer-protocol.md#outbound-requests): Open Payments' and
//! other HTTPS requests (spec/decisions/open-payments.md), and WebSockets to
//! ledgers, to public addresses only, with bounded concurrency and response
//! sizes, and no redirects. [`Resolver`] is the guard on addresses that
//! these and the QUIC endpoint share; QUIC bounds its own requests in
//! flight, by node ([`crate::quic`]).
//!
//! The node owns one client, so its connection pool outlives restarts of
//! the engine.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use dassie_node_core::address::is_public;
use http::header::CONTENT_TYPE;
use http_body_util::{BodyExt, Full, Limited};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

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
    /// The fields a node's signature on its answer is in, each line as it
    /// came: `signature`, `signature-input` and `content-digest`
    /// (spec/areas/payments.md#signed-responses).
    pub signature_fields: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

/// The response fields [`WebResponse::signature_fields`] keeps.
const SIGNATURE_FIELDS: [http::HeaderName; 3] = [
    http::HeaderName::from_static("signature"),
    http::HeaderName::from_static("signature-input"),
    http::HeaderName::from_static("content-digest"),
];

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

/// Resolves names, keeping only addresses the node may send to: public
/// ones, unless private ones are allowed (dev and tests). Every outbound
/// connection of the node's resolves through one, QUIC's included, so the
/// same guard applies to all of them.
#[derive(Clone, Copy, Debug)]
pub struct Resolver {
    allow_private: bool,
}

type Addresses = std::vec::IntoIter<SocketAddr>;

impl Resolver {
    /// A resolver that allows private addresses only if `allow_private`.
    #[must_use]
    pub const fn new(allow_private: bool) -> Self {
        Self { allow_private }
    }

    /// Whether it allows private addresses (dev and tests).
    #[must_use]
    pub const fn allows_private(&self) -> bool {
        self.allow_private
    }

    /// The addresses of `host` the node may connect to, at `port`.
    ///
    /// # Errors
    ///
    /// The name does not resolve, or only to addresses refused.
    pub async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        let name = Name::from_str(host).map_err(io::Error::other)?;
        let found = tower_service::Service::call(&mut self.clone(), name).await?;
        Ok(found
            .map(|address| SocketAddr::new(address.ip(), port))
            .collect())
    }

    /// Whether a URL's authority names an IP literal it refuses: one that
    /// is not public, unless private addresses are allowed. Names are
    /// checked when resolved; literals never are.
    #[must_use]
    pub fn refuses_literal(&self, authority: &str) -> bool {
        !self.allow_private && literal_is_private(authority)
    }
}

impl tower_service::Service<Name> for Resolver {
    type Response = Addresses;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Addresses, io::Error>> + Send>>;

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
                return Err(io::Error::other("no public address"));
            }
            Ok(allowed.into_iter())
        })
    }
}

type Https = HyperClient<HttpsConnector<HttpConnector<Resolver>>, Full<Bytes>>;

/// The node's client for servers that are not nodes: HTTPS requests and
/// WebSockets.
#[derive(Clone)]
pub struct OutboundClient {
    http: Https,
    /// The TLS client config, for connections that are not HTTP requests:
    /// the ledgers' WebSockets.
    tls: Arc<rustls::ClientConfig>,
    resolver: Resolver,
    in_flight: Arc<Mutex<InFlight>>,
}

#[derive(Default)]
struct InFlight {
    total: usize,
    by_host: BTreeMap<String, usize>,
}

impl fmt::Debug for OutboundClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OutboundClient")
    }
}

/// A request's place in the bounds, given back when it ends.
struct Slot {
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

impl OutboundClient {
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
        let resolver = Resolver::new(allow_private);
        let tls = Arc::new(config.clone());
        let mut http = HttpConnector::new_with_resolver(resolver);
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
            resolver,
            in_flight: Arc::new(Mutex::new(InFlight::default())),
        })
    }

    /// The TLS config the client trusts servers by.
    #[must_use]
    pub fn tls(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(&self.tls)
    }

    /// The resolver it connects by.
    #[must_use]
    pub const fn resolver(&self) -> Resolver {
        self.resolver
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

    /// Sends a request to another server and reads its response, bounded
    /// in time and size.
    ///
    /// # Errors
    ///
    /// See [`WebError`].
    pub async fn web(&self, request: WebRequest) -> Result<WebResponse, WebError> {
        let authority = authority(&request.url).ok_or(WebError::Refused)?;
        if self.resolver.refuses_literal(authority) {
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
        let signature_fields = signature_fields(response.headers());
        let body = Limited::new(response.into_body(), max_bytes)
            .collect()
            .await
            .map_err(|_| WebError::TooLarge)?
            .to_bytes()
            .to_vec();
        Ok(WebResponse {
            status,
            media_type,
            signature_fields,
            body,
        })
    }

    /// A WebSocket to `url`, as the node connects to a ledger's server:
    /// `wss://`, or `ws://` where private addresses are allowed (dev and
    /// tests), by the guarded resolver, with messages and frames of at most
    /// `max_message_bytes`. The caller bounds the time it takes.
    ///
    /// # Errors
    ///
    /// The URL is not one the node may connect to, or the connection, TLS
    /// or the upgrade fails.
    pub async fn websocket(
        &self,
        url: &str,
        max_message_bytes: usize,
    ) -> Result<WebSocket, String> {
        let (secure, rest) = if let Some(rest) = url.strip_prefix("wss://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("ws://") {
            if !self.resolver.allows_private() {
                return Err("ws:// is for dev and tests only".into());
            }
            (false, rest)
        } else {
            return Err("not a WebSocket URL".into());
        };
        let (host, port) = authority_after(rest)
            .and_then(|authority| host_and_port(authority, if secure { 443 } else { 80 }))
            .ok_or("not a WebSocket URL")?;
        let addresses = self
            .resolver
            .resolve(host, port)
            .await
            .map_err(|e| e.to_string())?;
        let tcp = TcpStream::connect(addresses.as_slice())
            .await
            .map_err(|e| e.to_string())?;
        let stream = if secure {
            let name = ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
            let tls = tokio_rustls::TlsConnector::from(self.tls())
                .connect(name, tcp)
                .await
                .map_err(|e| e.to_string())?;
            Stream::Tls(Box::new(tls))
        } else {
            Stream::Plain(tcp)
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(max_message_bytes))
            .max_frame_size(Some(max_message_bytes));
        let (socket, _) = tokio_tungstenite::client_async_with_config(url, stream, Some(config))
            .await
            .map_err(|e| e.to_string())?;
        Ok(socket)
    }
}

/// A WebSocket to a ledger's server ([`OutboundClient::websocket`]).
pub type WebSocket = WebSocketStream<Stream>;

/// What a [`WebSocket`] runs over: TCP, or TLS over it.
#[derive(Debug)]
pub enum Stream {
    /// `ws://`, in dev and tests.
    Plain(TcpStream),
    /// `wss://`.
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

/// The lines of [`SIGNATURE_FIELDS`] that are text, at most
/// `httpsig::MAX_FIELD_LINES` of each, which is all a signature's check
/// reads.
fn signature_fields(headers: &http::HeaderMap) -> Vec<(String, String)> {
    SIGNATURE_FIELDS
        .iter()
        .flat_map(|name| {
            headers
                .get_all(name)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .take(dassie_payments::httpsig::MAX_FIELD_LINES)
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

/// The authority of an `https` URL, as the guard reads it: none if the URL
/// has none, or user info, which no URL the node connects to carries.
#[must_use]
pub fn authority(url: &str) -> Option<&str> {
    authority_after(url.strip_prefix("https://")?)
}

/// The authority of what follows a URL's scheme and `://`, as
/// [`authority`] reads it.
fn authority_after(rest: &str) -> Option<&str> {
    rest.split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty() && !authority.contains('@'))
}

/// The host and port of an authority, `host`, `host:port`, `[v6]` or
/// `[v6]:port`, with `default` for a port it names none of. An IPv6 literal
/// comes without its brackets. `None` for an empty host, a port that is
/// not a number, or port 0. Every URL the node connects to is read by it.
#[must_use]
pub fn host_and_port(authority: &str, default: u16) -> Option<(&str, u16)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        match after.strip_prefix(':') {
            Some(number) => (host, number.parse().ok()?),
            None if after.is_empty() => (host, default),
            None => return None,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, number)) => (host, number.parse().ok()?),
            None => (authority, default),
        }
    };
    (!host.is_empty() && port != 0).then_some((host, port))
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
pub(crate) fn literal_is_private(authority: &str) -> bool {
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

    #[test]
    fn hosts_and_ports_are_read_as_written() {
        assert_eq!(host_and_port("a.example", 443), Some(("a.example", 443)));
        assert_eq!(
            host_and_port("a.example:51233", 443),
            Some(("a.example", 51233))
        );
        assert_eq!(host_and_port("[::1]", 80), Some(("::1", 80)));
        assert_eq!(host_and_port("[::1]:6006", 80), Some(("::1", 6006)));
        for refused in ["[::1", "[::1]x", "a.example:x", "a.example:0", "", ":443"] {
            assert_eq!(host_and_port(refused, 80), None, "{refused}");
        }
    }

    #[test]
    fn a_url_has_an_authority_only_without_user_info() {
        assert_eq!(
            authority("https://a.example:8443/x?y"),
            Some("a.example:8443")
        );
        assert_eq!(authority("https://user@a.example/"), None);
        assert_eq!(authority("https:///x"), None);
        assert_eq!(authority("http://a.example/"), None);
    }

    #[tokio::test]
    async fn localhost_resolves_to_loopback_only_when_private_is_allowed() {
        use tower_service::Service as _;
        let name: Name = "d7.localhost".parse().unwrap();
        let mut allowed = Resolver {
            allow_private: true,
        };
        let found: Vec<SocketAddr> = allowed.call(name.clone()).await.unwrap().collect();
        assert_eq!(found, [SocketAddr::from((Ipv4Addr::LOCALHOST, 0))]);
        // Without it, loopback is refused like any private address.
        let mut guarded = Resolver {
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
        let client = OutboundClient::new(Some(Path::new(roots)), false).unwrap();
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
