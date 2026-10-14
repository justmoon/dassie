//! The authorization server's endpoints.

use std::net::SocketAddr;

use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};

use super::{ReadRequest, error, json};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::Urls;
use crate::open_payments::auth::{self, OpError};

/// `POST /gnap`: a grant request.
pub async fn request(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    request: Request<Incoming>,
) -> Response<Body> {
    if !node
        .open_payments()
        .allow_creation(peer.ip(), node.now_ms())
    {
        return error(&OpError::new(429, "too_many_requests", "slow down"), None);
    }
    let read = match ReadRequest::read(node, request).await {
        Ok(read) => read,
        Err(failure) => return error(&failure, None),
    };
    let fields = read.field_refs();
    match auth::request_grant(node, urls, &read.signed(&fields)).await {
        Ok(grant) => json(StatusCode::OK, &grant),
        Err(failure) => error(&failure, None),
    }
}
