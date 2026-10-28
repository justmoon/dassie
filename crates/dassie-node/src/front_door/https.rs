//! The front door's HTTPS listener (spec/areas/http.md#front-doors): it
//! takes Open Payments requests for `<node ID>.<domain>` and `<name>.<domain>`,
//! passes them to the node the ID names, or to the name's holder in the
//! front door's view of the registrars' lists ([`names`](super::names)), and
//! passes the node's answer back as it came, so the node's signature on it
//! still verifies.
//!
//! It passes on only Open Payments' routes, with bounded heads and bodies,
//! and only end-to-end headers. It returns only JSON, or no body, but for
//! one static page it writes itself at the root of a node's name
//! ([`page`]). It never serves a node's page, the wallet or a cookie.
//!
//! On the host nodes dial it at as their relay, it also takes tunnels for
//! QUIC at `/quic`, as a node's HTTPS listener does
//! (spec/areas/peer-protocol.md#quic-over-websocket), and hands them to its
//! QUIC endpoint: so nodes whose UDP does not reach it keep it as their
//! relay all the same.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::NodeId;
use dassie_kit::ClientKey;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::origin::{Authority, Host, parse_authority};
use dassie_node_core::rate_limit::{RateLimitPolicy, RateLimiter};
use dassie_node_core::urls::DEFAULT_HTTPS_PORT;
use dassie_peer::names::Name;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::names::Names;
use super::nodes::{Answer, Missed, Nodes};
use super::page::{self, Reached};
use crate::clock::Clock;
use crate::http::open_payments::{MAX_BODY_BYTES, is_open_payments_path};
use crate::http::response::{self, Body, header_has_token, header_str};
use crate::http::{
    DRAIN_TIMEOUT, HEADER_TIMEOUT, MAX_CONNECTIONS, MAX_HEAD_BYTES, TLS_HANDSHAKE_TIMEOUT,
    TunnelRefused, upgrade_tunnel,
};
use crate::log::target;
use crate::node::{Policies, lock};
use crate::open_payments::REQUESTS;
use crate::quic::{Quic, tunnel};
use crate::slots::{ClientSlot, ClientSlots};

/// Most connections from one client (an IPv4 address or an IPv6 /56), as
/// a node's listeners allow.
const CONNECTIONS_PER_CLIENT: u32 = Policies::DEFAULT.http_connections_per_client;

/// Time to send a request's body, as a node allows.
const BODY_TIMEOUT: Duration = Duration::from_millis(Policies::DEFAULT.body_timeout_ms);

/// Where interactive grants send the owner, which needs an owner app
/// (spec/designs/periphery.md#6-owner-apps): the front door answers it
/// itself.
const INTERACT_PREFIX: &str = "/gnap/interact/";

/// The Content Security Policy on every answer the front door gives or
/// passes on, replacing a node's: nothing runs, nothing loads, so even a
/// body a browser misreads as a page can do nothing on the front door's
/// names (spec/areas/http.md#front-doors).
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; sandbox";

/// A client's requests to one node, per client address: a quarter of what
/// the node lets one address send (`REQUESTS`), since the node counts the
/// front door as a single client of every request it passes on. Four
/// clients at their allowance still use up a node's whole budget for the
/// front door (spec/roadmap.md#periphery).
const REQUESTS_PER_NODE: RateLimitPolicy = RateLimitPolicy {
    client_burst: REQUESTS.client_burst / 4,
    client_refill_ms: REQUESTS.client_refill_ms * 4,
    client_slots: REQUESTS.client_slots,
};

/// Headers that concern one connection, never passed on either way
/// (RFC 9110 §7.6.1), beside those `Connection` names.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The front door's side of a request: whose name it is, and how to
/// reach the node.
pub struct Door {
    /// The domain, lowercase, as names compare.
    domain: String,
    /// Where nodes dial it as their relay, and the port: the host whose
    /// `/quic` takes tunnels.
    relay: (Host, u16),
    /// Its QUIC endpoint, which takes the tunnels.
    quic: Arc<Quic>,
    nodes: Arc<Nodes>,
    /// Who holds each name.
    names: Arc<Names>,
    requests: Mutex<RateLimiter>,
    /// Per client address and node: [`REQUESTS_PER_NODE`].
    requests_per_node: Mutex<RateLimiter>,
    clock: Arc<dyn Clock>,
}

