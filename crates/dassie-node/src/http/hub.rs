//! `POST /hub`: a settlement hub's stream over HTTPS, for members whose
//! QUIC cannot reach a validator (spec/areas/peer-protocol.md#the-hubs-stream).
//! Validators never use it, so it takes only a member's request.
//!
//! A client proves no key here, so every request is charged to its address
//! at the rate of the peer protocol's unauthenticated requests, and holds
//! one of the places for requests waiting, as `POST /peer` does.

use std::net::SocketAddr;
use std::time::Duration;

use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header;
use hyper::{Request, Response, StatusCode};

use super::response::{self, Body, header_str};
use crate::hub::transport::{HubAnswer, MAX_REQUEST_BYTES, MEDIA_TYPE, Sender, encode_frames};
use crate::node::Node;

/// Handles a member's request to a hub the node validates.
pub async fn handle(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    if header_str(request.headers(), &header::CONTENT_TYPE) != Some(MEDIA_TYPE) {
        return response::text(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported Media Type, expected application/dassie-hub",
        );
    }
    let client = peer.ip();
    if node.peer_limited(client) {
        return response::text(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
    }
    let Some(_permit) = node.peer_permit(client) else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    };
    node.peer_charge(client);
    let timeout = Duration::from_millis(node.policies().body_timeout_ms);
    let body = Limited::new(request.into_body(), MAX_REQUEST_BYTES).collect();
    let body = match tokio::time::timeout(timeout, body).await {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(_)) => return response::text(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"),
        Err(_) => return response::text(StatusCode::REQUEST_TIMEOUT, "Request Timeout"),
    };
    match node
        .hubs()
        .answer(Sender::Address(client), &body, true)
        .await
    {
        HubAnswer::Taken(messages) => {
            response::with_body(StatusCode::OK, MEDIA_TYPE, encode_frames(&messages))
        }
        HubAnswer::UnknownHub => response::not_found(),
        HubAnswer::Busy => response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable"),
        HubAnswer::Malformed => response::text(StatusCode::BAD_REQUEST, "Bad Request"),
    }
}
