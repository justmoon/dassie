//! The authorization server's endpoints.

use std::net::SocketAddr;

use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::{ReadRequest, error, json};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::Urls;
use crate::open_payments::auth::{self, OpError};
use crate::store::PaymentId;

/// `POST|DELETE /gnap/continue/{id}` and `POST|DELETE /gnap/token/{id}`.
pub async fn manage(
    node: &Node,
    urls: &Urls,
    method: &Method,
    path: &str,
    request: Request<Incoming>,
) -> Response<Body> {
    let read = match ReadRequest::read(node, urls, request).await {
        Ok(read) => read,
        Err(failure) => return error(&failure, None),
    };
    let fields = read.field_refs();
    let signed = read.signed(&fields);
    let target = path
        .strip_prefix("/gnap/continue/")
        .map(|id| (true, id))
        .or_else(|| path.strip_prefix("/gnap/token/").map(|id| (false, id)));
    let Some((continuation, id)) = target else {
        return error(&OpError::not_found("Not Found"), None);
    };
    let Some(id) = PaymentId::parse(id) else {
        return error(&OpError::not_found("Not Found"), None);
    };
    let result = match (continuation, method) {
        (true, &Method::POST) => {
            auth::continue_grant(node, id, &signed).map(|grant| json(StatusCode::OK, &grant))
        }
        (true, &Method::DELETE) => auth::revoke_grant(node, id, &signed).map(|()| no_content()),
        (false, &Method::POST) => auth::rotate_token(node, urls, id, &signed).map(|access_token| {
            json(
                StatusCode::OK,
                &serde_json::json!({ "access_token": access_token }),
            )
        }),
        (false, &Method::DELETE) => auth::revoke_token(node, id, &signed).map(|()| no_content()),
        _ => Err(OpError::not_found("Not Found")),
    };
    result.unwrap_or_else(|failure| error(&failure, None))
}

fn no_content() -> Response<Body> {
    crate::http::response::with_body(StatusCode::NO_CONTENT, "application/json", "")
}

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
    let read = match ReadRequest::read(node, urls, request).await {
        Ok(read) => read,
        Err(failure) => return error(&failure, None),
    };
    let fields = read.field_refs();
    match auth::request_grant(node, urls, &read.signed(&fields)).await {
        Ok(grant) => json(StatusCode::OK, &grant),
        Err(failure) => error(&failure, None),
    }
}
