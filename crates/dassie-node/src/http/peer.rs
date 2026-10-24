//! `POST /peer`: the peer protocol's endpoint over HTTPS, and what it shares
//! with the one over QUIC (spec/areas/peer-protocol.md#transport).
//!
//! This layer checks only what needs no state: the media types, the body's
//! size and pace, the number of requests waiting, and the rate of
//! unauthenticated or failing requests per address. The engine's peer node
//! decides the rest, including the status (NET-11) and what a network of
//! addresses may hold, and answers once what the request changed is on
//! disk.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeId;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use tokio_util::sync::CancellationToken;

use dassie_peer::wire::{
    Authentication, Envelope, MAX_MESSAGE_BYTES, MESSAGE_CONTENT_TYPE, RESPONSE_CONTENT_TYPE,
};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header;
use hyper::{Request, Response, StatusCode};

use super::response::{self, Body, header_str};
use crate::node::Node;
use crate::quic::places::Peerings;
use crate::quic::relay::{Purpose, ServedStream};

// The largest ILP packet of any type fits in a request, with room for the
// envelope's sender, authentication and tags.
const _: () = assert!(dassie_ilp::MAX_PACKET_LENGTH + 1024 <= MAX_MESSAGE_BYTES);

/// Why a request's body could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyError {
    /// It is longer than [`MAX_MESSAGE_BYTES`].
    TooLarge,
    /// The connection or stream broke while it was read.
    Broken,
}

/// The answer to a peer request: for 200, a `PeerResponse`; otherwise a
/// short text for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The status (spec/areas/peer-protocol.md#statuses).
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

impl Answer {
    fn text(status: StatusCode, text: &str) -> Self {
        Self {
            status: status.as_u16(),
            body: text.as_bytes().to_vec(),
        }
    }
}

