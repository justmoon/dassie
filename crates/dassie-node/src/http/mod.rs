//! The HTTP and HTTPS listeners.
//!
//! HTTP/1.1 only, over hyper, with a route table small enough to read in one
//! place ([`route`]). Each listener bounds its connections, in all and per
//! client, the time to complete a TLS handshake and to send headers, and on
//! shutdown stops accepting, lets requests in progress finish, and closes
//! WebSockets with 1001, waiting at most [`DRAIN_TIMEOUT`].

mod api;
pub mod assets;
mod btp;
pub mod hub;
pub mod mime;
pub mod open_payments;
pub mod peer;
pub mod response;
pub mod stats;
mod ws;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::ServerConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::urls;
use response::{Body, header_str};

pub use ws::SESSION_ENDED;

use crate::log::target;
use crate::node::Node;
use crate::slots::{ClientSlot, ClientSlots};

/// Most connections a listener holds at once; more are closed on arrival.
/// One client holds at most `Policies::http_connections_per_client` of
/// them, so that a few addresses cannot take them all.
pub const MAX_CONNECTIONS: usize = 1024;
/// Time to complete a TLS handshake.
pub const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time to send a request's headers.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Time connections get to finish at shutdown.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest buffered request head, in bytes.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Which listener a request came in on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Plain HTTP: ACME challenges and redirects.
    Http,
    /// HTTPS: everything else.
    Https,
}

#[derive(Clone)]
struct Context {
    node: Arc<Node>,
    kind: Kind,
    tasks: TaskTracker,
    shutdown: CancellationToken,
}

/// Accepts connections on `listener` until `shutdown`, then drains them.
pub async fn serve(
    node: Arc<Node>,
    listener: Arc<TcpListener>,
    tls: Option<Arc<ServerConfig>>,
    shutdown: CancellationToken,
) {
    let kind = if tls.is_some() {
        Kind::Https
    } else {
        Kind::Http
    };
    let acceptor = tls.map(TlsAcceptor::from);
    let context = Context {
        node,
        kind,
        tasks: TaskTracker::new(),
        shutdown,
    };
    let slots = ClientSlots::new(ConnectionLimits {
        max_total: MAX_CONNECTIONS,
        max_per_client: context.node.policies().http_connections_per_client,
    });
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = context.shutdown.cancelled() => break,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors and the like: wait a little.
                tracing::warn!(target: target::HTTP, %error, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let slot = match slots.take(context.node.counted_client(peer.ip())) {
            Ok(slot) => slot,
            Err(refused) => {
                tracing::debug!(target: target::HTTP, %peer, ?refused, "too many connections; closing");
                continue;
            }
        };
        context.tasks.spawn(connection(
            context.clone(),
            stream,
            peer,
            acceptor.clone(),
            slot,
        ));
    }
    context.tasks.close();
    if tokio::time::timeout(DRAIN_TIMEOUT, context.tasks.wait())
        .await
        .is_err()
    {
        tracing::warn!(target: target::HTTP, ?kind, "connections still open after the drain timeout");
    }
}

async fn connection(
    context: Context,
    stream: TcpStream,
    peer: SocketAddr,
    acceptor: Option<TlsAcceptor>,
    _slot: ClientSlot,
) {
    let _ = stream.set_nodelay(true);
    match acceptor {
        None => http(context, stream, peer).await,
        Some(acceptor) => {
            match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => http(context, stream, peer).await,
                Ok(Err(error)) => {
                    tracing::debug!(target: target::HTTP, %peer, %error, "TLS handshake failed")
                }
                Err(_) => tracing::debug!(target: target::HTTP, %peer, "TLS handshake timed out"),
            }
        }
    }
}

