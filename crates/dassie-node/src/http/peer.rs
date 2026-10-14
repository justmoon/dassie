//! `POST /peer`: the peer protocol's endpoint (spec 3 §6, spec 7 §4.7).
//!
//! This layer checks only what needs no state: the media types, the body's
//! size and pace, the number of requests waiting, and the rate of
//! unauthenticated or failing requests per address. The engine's peer node
//! decides the rest, including the status (NET-11), and answers once what
//! the request changed is on disk.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use dassie_peer::wire::{
    Authentication, Envelope, MAX_MESSAGE_BYTES, MESSAGE_CONTENT_TYPE, RESPONSE_CONTENT_TYPE,
};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header;
use hyper::{Request, Response, StatusCode};

use super::response::{self, Body, header_str};
use crate::node::Node;

/// Handles a peer request.
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
    if node.peer_limited(peer.ip()) {
        return response::text(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests");
    }
    let (Some(engine), Some(_permit)) = (node.engine(), node.peer_permit()) else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    };
    let timeout = Duration::from_millis(node.policies().body_timeout_ms);
    let collect = Limited::new(request.into_body(), MAX_MESSAGE_BYTES).collect();
    let body: Bytes = match tokio::time::timeout(timeout, collect).await {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(_)) => return response::text(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"),
        Err(_) => return response::text(StatusCode::REQUEST_TIMEOUT, "Request Timeout"),
    };
    let anonymous = Envelope::decode(&body)
        .is_ok_and(|envelope| envelope.authentication == Authentication::None);
    let Ok(reply) = engine.peer_inbound(body.to_vec(), Some(peer.ip())).await else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
    };
    // Unauthenticated requests are for tools; refused ones may be someone
    // making the node verify signatures, exchange keys or probe URLs for
    // nothing. All are rationed.
    if anonymous || matches!(reply.status, 400 | 401 | 409 | 429) {
        node.peer_charge(peer.ip());
    }
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::OK {
        response::with_body(status, RESPONSE_CONTENT_TYPE, reply.body)
    } else {
        response::with_body(status, "text/plain; charset=utf-8", reply.body)
    }
}