/// Where a request goes.
enum Target {
    /// To the node with this ID.
    Node(NodeId),
    /// The consent route, which the front door answers.
    Interact,
    /// The page at the root of the node's name, which the front door
    /// writes itself ([`page`]).
    Page(Addressed),
}

/// The node a name under the domain stands for, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Addressed {
    node: NodeId,
    /// The name, if the request came under one rather than the node's ID.
    name: Option<Name>,
}

impl Door {
    /// The front door for nodes under `domain`, reached through `nodes`,
    /// and for the names whose holders `names` has; and for the tunnels of
    /// the nodes that dial it at `relay` ([`relay_authority`]), which `quic`
    /// takes.
    #[must_use]
    pub fn new(
        domain: &str,
        (relay, quic): ((Host, u16), Arc<Quic>),
        (nodes, names): (Arc<Nodes>, Arc<Names>),
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            domain: domain.to_ascii_lowercase(),
            relay,
            quic,
            nodes,
            names,
            requests: Mutex::new(RateLimiter::new(REQUESTS)),
            requests_per_node: Mutex::new(RateLimiter::new(REQUESTS_PER_NODE)),
            clock,
        }
    }

    /// Answers one request from `peer`; a tunnel it takes goes on a task
    /// of `tasks` until upgraded.
    async fn route(
        &self,
        peer: SocketAddr,
        request: Request<Incoming>,
        tasks: &TaskTracker,
    ) -> Response<Body> {
        let client = ClientKey::from_ip(peer.ip().to_canonical());
        let now_ms = self.clock.now_ms();
        if lock(&self.requests).check(client, now_ms).is_err() {
            return too_many();
        }
        let host = header_str(request.headers(), &header::HOST);
        if request.uri().path() == tunnel::PATH && self.is_relay(host) {
            return self.tunnel(peer, request, tasks);
        }
        match self.target(&request) {
            Err(refused) => refused.response(),
            Ok(Target::Page(Addressed { node, name })) => {
                let reached = match &name {
                    Some(name) => Reached::Name(name),
                    None => Reached::Id(&node),
                };
                let connected = self.nodes.is_connected(node).await;
                page::response(&self.domain, reached, &node, connected)
            }
            Ok(Target::Interact) => error(
                StatusCode::NOT_IMPLEMENTED,
                "interaction_unavailable",
                "interactive grants need the owner's Dassie app, which is not available yet",
            ),
            Ok(Target::Node(node)) => {
                let key = per_node(client, &node);
                if lock(&self.requests_per_node).check(key, now_ms).is_err() {
                    return too_many();
                }
                self.pass(node, request).await
            }
        }
    }

    /// Whether `host` is where nodes dial the front door as their relay, on
    /// the same port: the one host whose `/quic` takes tunnels. Every other
    /// host answers `/quic` as any path outside Open Payments.
    fn is_relay(&self, host: Option<&str>) -> bool {
        host.and_then(parse_authority).is_some_and(|authority| {
            let port = authority.port.unwrap_or(DEFAULT_HTTPS_PORT);
            authority.host == self.relay.0 && port == self.relay.1
        })
    }

    /// Takes a tunnel for QUIC from `peer`, as a node's HTTPS listener
    /// does: once upgraded, its connection leaves this listener's places,
    /// and the tunnel holds the place of the QUIC connection it carries,
    /// by its address as over UDP: first one among those being set up,
    /// then, once the connection proved its key, a node's
    /// ([`relay`](super::relay)).
    fn tunnel(
        &self,
        peer: SocketAddr,
        request: Request<Incoming>,
        tasks: &TaskTracker,
    ) -> Response<Body> {
        if !header_has_token(request.headers(), &header::CONNECTION, "upgrade") {
            return error(
                StatusCode::UPGRADE_REQUIRED,
                "invalid_request",
                "a tunnel is a WebSocket",
            );
        }
        let refused = match upgrade_tunnel(Some(&self.quic), peer, request, tasks) {
            Ok(switching) => return switching,
            Err(refused) => refused,
        };
        tracing::debug!(target: target::FRONT_DOOR, %peer, ?refused, "tunnel refused");
        let (status, code, description) = match refused {
            TunnelRefused::NotAHandshake => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "expected a WebSocket handshake",
            ),
            TunnelRefused::FromABrowser => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "tunnels are not for browsers",
            ),
            TunnelRefused::NoSubprotocol => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "expected the dassie-quic/1 subprotocol",
            ),
            TunnelRefused::NotServing => (
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "the relay is not running",
            ),
            TunnelRefused::TooMany => (
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "too many tunnels",
            ),
        };
        error(status, code, description)
    }

    /// Where `request` goes, or why it goes nowhere: a name outside the
    /// domain is refused, and a name that is neither a node's ID nor a
    /// name with a holder, a path outside Open Payments or an upgrade find
    /// nothing.
    fn target(&self, request: &Request<Incoming>) -> Result<Target, Refused> {
        let host = header_str(request.headers(), &header::HOST);
        let addressed = self.node_of(host)?;
        let node = addressed.node;
        let path = request.uri().path();
        if header_has_token(request.headers(), &header::CONNECTION, "upgrade") {
            return Err(Refused::NoRoute);
        }
        if path == page::PATH && matches!(*request.method(), Method::GET | Method::HEAD) {
            return Ok(Target::Page(addressed));
        }
        if !is_open_payments_path(path) {
            return Err(Refused::NoRoute);
        }
        if path.starts_with(INTERACT_PREFIX) {
            return Ok(Target::Interact);
        }
        Ok(Target::Node(node))
    }

    /// The node `<node ID>.<domain>` names, on the default port, or the
    /// holder of `<name>.<domain>`. A name is shorter than a node ID
    /// (`dassie_peer::names::MAX_NAME_BYTES`), so no host is both.
    fn node_of(&self, host: Option<&str>) -> Result<Addressed, Refused> {
        let authority = host.and_then(parse_authority).ok_or(Refused::Misdirected)?;
        let Host::Name(name) = authority.host else {
            return Err(Refused::Misdirected);
        };
        if authority
            .port
            .is_some_and(|port| port != DEFAULT_HTTPS_PORT)
        {
            return Err(Refused::Misdirected);
        }
        let label = name
            .strip_suffix(&self.domain)
            .and_then(|rest| rest.strip_suffix('.'));
        let Some(label) = label else {
            return Err(if name == self.domain {
                Refused::NoNode
            } else {
                Refused::Misdirected
            });
        };
        if label.contains('.') {
            return Err(Refused::NoNode);
        }
        if let Ok(node) = NodeId::parse(label) {
            return Ok(Addressed { node, name: None });
        }
        // A name no node holds finds nothing, as an ID that is no node's.
        let name = Name::parse(label).map_err(|_| Refused::NoNode)?;
        let node = self.names.holder(&name).ok_or(Refused::NoNode)?;
        Ok(Addressed {
            node,
            name: Some(name),
        })
    }

    /// Passes `request` to `node` and its answer back.
    async fn pass(&self, node: NodeId, request: Request<Incoming>) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let read = tokio::time::timeout(BODY_TIMEOUT, Limited::new(body, MAX_BODY_BYTES).collect());
        let body = match read.await {
            Ok(Ok(collected)) => collected.to_bytes(),
            Ok(Err(_)) => {
                return error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "invalid_request",
                    "the body is too large",
                );
            }
            Err(_) => {
                return error(
                    StatusCode::REQUEST_TIMEOUT,
                    "invalid_request",
                    "the body took too long",
                );
            }
        };
        let path_and_query = parts.uri.path_and_query().map_or("/", |pq| pq.as_str());
        let mut forwarded = Request::new(Full::new(body));
        *forwarded.method_mut() = parts.method.clone();
        *forwarded.version_mut() = Version::HTTP_11;
        match path_and_query.parse() {
            Ok(uri) => *forwarded.uri_mut() = uri,
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_request", "Bad Request"),
        }
        *forwarded.headers_mut() = passed_on(&parts.headers);
        match self.nodes.send(node, forwarded).await {
            Ok(answer) => returned(answer),
            Err(missed) => {
                tracing::debug!(target: target::FRONT_DOOR, %node, ?missed, "no answer from the node");
                unanswered(missed)
            }
        }
    }
}

