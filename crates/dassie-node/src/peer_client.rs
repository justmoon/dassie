//! The node's outbound HTTPS client: peer requests
//! (spec/areas/peer-protocol.md#outbound-requests), `POST {url}/peer`, and
//! Open Payments' requests to other servers
//! (spec/decisions/open-payments.md), over HTTPS to public addresses only,
//! with bounded concurrency and response sizes, and no redirects.
//!
//! The node owns one client, so its connection pool outlives restarts of
//! the engine. Whether a failure means the request surely did not arrive
//! matters for packets: only then is a packet rejected at once.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use dassie_node_core::address::is_public;
use dassie_peer::wire::{MESSAGE_CONTENT_TYPE, RESPONSE_CONTENT_TYPE};
use dassie_peer::{Failure, Request, Response};
use http::header::{ACCEPT, CONTENT_TYPE};
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

/// Longest response body read: a node list of 4096 IDs fits.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Most requests in flight, in total and to one host.
const MAX_IN_FLIGHT: usize = 128;
const MAX_PER_HOST: usize = 16;

/// Most entries of a name's lookup read, per address family: whoever runs
/// the name chooses how many records it has.
const MAX_RECORDS_READ: usize = 64;

/// Most addresses of a name kept, per address family.
const MAX_ADDRESSES_PER_FAMILY: usize = 8;

/// How long the IPv6 lookup is still waited for once the IPv4 lookup has
/// given addresses the node may use. A resolver that never answers AAAA
/// queries would otherwise hold the lookup until its own timeout, which
/// can be longer than the time to connect. The IPv4 lookup is always
/// waited for in full.
const IPV6_GRACE: Duration = Duration::from_millis(250);

/// Most IPv6 lookups running at once. One no longer waited for runs on, on
/// tokio's blocking pool, until the system's resolver gives up; this keeps
/// a resolver that never answers AAAA from filling the pool, which the
/// node's file IO shares. IPv4 lookups are always waited for, so the
/// requests in flight bound them.
const MAX_IPV6_LOOKUPS: usize = 64;

/// The places of [`MAX_IPV6_LOOKUPS`].
static IPV6_LOOKUPS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_IPV6_LOOKUPS);

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
            let (four, six) = both_families(
                lookup(name.as_str(), Family::V4, None),
                lookup(name.as_str(), Family::V6, Some(&IPV6_LOOKUPS)),
                IPV6_GRACE,
                allow_private,
            )
            .await;
            let allowed = merge(four, six, allow_private)?;
            Ok(allowed
                .into_iter()
                .map(|address| SocketAddr::new(address, 0))
                .collect::<Vec<_>>()
                .into_iter())
        })
    }
}

/// An address family, looked up on its own.
#[derive(Clone, Copy, Debug)]
enum Family {
    V4,
    V6,
}

/// The addresses of `name` in one family, from the system's resolver.
///
/// Each family is asked for on its own, never both in one call
/// (spec/areas/peer-protocol.md#outbound-requests): musl's `getaddrinfo`
/// fails the whole name when either answer is an error, and some home
/// routers answer one family with NXDOMAIN while the other resolves. The
/// system's resolver, not a DNS client of our own, so that `/etc/hosts`
/// and nsswitch still apply (spec/decisions/resolving-by-family.md).
///
/// With `places`, the lookup runs only if it gets a place there, which it
/// holds until the system's resolver returns, waited for or not.
async fn lookup(
    name: &str,
    family: Family,
    places: Option<&'static tokio::sync::Semaphore>,
) -> io::Result<Vec<IpAddr>> {
    let place = match places {
        Some(places) => Some(
            places
                .try_acquire()
                .map_err(|_| io::Error::other("too many lookups running"))?,
        ),
        None => None,
    };
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || {
        let _place = place;
        let hints = dns_lookup::AddrInfoHints {
            address: match family {
                Family::V4 => dns_lookup::AddrFamily::Inet,
                Family::V6 => dns_lookup::AddrFamily::Inet6,
            }
            .into(),
            // One entry per address, not one per socket type.
            socktype: dns_lookup::SockType::Stream.into(),
            ..dns_lookup::AddrInfoHints::default()
        };
        let mut found = Vec::new();
        for entry in dns_lookup::getaddrinfo(Some(&name), None, Some(hints))?.take(MAX_RECORDS_READ)
        {
            let address = entry?.sockaddr.ip();
            if matches!(
                (family, address),
                (Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
            ) {
                found.push(address);
            }
        }
        Ok(found)
    })
    .await
    .map_err(io::Error::other)?
}

