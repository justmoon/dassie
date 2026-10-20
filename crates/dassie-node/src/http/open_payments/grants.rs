//! The authorization server's endpoints.

use std::net::SocketAddr;

use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::{ReadRequest, error, json};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::Urls;
use crate::open_payments::auth::{self, OpError};
use crate::open_payments::consent;
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
        (true, &Method::POST) => auth::continue_grant(node, urls, id, &signed)
            .await
            .map(|grant| json(StatusCode::OK, &grant)),
        (true, &Method::DELETE) => auth::revoke_grant(node, id, &signed)
            .await
            .map(|()| no_content()),
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

/// `GET /gnap/interact/{id}/{key}`: sends the owner to the wallet's
/// consent page for the grant. A person reads what this answers, so an
/// error is plain text.
pub fn interact(node: &Node, urls: &Urls, path: &str) -> Response<Body> {
    let target = path
        .strip_prefix("/gnap/interact/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(id, key)| Some((PaymentId::parse(id)?, key)));
    let location = target.and_then(|(id, key)| consent::start(node, urls, id, key).ok());
    match location.and_then(|location| http::HeaderValue::from_str(&location).ok()) {
        Some(location) => {
            let mut response =
                crate::http::response::with_body(StatusCode::FOUND, "text/plain", "");
            response
                .headers_mut()
                .insert(http::header::LOCATION, location);
            response
        }
        None => crate::http::response::with_body(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "This request for your consent is no longer waiting: it was decided, or it lapsed after 10 minutes. Ask the app to start again.",
        ),
    }
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
    let network = node.counted_network(peer.ip());
    match auth::request_grant(node, urls, &read.signed(&fields), network).await {
        Ok(grant) => json(StatusCode::OK, &grant),
        Err(failure) => error(&failure, None),
    }
}