/// Where nodes dial a front door whose relay URL is `url`,
/// `https://host[:port]`: its host, and its port, 443 when it names none.
/// Their tunnels name it as their `Host`.
#[must_use]
pub fn relay_authority(url: &str) -> Option<(Host, u16)> {
    let Authority { host, port } = parse_authority(url.strip_prefix("https://")?)?;
    Some((host, port.unwrap_or(DEFAULT_HTTPS_PORT)))
}

/// The key a client's requests to `node` count under: the client's and
/// the node's, hashed into one, as the rate limiter keys clients. The hash
/// goes in as an IPv6 address, of which `ClientKey::from_ip` keeps the /56:
/// the key is 56 bits of the hash, plenty to keep a table of at most
/// `client_slots` keys apart.
fn per_node(client: ClientKey, node: &NodeId) -> ClientKey {
    let mut hash = Sha256::new();
    hash.update(client.to_bytes());
    hash.update(node.as_str().as_bytes());
    let digest: [u8; 32] = hash.finalize().into();
    let mut key = [0u8; 16];
    key.copy_from_slice(digest.get(..16).unwrap_or(&[0; 16]));
    ClientKey::from_ip(std::net::Ipv6Addr::from(key).into())
}

fn too_many() -> Response<Body> {
    error(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_requests",
        "slow down",
    )
}