/// Answers a peer request from `client` whose body `body` reads, whichever
/// transport carried it. The body is read only once the request may be
/// handled, and within the node's time for a body.
pub async fn answer<F>(node: &Node, client: IpAddr, body: F) -> Answer
where
    F: Future<Output = Result<Vec<u8>, BodyError>>,
{
    if node.peer_limited(client) {
        return Answer::text(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
    }
    let (Some(engine), Some(_permit)) = (node.engine(), node.peer_permit(client)) else {
        return Answer::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    };
    let timeout = Duration::from_millis(node.policies().body_timeout_ms);
    // A body that is too large, broken or slow held a place for nothing:
    // charged, so that a client cannot hold places over and over.
    let (status, text) = match tokio::time::timeout(timeout, body).await {
        Ok(Ok(body)) => return answer_body(node, &engine, client, body).await,
        Ok(Err(BodyError::TooLarge)) => (StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"),
        Ok(Err(BodyError::Broken)) => (StatusCode::BAD_REQUEST, "Bad Request"),
        Err(_) => (StatusCode::REQUEST_TIMEOUT, "Request Timeout"),
    };
    node.peer_charge(client);
    Answer::text(status, text)
}

/// Hands a request's body to the engine and returns its reply.
async fn answer_body(
    node: &Node,
    engine: &crate::engine::Engine,
    client: IpAddr,
    body: Vec<u8>,
) -> Answer {
    let anonymous = Envelope::decode(&body)
        .is_ok_and(|envelope| envelope.authentication == Authentication::None);
    // Loopback is no network of its own where private peers are allowed, as
    // for the limits per client: dev and the tests run many nodes there.
    let source = node.counted_client(client).map(|_| client.to_canonical());
    let Ok(reply) = engine.peer_inbound(body, source).await else {
        return Answer::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    };
    // Unauthenticated requests are for tools; refused ones may be someone
    // making the node verify signatures, exchange keys or probe URLs for
    // nothing. All are rationed.
    if anonymous || matches!(reply.status, 400 | 401 | 409 | 429) {
        node.peer_charge(client);
    }
    Answer {
        status: reply.status,
        body: reply.body,
    }
}

/// Answers peer requests over QUIC with the node, as over HTTPS: the same
/// checks, limits and statuses, and the same bound on connections from
/// anyone. Core peers and downlinks have places of their own.
#[derive(Debug)]
pub struct QuicHandler(pub Arc<Node>);

impl crate::quic::Handler for QuicHandler {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        self.0.counted_client(address)
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: super::MAX_CONNECTIONS,
            max_per_client: self.0.policies().http_connections_per_client,
        }
    }

    /// Core peers and downlinks, each under the peer node's own bound.
    fn peerings(&self) -> Option<Peerings> {
        let peer = &self.0.policies().peer;
        Some(Peerings {
            classes: self.0.peerings(),
            max_core_peers: peer.max_peers,
            max_downlinks: peer.max_downlinks,
        })
    }

    fn answer<'a>(
        &'a self,
        client: IpAddr,
        body: crate::quic::Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(answer(&self.0, client, body))
    }

    /// The hubs the node validates, for validators and members alike: the
    /// connection proved who sent it.
    fn hub<'a>(
        &'a self,
        from: dassie_identity::PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<crate::hub::HubAnswer>> + Send + 'a>> {
        Box::pin(async move {
            let from = crate::hub::Sender::Node(from);
            Some(self.0.hubs().answer(from, &request, false).await)
        })
    }

    fn hub_access(
        &self,
        from: &dassie_identity::PublicKey,
        hub: &[u8; 32],
    ) -> crate::hub::HubAccess {
        self.0.hubs().access(Some(from), hub)
    }

    fn hub_permit(
        &self,
        access: crate::hub::HubAccess,
    ) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.0.hubs().request_permit(access)
    }

    /// The states the node keeps of the hubs it validates, for their other
    /// validators only.
    fn hub_state<'a>(
        &'a self,
        from: dassie_identity::PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<crate::hub::transport::StateReply>> + Send + 'a>> {
        Box::pin(async move {
            let store = self.0.store_handle();
            Some(self.0.hubs().answer_state(store, &from, &request).await)
        })
    }

    /// Open Payments, and owner apps' RPC. The wallet, `/api/*` and `/rpc`
    /// are never relayed (spec/decisions/core-and-periphery.md): an owner
    /// app proves its device key instead of carrying a browser's cookie.
    fn serves(&self, purpose: Purpose) -> bool {
        matches!(purpose, Purpose::Payments | Purpose::Owner)
    }

    fn serve_relayed<'a>(
        &'a self,
        purpose: Purpose,
        client: NodeId,
        relay: SocketAddr,
        stream: ServedStream,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let node = Arc::clone(&self.0);
        Box::pin(async move {
            match purpose {
                Purpose::Payments => {
                    super::serve_relayed(node, stream, client, relay, shutdown).await;
                }
                Purpose::Owner => crate::owner_app::serve(node, stream, client, shutdown).await,
            }
        })
    }
}

/// Handles a peer request over HTTPS.
pub async fn handle(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    let headers = request.headers();
    if header_str(headers, &header::ACCEPT) != Some(MESSAGE_CONTENT_TYPE) {
        return response::text(
            StatusCode::NOT_ACCEPTABLE,
            "Not Acceptable, expected application/dassie-message",
        );
    }
    if header_str(headers, &header::CONTENT_TYPE) != Some(MESSAGE_CONTENT_TYPE) {
        return response::text(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported Media Type, expected application/dassie-message",
        );
    }
    let body = async {
        Limited::new(request.into_body(), MAX_MESSAGE_BYTES)
            .collect()
            .await
            .map(|collected| collected.to_bytes().to_vec())
            .map_err(|_| BodyError::TooLarge)
    };
    let answer = answer(node, peer.ip(), body).await;
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::OK {
        response::with_body(status, RESPONSE_CONTENT_TYPE, answer.body)
    } else {
        response::with_body(status, "text/plain; charset=utf-8", answer.body)
    }
}
