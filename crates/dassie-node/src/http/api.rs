//! `POST /api/setup`, `/api/login` and `/api/logout`.
//!
//! The bodies, the cookie and the routes' presence (setup only before, the
//! others only after) are the original's (spec/areas/http.md#setup,
//! spec/areas/http.md#login-and-logout). New: the request must come from the
//! node's own pages (`Origin`), carry JSON as JSON (`Content-Type`,
//! spec/areas/http.md#security-model), stay under 16 KiB, and not arrive too
//! often.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use dassie_kit::ClientKey;
use dassie_node_core::config::Realm;
use dassie_node_core::origin;
use dassie_node_core::session;
use dassie_node_core::setup::{SetupError, SetupRequest};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Request, Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::response::{self, Body, header_str, header_values};
use crate::log::target;
use crate::node::{AuthError, NewSession, Node};

/// Largest body accepted, in bytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    setup_authorization_token: String,
    raw_dassie_key_hex: String,
    login_authorization_signature: String,
    /// `test` or `live`; `test` if absent. Fixed for good once set up.
    #[serde(default)]
    realm: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginBody {
    login_authorization_signature: String,
}

/// A request refused before it reached its handler.
#[derive(Debug)]
pub struct Refusal(pub StatusCode, pub String);

impl Refusal {
    /// The plain-text response.
    pub fn into_response(self) -> Response<Body> {
        response::text(self.0, self.1)
    }
}

/// Refuses a request unless it comes from the node's own pages.
pub fn check_origin(node: &Node, request: &Request<Incoming>) -> Result<(), Refusal> {
    let headers = request.headers();
    let config = node.config();
    origin::check_same_origin(
        header_str(headers, &header::ORIGIN),
        header_str(headers, &header::HOST),
        config.hostname.as_ref(),
    )
    .map_err(|error| Refusal(StatusCode::FORBIDDEN, error.to_string()))
}

/// Reads a JSON body of at most [`MAX_BODY_BYTES`], sent within `timeout`.
async fn json_body<T: DeserializeOwned>(
    request: Request<Incoming>,
    timeout: Duration,
) -> Result<T, Refusal> {
    if !is_json(request.headers()) {
        return Err(Refusal(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Expected application/json".into(),
        ));
    }
    // Headers have hyper's timeout; the body gets one here, or a client
    // could hold its connection by sending a byte now and then.
    let collect = Limited::new(request.into_body(), MAX_BODY_BYTES).collect();
    let bytes: Bytes = match tokio::time::timeout(timeout, collect).await {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(_)) => {
            return Err(Refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Payload Too Large".into(),
            ));
        }
        Err(_) => {
            return Err(Refusal(
                StatusCode::REQUEST_TIMEOUT,
                "Request Timeout".into(),
            ));
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|_| Refusal(StatusCode::BAD_REQUEST, "Invalid HTTP request body".into()))
}

/// Whether the body's media type is JSON, whatever its parameters.
fn is_json(headers: &HeaderMap) -> bool {
    header_str(headers, &header::CONTENT_TYPE)
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

fn body_timeout(node: &Node) -> Duration {
    Duration::from_millis(node.policies().body_timeout_ms)
}

fn user_agent(request: &Request<Incoming>) -> String {
    header_str(request.headers(), &header::USER_AGENT)
        .unwrap_or_default()
        .to_string()
}

/// `POST /api/setup`.
pub async fn setup(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    if let Err(refusal) = check_origin(node, &request) {
        return refusal.into_response();
    }
    let user_agent = user_agent(&request);
    let body: SetupBody = match json_body(request, body_timeout(node)).await {
        Ok(body) => body,
        Err(refusal) => return refusal.into_response(),
    };
    let realm = match body.realm.as_deref().map(Realm::parse).transpose() {
        Ok(realm) => realm.unwrap_or_default(),
        Err(error) => return response::text(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let setup = SetupRequest {
        setup_authorization_token: &body.setup_authorization_token,
        raw_dassie_key_hex: &body.raw_dassie_key_hex,
        login_authorization_signature: &body.login_authorization_signature,
    };
    signed_in(node.setup(&setup, realm, ClientKey::from_ip(peer.ip()), &user_agent))
}

/// `POST /api/login`.
pub async fn login(node: &Node, peer: SocketAddr, request: Request<Incoming>) -> Response<Body> {
    if let Err(refusal) = check_origin(node, &request) {
        return refusal.into_response();
    }
    let user_agent = user_agent(&request);
    let body: LoginBody = match json_body(request, body_timeout(node)).await {
        Ok(body) => body,
        Err(refusal) => return refusal.into_response(),
    };
    signed_in(node.login(
        &body.login_authorization_signature,
        ClientKey::from_ip(peer.ip()),
        &user_agent,
    ))
}

fn signed_in(result: Result<NewSession, AuthError>) -> Response<Body> {
    match result {
        Ok(session) => {
            let mut response = response::empty_json();
            if let Ok(cookie) = HeaderValue::from_str(&session.cookie) {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(AuthError::Setup(SetupError::MalformedKey)) => response::text(
            StatusCode::BAD_REQUEST,
            SetupError::MalformedKey.to_string(),
        ),
        Err(AuthError::Realm(error)) => response::text(StatusCode::BAD_REQUEST, error.to_string()),
        Err(AuthError::RateLimited(retry_after_ms)) => {
            let mut response = response::text(
                StatusCode::TOO_MANY_REQUESTS,
                AuthError::RateLimited(0).to_string(),
            );
            let seconds = retry_after_ms.div_ceil(1_000).max(1);
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
        Err(AuthError::Store(error)) => {
            tracing::error!(target: target::AUTH, %error, "database failure during login");
            response::text(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
        }
        Err(error) => response::text(StatusCode::UNAUTHORIZED, error.to_string()),
    }
}

/// `POST /api/logout`: ends the cookie's session, if any, and clears it.
pub fn logout(node: &Node, request: &Request<Incoming>) -> Response<Body> {
    if let Err(refusal) = check_origin(node, request) {
        return refusal.into_response();
    }
    let token = session::find_session_cookie(header_values(request.headers(), &header::COOKIE));
    if let Err(error) = node.logout(token.as_ref()) {
        tracing::error!(target: target::AUTH, %error, "database failure during logout");
        return response::text(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
    }
    let mut response = response::empty_json();
    if let Ok(cookie) = HeaderValue::from_str(&session::clear_cookie()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

#[cfg(test)]
mod tests {
    use hyper::header::HeaderName;
    use proptest::prelude::*;

    use super::super::response::tests::headers;
    use super::*;

    const NAMES: &[HeaderName] = &[header::CONTENT_TYPE];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        // Anyone may post to the setup and login endpoints: their media
        // type is read, or refused, without a panic
        // (spec/decisions/tigerstyle.md).
        #[test]
        fn any_media_type_is_read(headers in headers(NAMES)) {
            if is_json(&headers) {
                let value = header_str(&headers, &header::CONTENT_TYPE).unwrap_or_default();
                prop_assert!(value.to_ascii_lowercase().trim_start().starts_with("application/json"));
            }
        }
    }
}
