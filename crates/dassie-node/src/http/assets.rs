//! The wallet, embedded in the binary at build time by
//! `dassie-wallet-assets`, which the CLI and the desktop app link and hand
//! the daemon ([`Wallet`]), so that a change to the wallet compiles that
//! crate again rather than the node.
//!
//! The original read files from disk on every request, knew five MIME types
//! (so fonts, SVG and source maps came back as HTML), sent no caching
//! headers, compressed nothing and answered `HEAD` with 404. Here:
//!
//! - every file has its type, and an ETag from its content;
//! - Vite's hashed files under `/assets/` are cached for a year, and
//!   `index.html` is revalidated each time, so a new build shows at once;
//! - files that shrink are served gzipped to clients that accept it;
//! - other paths get `index.html`, for the wallet's own routing, except
//!   under `/assets/`, where a missing file is a 404.
//!
//! In development (`DASSIE_DEV_WALLET_DIR`), the wallet is read from a
//! directory that `vite build --watch` writes, under the same CSP, so a
//! change shows after a reload ([`serve_dir`]).

use std::path::{Component, Path, PathBuf};

use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Response, StatusCode};
use sha2::{Digest, Sha256};

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

/// The wallet a node serves: its files, sorted by path, and whether they
/// are a built wallet rather than the placeholder.
#[derive(Clone, Copy)]
pub struct Wallet {
    assets: &'static [Asset],
    built: bool,
}

impl Wallet {
    /// A page saying the binary has no wallet, for a binary built without
    /// one, so that `cargo build` and `cargo test` need no Node.js.
    pub const PLACEHOLDER: Self = Self {
        assets: &[Asset {
            path: "/index.html",
            content_type: "text/html; charset=utf-8",
            etag: concat!("\"placeholder-", env!("CARGO_PKG_VERSION"), "\""),
            body: b"<!doctype html>\n<meta charset=\"utf-8\">\n<title>Dassie</title>\n\
                <p>This Dassie binary was built without its wallet. Build the wallet \
                (<code>node --run build:wallet</code>) and rebuild with \
                <code>DASSIE_WALLET_DIST</code> pointing at it.</p>\n",
            gzip: None,
        }],
        built: false,
    };

    /// A built wallet's files, sorted by path.
    #[must_use]
    pub const fn built(assets: &'static [Asset]) -> Self {
        Self {
            assets,
            built: true,
        }
    }

    /// Whether this is a built wallet rather than the placeholder.
    #[must_use]
    pub const fn is_built(self) -> bool {
        self.built
    }

    fn find(self, path: &str) -> Option<&'static Asset> {
        self.assets.iter().find(|asset| asset.path == path)
    }
}

impl Default for Wallet {
    fn default() -> Self {
        Self::PLACEHOLDER
    }
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("files", &self.assets.len())
            .field("built", &self.built)
            .finish()
    }
}

/// The directory of Vite's content-hashed files.
const HASHED_PREFIX: &str = "/assets/";

/// Answers `GET` or `HEAD` for `path` from `wallet`.
pub fn serve(wallet: Wallet, method: &Method, path: &str, headers: &HeaderMap) -> Response<Body> {
    let asset = match wallet.find(path) {
        Some(asset) => asset,
        None if path.starts_with(HASHED_PREFIX) => return response::not_found(),
        None => match wallet.find("/index.html") {
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

/// The largest file served from a directory.
const MAX_DEV_FILE_BYTES: u64 = 64 << 20;

/// Answers `GET` or `HEAD` for `path` from the directory `root`, which a
/// development build of the wallet keeps writing: nothing is cached, and
/// nothing outside `root` is ever read.
pub async fn serve_dir(
    root: &Path,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let file = match resolve(root, path).await {
        Some(file) => Some(file),
        None if path.starts_with(HASHED_PREFIX) => return response::not_found(),
        None => resolve(root, "/index.html").await,
    };
    let Some(file) = file else {
        return response::text(
            StatusCode::SERVICE_UNAVAILABLE,
            "The wallet is being built; reload in a moment.",
        );
    };
    let body = match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.len() <= MAX_DEV_FILE_BYTES => tokio::fs::read(&file).await.ok(),
        _ => None,
    };
    let Some(body) = body else {
        return response::text(StatusCode::INTERNAL_SERVER_ERROR, "unreadable file");
    };
    let extension = file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let (content_type, _) = super::mime::content_type(extension);
    let digest = Sha256::digest(&body);
    let etag = format!("\"{}\"", hex(digest.get(..16).unwrap_or_default()));
    let not_modified = header_values(headers, &header::IF_NONE_MATCH)
        .flat_map(|value| value.split(','))
        .any(|tag| tag.trim() == etag);
    let mut response = if not_modified {
        let mut response = response::with_body(StatusCode::NOT_MODIFIED, content_type, "");
        response.headers_mut().remove(header::CONTENT_TYPE);
        response
    } else {
        response::with_body(StatusCode::OK, content_type, body)
    };
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if method == Method::HEAD {
        *response.body_mut() = Body::default();
    }
    response
}

/// The file `path` names under `root`, if it is a file there: only plain
/// segments of safe characters, and after symlinks are followed, still
/// under `root` (which is itself canonical).
async fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let relative = path.strip_prefix('/')?;
    if relative.is_empty() {
        return None;
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/');
    if !relative.chars().all(safe) {
        return None;
    }
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let file = tokio::fs::canonicalize(root.join(relative)).await.ok()?;
    let meta = tokio::fs::metadata(&file).await.ok()?;
    (file.starts_with(root) && meta.is_file()).then_some(file)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

    #[test]
    fn index_with_head_caching_and_etag() {
        let response = serve(
            Wallet::PLACEHOLDER,
            &Method::HEAD,
            "/some/route",
            &headers(&[]),
        );
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
            serve(Wallet::PLACEHOLDER, &Method::GET, "/", &conditional).status(),
            StatusCode::NOT_MODIFIED
        );
    }

    // DASSIE_DEV_WALLET_DIR: files under the root only, the index for
    // other paths, and nothing cached.
    #[tokio::test]
    async fn a_directory_serves_only_what_is_in_it() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "no").unwrap();
        let root = temp.path().join("dist");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        std::fs::write(root.join("index.html"), "<p>wallet</p>").unwrap();
        std::fs::write(root.join("assets/app.js"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
        let get = |path: &'static str| {
            let root = root.clone();
            async move { serve_dir(&root, &Method::GET, path, &headers(&[])).await }
        };
        let script = get("/assets/app.js").await;
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(
            script.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(script.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(get("/assets/gone.js").await.status(), StatusCode::NOT_FOUND);
        for path in [
            "/escape/secret.txt",
            "/../dist/index.html",
            "/%2e%2e/x",
            "//x",
        ] {
            let response = get(path).await;
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/html; charset=utf-8",
                "{path} gets the index"
            );
        }
        assert_eq!(get("/network/d1").await.status(), StatusCode::OK);
        std::fs::remove_file(root.join("index.html")).unwrap();
        assert_eq!(get("/").await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn missing_hashed_files_are_not_the_index() {
        assert_eq!(
            serve(
                Wallet::PLACEHOLDER,
                &Method::GET,
                "/assets/missing.js",
                &headers(&[])
            )
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}