/// The answer as the node gave it, with only the headers a client needs
/// ([`passed_back`]); or 502, unless it is JSON with a single
/// `Content-Type`, or has no body. The front door's own `nosniff` and
/// Content Security Policy replace the node's. The node's signature covers
/// neither (spec/areas/payments.md#signed-responses), so it still verifies.
fn returned((parts, body): Answer) -> Response<Body> {
    let bad = |description: &str| error(StatusCode::BAD_GATEWAY, "bad_gateway", description);
    let types: Vec<&HeaderValue> = parts.headers.get_all(header::CONTENT_TYPE).iter().collect();
    let is_json = match types.as_slice() {
        [] => false,
        [one] => one
            .to_str()
            .ok()
            .and_then(|value| value.split(';').next())
            .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json")),
        _ => return bad("the node named more than one media type"),
    };
    if !body.is_empty() && !is_json {
        return bad("the node did not answer with JSON");
    }
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = parts.status;
    let mut headers = passed_back(&parts.headers);
    if is_json {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    harden(&mut headers);
    *response.headers_mut() = headers;
    response
}

/// Sets the front door's own `nosniff` and Content Security Policy,
/// replacing any a node sent.
fn harden(headers: &mut HeaderMap) {
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
}

/// Why a node gave no answer, in the specs' error form.
fn unanswered(missed: Missed) -> Response<Body> {
    match missed {
        Missed::NotConnected => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "the node is not connected to this front door",
        ),
        Missed::Busy => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "the node is busy",
        ),
        Missed::Broken => error(
            StatusCode::BAD_GATEWAY,
            "bad_gateway",
            "the node's answer broke",
        ),
        Missed::TimedOut => error(
            StatusCode::GATEWAY_TIMEOUT,
            "gateway_timeout",
            "the node did not answer in time",
        ),
    }
}

