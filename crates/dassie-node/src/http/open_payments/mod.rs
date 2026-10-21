//! Open Payments over HTTPS (spec/decisions/open-payments.md,
//! spec/areas/payments.md#http-routes): the wallet addresses, the
//! authorization server and the resource server.
//!
//! Unlike the owner's endpoints, these are for anyone: they check no
//! `Origin` and allow every origin (CORS, without credentials), since a
//! client proves itself by its GNAP token and its signature, never by a
//! cookie. Every request is rate-limited per address, bodies are bounded,
//! and errors take the specs' form.
//!
//! Each request is answered under one of the node's names
//! ([`Names`]), and each answer is signed with the node key, so that a
//! Dassie-aware client can check it came from the node
//! (spec/areas/payments.md#signed-responses).

mod grants;
mod incoming;
mod spending;
mod wallet;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;

use dassie_identity::base64url;
use dassie_payments::httpsig;

use super::response::{self, Body, header_str};
use crate::node::Node;
use crate::open_payments::auth::{OpError, SignedRequest};
use crate::open_payments::{GRANT_PATH, INCOMING_PREFIX, Names, PAY_PREFIX, Urls, WELL_KNOWN_PATH};

/// The largest request body, in bytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The headers a cross-origin client may send.
const ALLOWED_HEADERS: &str = "Accept, Authorization, Content-Type, Content-Digest, Content-Length, Signature, Signature-Input, Dassie-Nonce";

/// The headers a cross-origin client may read: where to get a grant, and
/// the node's signature.
const EXPOSED_HEADERS: &str = "WWW-Authenticate, Content-Digest, Signature, Signature-Input";

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
        || is_spending_path(path)
}

/// Whether `path` is one of quotes', outgoing payments' or a grant's spend.
fn is_spending_path(path: &str) -> bool {
    path == "/quotes"
        || path.starts_with("/quotes/")
        || path == "/outgoing-payments"
        || path.starts_with("/outgoing-payments/")
        || path == "/outgoing-payment-grant"
}

/// Answers an Open Payments request, under the name it came through,
/// and signs the answer.
pub async fn handle(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    let mut response = if request.method() == Method::OPTIONS {
        preflight()
    } else {
        match answering(node, &request) {
            Err(refused) => error(&refused, None),
            Ok(urls) => answer(node, peer, &urls, request).await,
        }
    };
    cors(response.headers_mut());
    response
}

/// Routes a request answered under `urls`, and signs the answer.
async fn answer(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    request: Request<Incoming>,
) -> Response<Body> {
    let answered = Answered::of(urls, &request);
    let response = if node.open_payments().allow_request(peer.ip(), node.now_ms()) {
        route(node, peer, urls, request).await
    } else {
        error(&OpError::new(429, "too_many_requests", "slow down"), None)
    };
    answered.sign(node, response).await
}

/// The URLs to answer `request` under: the node's hostname, or the name
/// the request came through, if it is one of the node's.
///
/// # Errors
///
/// 404 before the node is set up, and 421 for a name not on its list.
fn answering(node: &Node, request: &Request<Incoming>) -> Result<Urls, OpError> {
    let Some(names) = Names::of(node).filter(|_| node.is_set_up()) else {
        return Err(OpError::not_found("Open Payments is not set up"));
    };
    let host = header_str(request.headers(), &header::HOST).or_else(|| {
        request
            .uri()
            .authority()
            .map(|authority| authority.as_str())
    });
    names.answering(host).ok_or_else(|| {
        OpError::new(
            421,
            "misdirected_request",
            "this node does not answer to that name",
        )
    })
}

/// What the node's signature on its answer covers of the request
/// (spec/areas/payments.md#signed-responses).
struct Answered {
    method: Method,
    target_uri: String,
    nonces: Vec<String>,
}

