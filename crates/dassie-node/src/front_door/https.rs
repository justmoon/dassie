//! The front door's HTTPS listener (spec/areas/http.md#front-doors): it
//! takes Open Payments requests for `<label>.<domain>`, passes them to the
//! node the label names, and passes the node's answer back as it came, so
//! the node's signature on it still verifies.
//!
//! It passes on only Open Payments' routes, with bounded heads and bodies,
//! and only end-to-end headers. It returns only JSON, or no body. It never
//! serves a page, the wallet or a cookie of its own or of a node's.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::NodeId;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::origin::{Host, parse_authority};
use dassie_node_core::rate_limit::{ClientKey, RateLimiter};
use dassie_node_core::urls::DEFAULT_HTTPS_PORT;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::nodes::{Answer, Missed, Nodes};
use crate::clock::Clock;
use crate::http::open_payments::{MAX_BODY_BYTES, is_open_payments_path};
use crate::http::response::{self, Body, header_has_token, header_str};
use crate::http::{
    DRAIN_TIMEOUT, HEADER_TIMEOUT, MAX_CONNECTIONS, MAX_HEAD_BYTES, TLS_HANDSHAKE_TIMEOUT,
};
use crate::log::target;
use crate::node::{Policies, lock};
use crate::open_payments::REQUESTS;
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
    nodes: Arc<Nodes>,
    requests: Mutex<RateLimiter>,
    clock: Arc<dyn Clock>,
}

/// Where a request goes.
enum Target {
    /// To the node with this ID.
    Node(NodeId),
    /// The consent route, which the front door answers.
    Interact,
}

impl Door {
    /// The front door for nodes under `domain`, reached through `nodes`.
    #[must_use]
    pub fn new(domain: &str, nodes: Arc<Nodes>, clock: Arc<dyn Clock>) -> Self {
        Self {
            domain: domain.to_ascii_lowercase(),
            nodes,
            requests: Mutex::new(RateLimiter::new(REQUESTS)),
            clock,
        }
    }

    /// Answers one request from `peer`.
    async fn route(&self, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
        let client = ClientKey::from_ip(peer.ip().to_canonical());
        let allowed = lock(&self.requests).check(client, self.clock.now_ms());
        if allowed.is_err() {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "slow down",
            );
        }
        match self.target(&request) {
            Err(refused) => refused.response(),
            Ok(Target::Interact) => error(
                StatusCode::NOT_IMPLEMENTED,
                "interaction_unavailable",
                "interactive grants need the owner's Dassie app, which is not available yet",
            ),
            Ok(Target::Node(node)) => self.pass(node, request).await,
        }
    }

    /// Where `request` goes, or why it goes nowhere: a name outside the
    /// domain is refused, and a name that is no node's label, a path
    /// outside Open Payments or an upgrade find nothing.
    fn target(&self, request: &Request<Incoming>) -> Result<Target, Refused> {
        let host = header_str(request.headers(), &header::HOST);
        let node = self.node_of(host)?;
        let path = request.uri().path();
        if header_has_token(request.headers(), &header::CONNECTION, "upgrade")
            || !is_open_payments_path(path)
        {
            return Err(Refused::NoRoute);
        }
        if path.starts_with(INTERACT_PREFIX) {
            return Ok(Target::Interact);
        }
        Ok(Target::Node(node))
    }

    /// The node `<label>.<domain>` names, on the default port.
    fn node_of(&self, host: Option<&str>) -> Result<NodeId, Refused> {
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
        NodeId::from_label(label).map_err(|_| Refused::NoNode)
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
        *forwarded.headers_mut() = end_to_end(&parts.headers, true);
        match self.nodes.send(node, forwarded).await {
            Ok(answer) => returned(answer),
            Err(missed) => {
                tracing::debug!(target: target::FRONT_DOOR, %node, ?missed, "no answer from the node");
                unanswered(missed)
            }
        }
    }
}

/// The answer as the node gave it, without the headers that concern the
/// stream it came over and without cookies; or 502, unless it is JSON or
/// has no body.
fn returned((parts, body): Answer) -> Response<Body> {
    let is_json = header_str(&parts.headers, &header::CONTENT_TYPE)
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
    if !body.is_empty() && !is_json {
        return error(
            StatusCode::BAD_GATEWAY,
            "bad_gateway",
            "the node did not answer with JSON",
        );
    }
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = parts.status;
    *response.headers_mut() = end_to_end(&parts.headers, false);
    response
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

/// The end-to-end headers of `headers`: without hop-by-hop headers and
/// those `Connection` names; for a request also without `Expect`,
/// `Content-Length` (the body is read, and its length sent anew) and what
/// proxies add (`Forwarded`, `X-Forwarded-*`); for a response, without
/// cookies.
fn end_to_end(headers: &HeaderMap, request: bool) -> HeaderMap {
    let named: Vec<String> = response::header_values(headers, &header::CONNECTION)
        .flat_map(|value| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let dropped = |name: &HeaderName| {
        let name = name.as_str();
        let own = if request {
            matches!(name, "expect" | "content-length" | "forwarded")
                || name.starts_with("x-forwarded-")
        } else {
            matches!(name, "set-cookie" | "set-cookie2")
        };
        own || HOP_BY_HOP.contains(&name) || named.iter().any(|token| token == name)
    };
    let mut kept = HeaderMap::new();
    for (name, value) in headers {
        if !dropped(name) {
            kept.append(name.clone(), value.clone());
        }
    }
    kept
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
        let (door, peer) = (Arc::clone(&self.door), self.peer);
        let service = service_fn(move |request| {
            let door = Arc::clone(&door);
            async move { Ok::<_, Infallible>(door.route(peer, request).await) }
        });
        // hyper bounds a request's head to 100 headers by default.
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .max_buf_size(MAX_HEAD_BYTES)
            .serve_connection(TokioIo::new(stream), service);
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

    #[test]
    fn only_end_to_end_headers_pass() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
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
            ("set-cookie", "a=b"),
            ("content-type", "application/json"),
        ] {
            headers.append(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        }
        let names = |kept: HeaderMap| -> Vec<String> {
            kept.keys().map(|name| name.as_str().to_string()).collect()
        };
        assert_eq!(
            names(end_to_end(&headers, true)),
            ["host", "signature", "set-cookie", "content-type"]
        );
        assert_eq!(
            names(end_to_end(&headers, false)),
            [
                "host",
                "expect",
                "content-length",
                "forwarded",
                "x-forwarded-for",
                "signature",
                "content-type"
            ]
        );
    }

    #[test]
    fn only_json_or_nothing_comes_back() {
        let answer = |content_type: &str, body: &'static [u8]| {
            let mut response = Response::new(());
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type).unwrap(),
            );
            let (parts, ()) = response.into_parts();
            returned((parts, Bytes::from_static(body))).status()
        };
        assert_eq!(answer("application/json", b"{}"), StatusCode::OK);
        assert_eq!(
            answer("Application/JSON; charset=utf-8", b"{}"),
            StatusCode::OK
        );
        assert_eq!(answer("text/plain", b""), StatusCode::OK);
        assert_eq!(answer("text/html", b"<p>"), StatusCode::BAD_GATEWAY);
        assert_eq!(answer("application/jsonx", b"{}"), StatusCode::BAD_GATEWAY);
    }
}
