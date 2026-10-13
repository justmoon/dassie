//! Building responses.

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Response, StatusCode};

/// Every response's body type.
pub type Body = Full<Bytes>;

/// The page's Content Security Policy: nothing inline, nothing from other
/// origins; WebAssembly may compile, for the identity module (spec 7 §5).
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
     style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; \
     base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// HSTS for a year, once a CA-issued certificate is installed.
pub const STRICT_TRANSPORT_SECURITY: &str = "max-age=31536000";

/// A response with `status`, a body and its type.
pub fn with_body(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Bytes>,
) -> Response<Body> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

/// A plain-text response, as the original's failures were.
pub fn text(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    with_body(status, "text/plain; charset=utf-8", message.into())
}

/// `200 {}`, the original's success body for the auth API.
pub fn empty_json() -> Response<Body> {
    with_body(StatusCode::OK, "application/json", "{}")
}

/// `404 Not Found`.
pub fn not_found() -> Response<Body> {
    text(StatusCode::NOT_FOUND, "Not Found")
}

/// Adds the headers every HTTPS response carries.
pub fn secure(headers: &mut HeaderMap, hsts: bool) {
    let pairs = [
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (
            header::HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ),
    ];
    for (name, value) in pairs {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if hsts {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(STRICT_TRANSPORT_SECURITY),
        );
    }
}

/// The header's values as text, skipping any that are not.
pub fn header_values<'a>(
    headers: &'a HeaderMap,
    name: &header::HeaderName,
) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
}

/// The header's first value as text.
pub fn header_str<'a>(headers: &'a HeaderMap, name: &header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Whether a comma-separated header holds `token`, ignoring case.
pub fn header_has_token(headers: &HeaderMap, name: &header::HeaderName, token: &str) -> bool {
    header_values(headers, name)
        .flat_map(|value| value.split(','))
        .any(|item| item.trim().eq_ignore_ascii_case(token))
}