/// Runs both families' lookups at once, and gives both results. The IPv4
/// lookup is always waited for in full. The IPv6 lookup is too, unless
/// IPv4 gave addresses the node may use: then it has `grace` left to
/// answer, and counts as failed if it does not.
async fn both_families<F4, F6>(
    four: F4,
    six: F6,
    grace: Duration,
    allow_private: bool,
) -> (io::Result<Vec<IpAddr>>, io::Result<Vec<IpAddr>>)
where
    F4: Future<Output = io::Result<Vec<IpAddr>>>,
    F6: Future<Output = io::Result<Vec<IpAddr>>>,
{
    let (mut four, mut six) = (std::pin::pin!(four), std::pin::pin!(six));
    let (four, six_found) = tokio::select! {
        found = &mut four => (found, None),
        found = &mut six => (four.await, Some(found)),
    };
    let six = match six_found {
        Some(found) => found,
        None if four
            .as_ref()
            .is_ok_and(|found| found.iter().any(|address| usable(*address, allow_private))) =>
        {
            tokio::time::timeout(grace, six).await.unwrap_or_else(|_| {
                Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"))
            })
        }
        None => six.await,
    };
    (four, six)
}

/// Whether the node may connect to `address`.
fn usable(address: IpAddr, allow_private: bool) -> bool {
    allow_private || is_public(address)
}

