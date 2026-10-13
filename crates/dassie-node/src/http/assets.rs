//! The wallet, embedded in the binary at build time.
//!
//! The original read files from disk on every request, knew five MIME types
//! (so fonts, SVG and source maps came back as HTML), sent no caching
//! headers, compressed nothing and answered `HEAD` with 404 (RT-15). Here:
//!
//! - every file has its type, and an ETag from its content;
//! - Vite's hashed files under `/assets/` are cached for a year, and
//!   `index.html` is revalidated each time, so a new build shows at once;
//! - files that shrink are served gzipped to clients that accept it;
//! - other paths get `index.html`, for the wallet's own routing, except
//!   under `/assets/`, where a missing file is a 404.

use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Response, StatusCode};

use super::response::{self, Body, header_values};

/// A file of the wallet.
pub struct Asset {
    /// Its URL path.
    pub path: &'static str,
    /// Its media type.
    pub content_type: &'static str,
    /// A quoted hash of its content.
    pub etag: &'static str,
    /// The file.
    pub body: &'static [u8],
    /// The file gzipped, if that is smaller.
    pub gzip: Option<&'static [u8]>,
}

include!(concat!(env!("OUT_DIR"), "/wallet_assets.rs"));

/// The directory of Vite's content-hashed files.
const HASHED_PREFIX: &str = "/assets/";

/// Whether the binary carries a built wallet, rather than the placeholder.
#[must_use]
pub const fn wallet_built() -> bool {
    WALLET_BUILT
}

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|asset| asset.path == path)
}

/// Answers `GET` or `HEAD` for `path`.
pub fn serve(method: &Method, path: &str, headers: &HeaderMap) -> Response<Body> {
    let asset = match find(path) {
        Some(asset) => asset,
        None if path.starts_with(HASHED_PREFIX) => return response::not_found(),
        None => match find("/index.html") {
            Some(index) => index,
            None => return response::not_found(),
        },
    };
    let cache = if asset.path.starts_with(HASHED_PREFIX) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let not_modified = header_values(headers, &header::IF_NONE_MATCH)
        .flat_map(|value| value.split(','))
        .any(|tag| tag.trim() == asset.etag || tag.trim() == "*");
    let gzip = asset.gzip.filter(|_| accepts_gzip(headers));
    let mut response = if not_modified {
        let mut response = response::with_body(StatusCode::NOT_MODIFIED, asset.content_type, "");
        response.headers_mut().remove(header::CONTENT_TYPE);
        response
    } else {
        let body = gzip.unwrap_or(asset.body);
        let mut response = response::with_body(StatusCode::OK, asset.content_type, body);
        if gzip.is_some() {
            response
                .headers_mut()
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        }
        response
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::from_static(asset.etag));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if method == Method::HEAD {
        // hyper sends no body for HEAD; the headers still describe it.
        *response.body_mut() = Body::default();
    }
    response
}

/// Whether `Accept-Encoding` allows gzip: listed, and not with `q=0`.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    header_values(headers, &header::ACCEPT_ENCODING)
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            // A weight of zero (`0`, `0.0`, `0.000`) refuses the coding.
            let refused = parts.any(|parameter| {
                parameter
                    .strip_prefix("q=")
                    .is_some_and(|q| q.starts_with('0') && q.chars().all(|c| c == '0' || c == '.'))
            });
            name.eq_ignore_ascii_case("gzip") && !refused
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn gzip_negotiation() {
        assert!(accepts_gzip(&headers(&[(
            "accept-encoding",
            "gzip, deflate, br"
        )])));
        assert!(accepts_gzip(&headers(&[(
            "accept-encoding",
            "br;q=1.0, GZIP;q=0.5"
        )])));
        assert!(!accepts_gzip(&headers(&[("accept-encoding", "gzip;q=0")])));
        assert!(!accepts_gzip(&headers(&[(
            "accept-encoding",
            "gzip;q=0.000"
        )])));
        assert!(!accepts_gzip(&headers(&[("accept-encoding", "identity")])));
        assert!(!accepts_gzip(&headers(&[])));
    }

    // RT-15: the original answered HEAD with 404 and sent no cache headers.
    #[test]
    fn index_with_head_caching_and_etag() {
        let response = serve(&Method::HEAD, "/some/route", &headers(&[]));
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        assert_eq!(
            serve(&Method::GET, "/", &conditional).status(),
            StatusCode::NOT_MODIFIED
        );
    }

    #[test]
    fn missing_hashed_files_are_not_the_index() {
        assert_eq!(
            serve(&Method::GET, "/assets/missing.js", &headers(&[])).status(),
            StatusCode::NOT_FOUND
        );
    }
}
