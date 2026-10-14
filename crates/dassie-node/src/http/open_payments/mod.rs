//! Open Payments over HTTPS (D9, spec 7 §4.3): the wallet addresses, the
//! authorization server and the resource server.
//!
//! Unlike the owner's endpoints, these are for anyone: they check no
//! `Origin` and allow every origin (CORS, without credentials), since a
//! client proves itself by its GNAP token and its signature, never by a
//! cookie. Every request is rate-limited per address, bodies are bounded,
//! and errors take the specs' form.

mod grants;
mod incoming;
mod wallet;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;

use super::response::{self, Body, header_str};
use crate::node::Node;
use crate::open_payments::auth::{OpError, SignedRequest};
use crate::open_payments::{GRANT_PATH, INCOMING_PREFIX, PAY_PREFIX, Urls, WELL_KNOWN_PATH};

/// The largest request body, in bytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The headers a cross-origin client may send.
const ALLOWED_HEADERS: &str = "Accept, Authorization, Content-Type, Content-Digest, Content-Length, Signature, Signature-Input";

/// Whether `path` is one of Open Payments'.
#[must_use]
pub fn is_open_payments_path(path: &str) -> bool {
    path == WELL_KNOWN_PATH
        || path.starts_with("/.well-known/pay/")
        || path.starts_with(PAY_PREFIX)
        || path == GRANT_PATH
        || path.starts_with("/gnap/")
        || path == INCOMING_PREFIX
        || path.starts_with("/incoming-payments/")
}

/// Answers an Open Payments request.
pub async fn handle(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    let mut response = if request.method() == Method::OPTIONS {
        preflight()
    } else if !node.open_payments().allow_request(peer.ip(), node.now_ms()) {
        error(&OpError::new(429, "too_many_requests", "slow down"), None)
    } else {
        match Urls::of(node).filter(|_| node.is_set_up()) {
            None => error(&OpError::not_found("Open Payments is not set up"), None),
            Some(urls) => route(node, peer, &urls, request).await,
        }
    };
    cors(response.headers_mut());
    response
}

async fn route(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    request: Request<Incoming>,
) -> Response<Body> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let wallet_path = path.strip_suffix("/jwks.json");
    match (&method, path.as_str()) {
        (&Method::GET | &Method::HEAD, _) if wallet_path.is_some() => {
            wallet::jwks(node, wallet_path.unwrap_or_default()).await
        }
        (&Method::GET | &Method::HEAD, path)
            if path == WELL_KNOWN_PATH || path.starts_with(PAY_PREFIX) =>
        {
            wallet::wallet_address(node, urls, path).await
        }
        (&Method::POST, GRANT_PATH) => grants::request(node, peer, urls, request).await,
        (&Method::POST | &Method::DELETE, path) if path.starts_with("/gnap/") => {
            grants::manage(node, urls, &method, path, request).await
        }
        (_, path) if path == INCOMING_PREFIX || path.starts_with("/incoming-payments/") => {
            incoming::route(node, peer, urls, request).await
        }
        _ => error(&OpError::not_found("Not Found"), None),
    }
}

/// A request read whole: what signatures cover, and the body.
pub struct ReadRequest {
    method: String,
    target_uri: String,
    path: String,
    query: String,
    fields: Vec<(String, String)>,
    body: Bytes,
}

impl ReadRequest {
    /// Reads the head and a body of at most [`MAX_BODY_BYTES`] in time.
    /// The URL signatures must cover is the node's own, whatever `Host`
    /// says, so a request signed for another server never verifies here.
    async fn read(node: &Node, urls: &Urls, request: Request<Incoming>) -> Result<Self, OpError> {
        let path_and_query = request.uri().path_and_query().map_or("/", |pq| pq.as_str());
        let target_uri = format!("{}{path_and_query}", urls.origin());
        let fields = request
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        let method = request.method().as_str().to_string();
        let path = request.uri().path().to_string();
        let query = request.uri().query().unwrap_or_default().to_string();
        let has_body = request.method() == Method::POST;
        let is_json = header_str(request.headers(), &header::CONTENT_TYPE)
            .and_then(|value| value.split(';').next())
            .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"));
        let timeout = Duration::from_millis(node.policies().body_timeout_ms);
        let collect = Limited::new(request.into_body(), MAX_BODY_BYTES).collect();
        let body = match tokio::time::timeout(timeout, collect).await {
            Ok(Ok(collected)) => collected.to_bytes(),
            Ok(Err(_)) => {
                return Err(OpError::new(
                    413,
                    "invalid_request",
                    "the body is too large",
                ));
            }
            Err(_) => {
                return Err(OpError::new(
                    408,
                    "invalid_request",
                    "the body took too long",
                ));
            }
        };
        if has_body && !body.is_empty() && !is_json {
            return Err(OpError::new(
                415,
                "invalid_request",
                "expected application/json",
            ));
        }
        Ok(Self {
            method,
            target_uri,
            path,
            query,
            fields,
            body,
        })
    }

    /// The fields as signatures take them.
    fn field_refs(&self) -> Vec<(&str, &str)> {
        self.fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }

    /// The request as signatures see it, with `fields` from
    /// [`field_refs`](Self::field_refs).
    fn signed<'a>(&'a self, fields: &'a [(&'a str, &'a str)]) -> SignedRequest<'a> {
        SignedRequest {
            method: &self.method,
            target_uri: &self.target_uri,
            fields,
            body: &self.body,
        }
    }

    /// Whether the client sent a token.
    fn has_authorization(&self) -> bool {
        self.fields
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    }
}

/// A JSON response.
fn json(status: StatusCode, value: &impl Serialize) -> Response<Body> {
    match serde_json::to_vec(value) {
        Ok(body) => response::with_body(status, "application/json", body),
        Err(failure) => error(&OpError::internal(failure), None),
    }
}

/// An error in the specs' form; a 401 says where to get a grant.
fn error(failure: &OpError, urls: Option<&Urls>) -> Response<Body> {
    let body = dassie_payments::resources::ErrorBody::new(failure.code, &failure.description);
    let status = StatusCode::from_u16(failure.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = response::with_body(
        status,
        "application/json",
        serde_json::to_vec(&body).unwrap_or_default(),
    );
    if status == StatusCode::UNAUTHORIZED
        && let Some(urls) = urls
    {
        let value = dassie_payments::gnap::www_authenticate(&urls.auth_server());
        if let Ok(value) = HeaderValue::from_str(&value) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
    }
    response
}

fn preflight() -> Response<Body> {
    let mut response = response::with_body(StatusCode::NO_CONTENT, "text/plain", "");
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, POST, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOWED_HEADERS),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    response
}

fn cors(headers: &mut hyper::HeaderMap) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("WWW-Authenticate"),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn knows_its_paths() {
        for path in [
            "/.well-known/pay",
            "/.well-known/pay/jwks.json",
            "/pay/stub",
            "/gnap",
            "/gnap/continue/x",
            "/incoming-payments",
            "/incoming-payments/x/complete",
        ] {
            assert!(is_open_payments_path(path), "{path}");
        }
        for path in [
            "/",
            "/payments",
            "/gnapx",
            "/incoming-paymentsx",
            "/api/login",
        ] {
            assert!(!is_open_payments_path(path), "{path}");
        }
    }
}