/// The addresses the node may use, from both families' lookups: the
/// allowed ones of each, at most [`MAX_ADDRESSES_PER_FAMILY`] of each,
/// alternating and IPv4 first. IPv4 first because a home network that
/// has IPv4 always reaches out over it, and sorting the families against
/// each other by the routes the machine has (RFC 6724) is lost when they
/// are looked up apart.
///
/// # Errors
///
/// Both lookups failed, with the IPv4 lookup's error; or neither found an
/// address the node may use.
fn merge(
    four: io::Result<Vec<IpAddr>>,
    six: io::Result<Vec<IpAddr>>,
    allow_private: bool,
) -> io::Result<Vec<IpAddr>> {
    let (four, six) = match (four, six) {
        (Err(error), Err(_)) => return Err(error),
        (four, six) => (four.unwrap_or_default(), six.unwrap_or_default()),
    };
    let allowed = |found: Vec<IpAddr>| -> Vec<IpAddr> {
        found
            .into_iter()
            .filter(|address| usable(*address, allow_private))
            .take(MAX_ADDRESSES_PER_FAMILY)
            .collect()
    };
    let (four, six) = (allowed(four), allowed(six));
    let mut merged = Vec::with_capacity(four.len() + six.len());
    for index in 0..MAX_ADDRESSES_PER_FAMILY {
        merged.extend(four.get(index).into_iter().chain(six.get(index)));
    }
    if merged.is_empty() {
        return Err(io::Error::other("no address the node may use"));
    }
    Ok(merged)
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

    /// Sends `request` and reads its response.
    ///
    /// # Errors
    ///
    /// [`Failure::NotDelivered`] when the request surely did not reach the
    /// node: refused here, or the connection failed. [`Failure::Unknown`]
    /// when it may have: a timeout or a broken response.
    pub async fn send(&self, request: Request) -> Result<Response, Failure> {
        let _slot = self.hold(&request)?;
        self.send_held(request).await
    }

    /// Takes `request`'s place in the bounds on requests in flight, which
    /// every transport shares; `None` for a packet, which takes none.
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
        if request.kind == dassie_peer::wire::Kind::InterledgerPacket {
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

    /// Sends `request` over HTTPS, its place in the bounds held
    /// ([`hold`](Self::hold)).
    ///
    /// # Errors
    ///
    /// As for [`send`](Self::send).
    pub async fn send_held(&self, request: Request) -> Result<Response, Failure> {
        let url = format!("{}/peer", request.url);
        let host = request.url.strip_prefix("https://").unwrap_or_default();
        if !self.allow_private && literal_is_private(host) {
            return Err(Failure::NotDelivered);
        }
        let Ok(http_request) = http::Request::post(&url)
            .header(ACCEPT, MESSAGE_CONTENT_TYPE)
            .header(CONTENT_TYPE, MESSAGE_CONTENT_TYPE)
            .body(Full::new(Bytes::from(request.body)))
        else {
            return Err(Failure::NotDelivered);
        };
        let timeout = Duration::from_millis(request.timeout_ms.max(1));
        match tokio::time::timeout(timeout, self.exchange(http_request)).await {
            Ok(result) => result,
            Err(_) => Err(Failure::Unknown),
        }
    }

    async fn exchange(&self, request: http::Request<Full<Bytes>>) -> Result<Response, Failure> {
        let response = self.http.request(request).await.map_err(|error| {
            if error.is_connect() {
                Failure::NotDelivered
            } else {
                Failure::Unknown
            }
        })?;
        let status = response.status().as_u16();
        let content_type_ok = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes() == RESPONSE_CONTENT_TYPE.as_bytes());
        let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|_| Failure::Unknown)?
            .to_bytes()
            .to_vec();
        if status == 200 && !content_type_ok {
            // Not a peer's answer: treat it as an error, which no status
            // says arrived.
            return Ok(Response {
                status: 502,
                body: Vec::new(),
            });
        }
        Ok(Response { status, body })
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
            quic: true,
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

    fn nxdomain() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "Name does not resolve")
    }

    fn public_v4(last: u8) -> IpAddr {
        IpAddr::from([1, 1, 1, last])
    }

    fn public_v6(last: u16) -> IpAddr {
        IpAddr::from([0x2606, 0x4700, 0, 0, 0, 0, 0, last])
    }

    // Seen on a home router: the AAAA query answered NXDOMAIN while the A
    // query resolved, and musl's getaddrinfo gave up on the whole name.
    #[test]
    fn one_family_failing_leaves_the_other() {
        let four = vec![public_v4(1), public_v4(2)];
        assert_eq!(
            merge(Ok(four.clone()), Err(nxdomain()), false).unwrap(),
            four
        );
        let six = vec![public_v6(1)];
        assert_eq!(merge(Err(nxdomain()), Ok(six.clone()), false).unwrap(), six);
    }

    #[test]
    fn both_families_failing_fails_with_the_ipv4_error() {
        let four = io::Error::new(io::ErrorKind::TimedOut, "A timed out");
        let error = merge(Err(four), Err(nxdomain()), false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "A timed out");
    }

    #[test]
    fn no_address_in_either_family_fails() {
        assert!(merge(Ok(Vec::new()), Ok(Vec::new()), true).is_err());
        assert!(merge(Ok(Vec::new()), Err(nxdomain()), true).is_err());
    }

    #[test]
    fn private_addresses_are_refused_in_both_families() {
        let four = vec![IpAddr::from([10, 0, 0, 1]), public_v4(1)];
        let six = vec![IpAddr::from(std::net::Ipv6Addr::LOCALHOST), public_v6(1)];
        assert_eq!(
            merge(Ok(four.clone()), Ok(six.clone()), false).unwrap(),
            [public_v4(1), public_v6(1)]
        );
        assert_eq!(
            merge(Ok(four.clone()), Ok(six.clone()), true).unwrap(),
            [four[0], six[0], four[1], six[1]]
        );
        // Only refused addresses, and the other family failed: nothing.
        let only_private = vec![IpAddr::from(Ipv4Addr::LOCALHOST)];
        assert!(merge(Ok(only_private), Err(nxdomain()), false).is_err());
    }

    #[test]
    fn each_family_keeps_a_bounded_number_alternating_ipv4_first() {
        let many = u8::try_from(MAX_ADDRESSES_PER_FAMILY * 3).unwrap();
        let four: Vec<IpAddr> = (1..=many).map(public_v4).collect();
        let six: Vec<IpAddr> = (1..=u16::from(many)).map(public_v6).collect();
        let merged = merge(Ok(four.clone()), Ok(six.clone()), false).unwrap();
        assert_eq!(merged.len(), 2 * MAX_ADDRESSES_PER_FAMILY);
        for (index, pair) in merged.chunks(2).enumerate() {
            assert_eq!(pair, [four[index], six[index]]);
        }
        // Uneven: the longer family's rest follows, still bounded.
        let merged = merge(Ok(four[..1].to_vec()), Ok(six.clone()), false).unwrap();
        assert_eq!(merged.len(), 1 + MAX_ADDRESSES_PER_FAMILY);
        assert_eq!(merged[..3], [four[0], six[0], six[1]]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_ipv6_lookup_that_never_answers_is_waited_for_only_briefly() {
        let start = tokio::time::Instant::now();
        let (four, six) = both_families(
            async { Ok(vec![public_v4(1)]) },
            std::future::pending(),
            IPV6_GRACE,
            false,
        )
        .await;
        assert_eq!(start.elapsed(), IPV6_GRACE);
        assert_eq!(four.unwrap(), [public_v4(1)]);
        assert_eq!(six.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    // Found in review: AAAA cached, A not. Cutting the A lookup short left
    // only IPv6 addresses, which an IPv4-only home network cannot reach.
    #[tokio::test(start_paused = true)]
    async fn a_late_ipv4_lookup_is_waited_for_in_full() {
        let slow = 20 * IPV6_GRACE;
        let start = tokio::time::Instant::now();
        let (four, six) = both_families(
            async {
                tokio::time::sleep(slow).await;
                Ok(vec![public_v4(1)])
            },
            async { Ok(vec![public_v6(1)]) },
            IPV6_GRACE,
            false,
        )
        .await;
        assert_eq!(start.elapsed(), slow);
        assert_eq!(
            merge(four, six, false).unwrap(),
            [public_v4(1), public_v6(1)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_ipv4_lookup_waits_for_ipv6_in_full() {
        let slow = 20 * IPV6_GRACE;
        let start = tokio::time::Instant::now();
        let (four, six) = both_families(
            async { Err(nxdomain()) },
            async {
                tokio::time::sleep(slow).await;
                Ok(vec![public_v6(1)])
            },
            IPV6_GRACE,
            false,
        )
        .await;
        assert_eq!(start.elapsed(), slow);
        assert_eq!(merge(four, six, false).unwrap(), [public_v6(1)]);
    }

    // Found in review: IPv4 addresses the node may not use are no reason
    // to cut the IPv6 lookup short.
    #[tokio::test(start_paused = true)]
    async fn only_private_ipv4_addresses_wait_for_ipv6_in_full() {
        let slow = 20 * IPV6_GRACE;
        let private = IpAddr::from([10, 0, 0, 1]);
        let start = tokio::time::Instant::now();
        let (four, six) = both_families(
            async { Ok(vec![private]) },
            async {
                tokio::time::sleep(slow).await;
                Ok(vec![public_v6(1)])
            },
            IPV6_GRACE,
            false,
        )
        .await;
        assert_eq!(start.elapsed(), slow);
        assert_eq!(merge(four, six, false).unwrap(), [public_v6(1)]);
        // Where private addresses are allowed, they are usable ones.
        let start = tokio::time::Instant::now();
        let (four, six) = both_families(
            async { Ok(vec![private]) },
            std::future::pending(),
            IPV6_GRACE,
            true,
        )
        .await;
        assert_eq!(start.elapsed(), IPV6_GRACE);
        assert_eq!(merge(four, six, true).unwrap(), [private]);
    }

    // The FFI path, without the network: literals resolve in their own
    // family only, and a name the C side cannot take is an error.
    #[tokio::test]
    async fn the_system_resolver_is_asked_per_family() {
        let v4 = IpAddr::from(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::from(std::net::Ipv6Addr::LOCALHOST);
        assert_eq!(lookup("127.0.0.1", Family::V4, None).await.unwrap(), [v4]);
        assert!(lookup("127.0.0.1", Family::V6, None).await.is_err());
        assert_eq!(lookup("::1", Family::V6, None).await.unwrap(), [v6]);
        assert!(lookup("::1", Family::V4, None).await.is_err());
        assert!(lookup("bad\0name", Family::V4, None).await.is_err());
    }

    #[tokio::test]
    async fn a_lookup_runs_only_with_a_place() {
        static NONE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);
        static ONE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        assert!(lookup("::1", Family::V6, Some(&NONE)).await.is_err());
        let v6 = IpAddr::from(std::net::Ipv6Addr::LOCALHOST);
        assert_eq!(lookup("::1", Family::V6, Some(&ONE)).await.unwrap(), [v6]);
        // The place is given back once the lookup returned.
        assert_eq!(ONE.available_permits(), 1);
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