async fn http<S>(context: Context, stream: S, peer: SocketAddr)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let shutdown = context.shutdown.clone();
    let service = service_fn(move |request| {
        let context = context.clone();
        async move { Ok::<_, Infallible>(route(&context, peer, request).await) }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(MAX_HEAD_BYTES)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    let mut connection = pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        () = shutdown.cancelled() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

/// Serves HTTP/1.1 on a relayed stream with purpose payments
/// (spec/areas/http.md#relayed-streams), once its TLS is done: Open Payments
/// and nothing else. The wallet, `/api/*`, `/rpc`, `/peer` and BTP are
/// never served this way (spec/decisions/core-and-periphery.md), since the
/// page could keep the owner's passphrase, and nothing on a relayed stream
/// proves a browser's origin. `client` is the node the TLS proved; `relay`
/// the address of the node that carried the stream, which the rate limits
/// count requests against.
pub async fn serve_relayed<S>(
    node: Arc<Node>,
    io: S,
    client: dassie_identity::NodeId,
    relay: SocketAddr,
    shutdown: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tracing::debug!(target: target::HTTP, %client, %relay, "serving a relayed stream");
    let service = service_fn(move |request| {
        let node = Arc::clone(&node);
        async move { Ok::<_, Infallible>(route_relayed(&node, relay, request).await) }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(MAX_HEAD_BYTES)
        .serve_connection(TokioIo::new(io), service);
    let mut connection = pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        () = shutdown.cancelled() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

/// Open Payments' routes, and 404 for everything else, upgrades included.
async fn route_relayed(
    node: &Node,
    relay: SocketAddr,
    request: Request<Incoming>,
) -> Response<Body> {
    let path = request.uri().path();
    let mut response = if ws::is_upgrade(&request) || !open_payments::is_open_payments_path(path) {
        response::not_found()
    } else {
        open_payments::handle(node, relay, request).await
    };
    response::secure(response.headers_mut(), false);
    response
}

async fn route(context: &Context, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    match context.kind {
        Kind::Http => route_http(&context.node, &request),
        Kind::Https => {
            let hsts = context.node.certificate().info().source.allows_hsts();
            let mut response = route_https(context, peer, request).await;
            if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                response::secure(response.headers_mut(), hsts);
            }
            response
        }
    }
}

/// The HTTP listener serves ACME challenges and redirects everything else
/// to HTTPS, keeping the path and query (RT-1).
fn route_http(node: &Node, request: &Request<Incoming>) -> Response<Body> {
    let path = request.uri().path();
    if ws::is_upgrade(request) {
        return response::not_found();
    }
    if let Some(token) = path.strip_prefix("/.well-known/acme-challenge/") {
        if request.method() != Method::GET {
            return response::not_found();
        }
        if token.is_empty() {
            return response::text(StatusCode::BAD_REQUEST, "Bad Request");
        }
        return match node.challenge(token) {
            Some(key_authorization) => {
                response::with_body(StatusCode::OK, "text/plain", key_authorization)
            }
            None => response::not_found(),
        };
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return response::not_found();
    }
    let config = node.config();
    let path_and_query = request.uri().path_and_query().map_or("/", |pq| pq.as_str());
    let host = header_str(request.headers(), &header::HOST);
    match urls::redirect_location(
        config.hostname.as_ref(),
        host,
        node.ports().1,
        path_and_query,
    ) {
        Some(location) => {
            let mut response = response::text(StatusCode::MOVED_PERMANENTLY, "Moved Permanently");
            if let Ok(location) = HeaderValue::from_str(&location) {
                response.headers_mut().insert(header::LOCATION, location);
            }
            response
        }
        None => response::text(StatusCode::BAD_REQUEST, "Bad Request"),
    }
}

async fn route_https(
    context: &Context,
    peer: SocketAddr,
    request: Request<Incoming>,
) -> Response<Body> {
    let node = &context.node;
    let path = request.uri().path().to_string();
    if path == "/btp" && ws::is_upgrade(&request) {
        return btp::upgrade(node, request, peer, &context.tasks, &context.shutdown);
    }
    if path == "/rpc" {
        return if ws::is_upgrade(&request) {
            ws::upgrade(node, request, peer, &context.tasks, &context.shutdown)
        } else {
            response::text(StatusCode::UPGRADE_REQUIRED, "Upgrade Required")
        };
    }
    // RT-16: the original left unmatched upgrade requests hanging.
    if ws::is_upgrade(&request) {
        return response::not_found();
    }
    let method = request.method().clone();
    let set_up = node.is_set_up();
    match (&method, path.as_str()) {
        (&Method::POST, "/api/setup") if !set_up => api::setup(node, peer, request).await,
        (&Method::POST, "/api/login") if set_up => api::login(node, peer, request).await,
        (&Method::POST, "/api/logout") if set_up => api::logout(node, &request),
        (&Method::POST, "/peer") => peer::handle(node, peer, request).await,
        (&Method::POST, "/hub") => hub::handle(node, peer, request).await,
        (_, path) if open_payments::is_open_payments_path(path) => {
            open_payments::handle(node, peer, request).await
        }
        (_, path) if path.starts_with("/api/") => response::not_found(),
        (&Method::GET | &Method::HEAD, "/stats") => stats::serve(node).await,
        (&Method::GET | &Method::HEAD, path) => match node.dev_wallet_dir() {
            Some(root) => assets::serve_dir(root, &method, path, request.headers()).await,
            None => assets::serve(&method, path, request.headers()),
        },
        _ => {
            let mut response = response::text(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            response
        }
    }
}