/// The headers of a request passed on to a node: the end-to-end ones,
/// without hop-by-hop headers and those `Connection` names, `Expect`,
/// `Content-Length` (the body is read, and its length sent anew) and what
/// proxies add (`Forwarded`, `X-Forwarded-*`).
fn passed_on(headers: &HeaderMap) -> HeaderMap {
    let named: Vec<String> = response::header_values(headers, &header::CONNECTION)
        .flat_map(|value| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let dropped = |name: &HeaderName| {
        let name = name.as_str();
        matches!(name, "expect" | "content-length" | "forwarded")
            || name.starts_with("x-forwarded-")
            || HOP_BY_HOP.contains(&name)
            || named.iter().any(|token| token == name)
    };
    let mut kept = HeaderMap::new();
    for (name, value) in headers {
        if !dropped(name) {
            kept.append(name.clone(), value.clone());
        }
    }
    kept
}

/// The headers of a node's answer that an Open Payments client needs, and
/// nothing else: by an allowlist, so that no header a browser acts on
/// (`Content-Disposition`, cookies, `Refresh`, `Alt-Svc`, `Link`, the
/// node's policies) reaches the front door's names, whatever a node
/// sends. These are what the node's Open Payments answers set
/// (`http/open_payments/`), and `Retry-After` and `Vary`. `Location` is
/// not among them, since only the consent route, which the front door
/// answers itself, redirects; nor is `Content-Length`, which hyper writes
/// for the body the front door holds. `WWW-Authenticate` passes only as
/// the GNAP challenge ([`gnap_only`]).
const ANSWER_HEADERS: &[&str] = &[
    "content-type",
    "content-digest",
    "signature",
    "signature-input",
    "www-authenticate",
    "retry-after",
    "cache-control",
    "vary",
];

/// The headers of a node's answer passed back: [`ANSWER_HEADERS`] and the
/// CORS headers (`Access-Control-*`), with `WWW-Authenticate` only if it
/// is the GNAP challenge alone.
fn passed_back(headers: &HeaderMap) -> HeaderMap {
    let mut kept = HeaderMap::new();
    for (name, value) in headers {
        let name_text = name.as_str();
        if ANSWER_HEADERS.contains(&name_text) || name_text.starts_with("access-control-") {
            kept.append(name.clone(), value.clone());
        }
    }
    if !gnap_only(&kept) {
        kept.remove(header::WWW_AUTHENTICATE);
    }
    kept
}

/// Whether every `WWW-Authenticate` value is a single challenge of the
/// GNAP scheme, as the node's own are (`gnap::www_authenticate`). Any other
/// scheme, such as `Basic`, would have a browser show its sign-in dialog on
/// the front door's names and send what is typed there to the node; and a
/// comma may start a second challenge, so none is taken.
fn gnap_only(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .all(|value| {
            let Ok(text) = value.to_str() else {
                return false;
            };
            let scheme = text.split([' ', '\t']).next().unwrap_or_default();
            scheme.eq_ignore_ascii_case("gnap") && !text.contains(',')
        })
}

/// The front door's own answer, in the specs' error form, with the
/// headers every answer of a node's carries.
fn error(status: StatusCode, code: &str, description: &str) -> Response<Body> {
    let body = dassie_payments::resources::ErrorBody::new(code, description);
    let body = serde_json::to_vec(&body).unwrap_or_default();
    let mut response = response::with_body(status, "application/json", body);
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response::secure(headers, true);
    harden(headers);
    response
}

/// Why a request goes to no node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refused {
    /// Its name is outside the domain, or on another port.
    Misdirected,
    /// Its name is under the domain, but no node's.
    NoNode,
    /// Its path is outside Open Payments, or it asks for an upgrade.
    NoRoute,
}

impl Refused {
    fn response(self) -> Response<Body> {
        match self {
            Self::Misdirected => error(
                StatusCode::MISDIRECTED_REQUEST,
                "misdirected_request",
                "this front door does not serve that name",
            ),
            Self::NoNode => error(StatusCode::NOT_FOUND, "not_found", "no node has that name"),
            Self::NoRoute => error(StatusCode::NOT_FOUND, "not_found", "Not Found"),
        }
    }
}

/// Takes connections on `listener` until `shutdown`, then drains them.
pub async fn serve(
    door: Arc<Door>,
    listener: TcpListener,
    tls: Arc<ServerConfig>,
    shutdown: CancellationToken,
) {
    let acceptor = TlsAcceptor::from(tls);
    let tasks = TaskTracker::new();
    let slots = ClientSlots::new(ConnectionLimits {
        max_total: MAX_CONNECTIONS,
        max_per_client: CONNECTIONS_PER_CLIENT,
    });
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = shutdown.cancelled() => break,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(target: target::FRONT_DOOR, %error, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = slots.take(Some(ClientKey::from_ip(peer.ip().to_canonical()))) else {
            tracing::debug!(target: target::FRONT_DOOR, %peer, "too many connections; closing");
            continue;
        };
        let connection = Connection {
            door: Arc::clone(&door),
            peer,
            tasks: tasks.clone(),
            shutdown: shutdown.clone(),
            _slot: slot,
        };
        tasks.spawn(connection.run(stream, acceptor.clone()));
    }
    tasks.close();
    if tokio::time::timeout(DRAIN_TIMEOUT, tasks.wait())
        .await
        .is_err()
    {
        tracing::warn!(target: target::FRONT_DOOR, "connections still open after the drain timeout");
    }
}

/// One client's connection, and its place.
struct Connection {
    door: Arc<Door>,
    peer: SocketAddr,
    /// Where the tunnels it takes wait for their upgrade.
    tasks: TaskTracker,
    shutdown: CancellationToken,
    _slot: ClientSlot,
}