impl Answered {
    fn of(urls: &Urls, request: &Request<Incoming>) -> Self {
        let path_and_query = request.uri().path_and_query().map_or("/", |pq| pq.as_str());
        Self {
            method: request.method().clone(),
            target_uri: format!("{}{path_and_query}", urls.origin()),
            nonces: response::header_values(request.headers(), &NONCE_HEADER)
                .map(str::to_string)
                .collect(),
        }
    }

    /// Adds the body's `Content-Digest` and the node's signature. A HEAD
    /// answer goes without its body, so its signature covers none.
    async fn sign(&self, node: &Node, response: Response<Body>) -> Response<Body> {
        let Some(key) = node.node_key() else {
            return response;
        };
        let (mut parts, body) = response.into_parts();
        let body = body.collect().await.map(|collected| collected.to_bytes());
        let body = body.unwrap_or_default();
        let has_body = !body.is_empty() && self.method != Method::HEAD;
        let digest = has_body.then(|| httpsig::content_digest(&body));
        let mut fields: Vec<(&str, &str)> = Vec::new();
        if let Some(digest) = &digest {
            fields.push(("content-digest", digest));
        }
        let nonces: Vec<(&str, &str)> = self
            .nonces
            .iter()
            .map(|nonce| (httpsig::NONCE_FIELD, nonce.as_str()))
            .collect();
        let request = httpsig::Request {
            method: self.method.as_str(),
            target_uri: &self.target_uri,
            fields: &nonces,
        };
        let answer = httpsig::Response {
            status: parts.status.as_u16(),
            fields: &fields,
        };
        let public = key.public_key();
        let mut keyid = [0u8; 43];
        let keyid_length = base64url::encode(public.as_bytes(), &mut keyid);
        let keyid = std::str::from_utf8(keyid.get(..keyid_length).unwrap_or_default());
        let signed = keyid.ok().and_then(|keyid| {
            let sign = |base: &[u8]| *key.sign(base).as_bytes();
            let created_s = node.now_ms() / 1_000;
            httpsig::sign_response(&request, &answer, has_body, sign, keyid, created_s).ok()
        });
        let headers = [
            (header::HeaderName::from_static("content-digest"), digest),
            (
                header::HeaderName::from_static("signature-input"),
                signed.as_ref().map(|signed| signed.signature_input.clone()),
            ),
            (
                header::HeaderName::from_static("signature"),
                signed.map(|signed| signed.signature),
            ),
        ];
        for (name, value) in headers {
            if let Some(value) = value.and_then(|value| HeaderValue::from_str(&value).ok()) {
                parts.headers.insert(name, value);
            }
        }
        Response::from_parts(parts, Body::new(body))
    }
}

/// The client's nonce, which the signature binds the answer to.
static NONCE_HEADER: header::HeaderName = header::HeaderName::from_static(httpsig::NONCE_FIELD);

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
        (&Method::GET, path) if path.starts_with("/gnap/interact/") => {
            grants::interact(node, urls, path)
        }
        (&Method::POST | &Method::DELETE, path) if path.starts_with("/gnap/") => {
            grants::manage(node, urls, &method, path, request).await
        }
        (_, path) if path == INCOMING_PREFIX || path.starts_with("/incoming-payments/") => {
            incoming::route(node, peer, urls, request).await
        }
        (_, path) if is_spending_path(path) => spending::route(node, peer, urls, request).await,
        _ => error(&OpError::not_found("Not Found"), None),
    }
}

/// A request read whole: what signatures cover, and the body.
pub struct ReadRequest {
    urls: Urls,
    method: String,
    target_uri: String,
    path: String,
    query: String,
    fields: Vec<(String, String)>,
    body: Bytes,
}

impl ReadRequest {
    /// Reads the head and a body of at most [`MAX_BODY_BYTES`] in time.
    /// The URL signatures must cover is at the name the request is
    /// answered under: the node's hostname, whatever `Host` says, or a name
    /// on its list. So a request signed for another server never verifies
    /// here.
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
            urls: urls.clone(),
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
        HeaderValue::from_static(EXPOSED_HEADERS),
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