impl Connection {
    /// Runs TLS, then HTTP/1.1 until the client closes or `shutdown`.
    async fn run(self, stream: TcpStream, acceptor: TlsAcceptor) {
        let _ = stream.set_nodelay(true);
        let stream =
            match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => stream,
                _ => return,
            };
        let (door, peer, tasks) = (Arc::clone(&self.door), self.peer, self.tasks.clone());
        let service = service_fn(move |request| {
            let (door, tasks) = (Arc::clone(&door), tasks.clone());
            async move { Ok::<_, Infallible>(door.route(peer, request, &tasks).await) }
        });
        // hyper bounds a request's head to 100 headers by default.
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .max_buf_size(MAX_HEAD_BYTES)
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades();
        let mut connection = pin!(connection);
        tokio::select! {
            _ = connection.as_mut() => {}
            () = self.shutdown.cancelled() => {
                connection.as_mut().graceful_shutdown();
                let _ = connection.await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use bytes::Bytes;

    fn names(kept: &HeaderMap) -> Vec<String> {
        kept.keys().map(|name| name.as_str().to_string()).collect()
    }

    fn headers(fields: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in fields {
            headers.append(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        }
        headers
    }

    #[test]
    fn only_end_to_end_headers_are_passed_on() {
        let request = headers(&[
            ("host", "a.example"),
            ("connection", "keep-alive, x-private"),
            ("x-private", "1"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("te", "trailers"),
            ("upgrade", "h2c"),
            ("expect", "100-continue"),
            ("content-length", "12"),
            ("forwarded", "for=192.0.2.1"),
            ("x-forwarded-for", "192.0.2.1"),
            ("signature", "sig=:AA==:"),
            ("authorization", "GNAP token"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(
            names(&passed_on(&request)),
            ["host", "signature", "authorization", "content-type"]
        );
    }

    #[test]
    fn only_what_a_client_needs_is_passed_back() {
        let answer = headers(&[
            ("content-type", "application/json"),
            ("content-length", "2"),
            ("content-digest", "sha-256=:AA==:"),
            ("signature", "dassie=:AA==:"),
            ("signature-input", "dassie=()"),
            ("www-authenticate", "GNAP as_uri=\"https://a.example/gnap\""),
            ("cache-control", "max-age=60"),
            ("access-control-allow-origin", "*"),
            ("access-control-expose-headers", "Signature"),
            ("vary", "Origin"),
            (
                "content-disposition",
                "attachment; filename=\"invoice.pdf.exe\"",
            ),
            ("set-cookie", "a=b"),
            ("alt-svc", "h3=\":443\""),
            ("refresh", "0; url=https://elsewhere.example"),
            ("link", "<https://elsewhere.example>; rel=preload"),
            ("location", "https://elsewhere.example"),
            ("content-security-policy", "default-src *"),
            ("x-unknown", "1"),
            ("connection", "keep-alive"),
            ("transfer-encoding", "chunked"),
            ("date", "Thu, 01 Jan 2026 00:00:00 GMT"),
        ]);
        assert_eq!(
            names(&passed_back(&answer)),
            [
                "content-type",
                "content-digest",
                "signature",
                "signature-input",
                "www-authenticate",
                "cache-control",
                "access-control-allow-origin",
                "access-control-expose-headers",
                "vary"
            ]
        );
    }

    /// What the front door makes of a node's answer with `status`,
    /// `fields` and `body`.
    fn answer(status: u16, fields: &[(&str, &str)], body: &'static [u8]) -> Response<Body> {
        let mut response = Response::new(());
        *response.status_mut() = StatusCode::from_u16(status).unwrap();
        for (name, value) in fields {
            response.headers_mut().append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let (parts, ()) = response.into_parts();
        returned((parts, Bytes::from_static(body)))
    }

    #[test]
    fn only_json_or_nothing_comes_back() {
        let typed = |content_type: &str, body: &'static [u8]| {
            answer(200, &[("content-type", content_type)], body).status()
        };
        assert_eq!(typed("application/json", b"{}"), StatusCode::OK);
        assert_eq!(
            typed("Application/JSON; charset=utf-8", b"{}"),
            StatusCode::OK
        );
        assert_eq!(typed("text/plain", b""), StatusCode::OK);
        assert_eq!(typed("text/html", b"<p>"), StatusCode::BAD_GATEWAY);
        assert_eq!(typed("application/jsonx", b"{}"), StatusCode::BAD_GATEWAY);
        assert_eq!(answer(200, &[], b"{}").status(), StatusCode::BAD_GATEWAY);
        // A browser takes the last of two types: two are refused.
        let two = [
            ("content-type", "application/json"),
            ("content-type", "text/html"),
        ];
        assert_eq!(answer(200, &two, b"<p>").status(), StatusCode::BAD_GATEWAY);
        assert_eq!(answer(204, &two, b"").status(), StatusCode::BAD_GATEWAY);
        // A redirect goes nowhere: its `Location` stays behind.
        let redirect = answer(302, &[("location", "https://elsewhere.example/")], b"");
        assert_eq!(redirect.status(), StatusCode::FOUND);
        assert!(redirect.headers().get(header::LOCATION).is_none());
        assert_eq!(answer(304, &[], b"").status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn only_the_gnap_challenge_passes() {
        let challenges = |fields: &[(&'static str, &'static str)]| -> Vec<String> {
            passed_back(&headers(fields))
                .get_all(header::WWW_AUTHENTICATE)
                .iter()
                .map(|value| value.to_str().unwrap().to_string())
                .collect()
        };
        let gnap = "GNAP as_uri=https://a.dassiepay.net/gnap";
        assert_eq!(challenges(&[("www-authenticate", gnap)]), [gnap]);
        assert_eq!(
            challenges(&[("www-authenticate", "gnap as_uri=https://a.example/gnap")]),
            ["gnap as_uri=https://a.example/gnap"]
        );
        for fields in [
            &[("www-authenticate", "Basic realm=\"Sign in to Dassie\"")][..],
            &[
                ("www-authenticate", gnap),
                ("www-authenticate", "Basic realm=\"x\""),
            ],
            &[("www-authenticate", "GNAP as_uri=x, Basic realm=\"x\"")],
            &[("www-authenticate", "GNAPish realm=\"x\"")],
        ] {
            assert!(challenges(fields).is_empty(), "{fields:?}");
        }
    }

    #[test]
    fn the_front_doors_own_policy_replaces_the_nodes() {
        let fields = [
            ("content-type", "application/json; charset=utf-8"),
            ("content-security-policy", "default-src *"),
            ("x-content-type-options", "sniff-away"),
            ("signature", "dassie=:AA==:"),
        ];
        let response = answer(200, &fields, b"{}");
        let headers = response.headers();
        let all = |name: &HeaderName| -> Vec<&str> {
            headers
                .get_all(name)
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect()
        };
        assert_eq!(all(&header::CONTENT_TYPE), ["application/json"]);
        assert_eq!(
            all(&header::CONTENT_SECURITY_POLICY),
            [CONTENT_SECURITY_POLICY]
        );
        assert_eq!(all(&header::X_CONTENT_TYPE_OPTIONS), ["nosniff"]);
        assert_eq!(
            all(&HeaderName::from_static("signature")),
            ["dassie=:AA==:"]
        );
        // The front door's own answers carry them too.
        let own = error(StatusCode::NOT_FOUND, "not_found", "Not Found");
        let csp = own.headers().get(header::CONTENT_SECURITY_POLICY).unwrap();
        assert_eq!(csp, CONTENT_SECURITY_POLICY);
    }

    #[test]
    fn the_relay_is_where_its_url_leads() {
        let name = |name: &str| Host::Name(name.to_string());
        assert_eq!(
            relay_authority("https://relay.dassiepay.net"),
            Some((name("relay.dassiepay.net"), 443))
        );
        assert_eq!(
            relay_authority("https://Relay.Example:8443"),
            Some((name("relay.example"), 8443))
        );
        assert_eq!(
            relay_authority("https://[::1]:8443"),
            Some((Host::Ip(std::net::Ipv6Addr::LOCALHOST.into()), 8443))
        );
        assert_eq!(relay_authority("http://relay.dassiepay.net"), None);
    }

    #[test]
    fn a_client_counts_per_node_apart() {
        let client = ClientKey::from_ip([192, 0, 2, 1].into());
        let other = ClientKey::from_ip([192, 0, 2, 2].into());
        let (a, b) = (
            dassie_identity::TableKeygen::identity_key(&[1; 32]).node_id(),
            dassie_identity::TableKeygen::identity_key(&[2; 32]).node_id(),
        );
        assert_eq!(per_node(client, &a), per_node(client, &a));
        assert_ne!(per_node(client, &a), per_node(client, &b));
        assert_ne!(per_node(client, &a), per_node(other, &a));
        let mut limiter = RateLimiter::new(REQUESTS_PER_NODE);
        for _ in 0..REQUESTS_PER_NODE.client_burst {
            assert!(limiter.check(per_node(client, &a), 0).is_ok());
        }
        assert!(limiter.check(per_node(client, &a), 0).is_err());
        assert!(limiter.check(per_node(client, &b), 0).is_ok());
    }
}
