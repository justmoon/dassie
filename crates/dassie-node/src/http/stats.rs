//! `GET /stats` (spec/areas/http.md#get-stats): who the node is, and its
//! peers, for anyone.
//!
//! As in the original it needs no authentication: it shows the version,
//! the node and its peers. Only the peers the node advertises are listed,
//! which its signed link state publishes anyway. The answer is kept for a
//! few seconds, and made by one request at a time while the others wait
//! for it, so unauthenticated requests queue at most one call to the
//! engine.

use dassie_stream::base64url;
use hyper::StatusCode;
use hyper::header::{self, HeaderValue};
use serde::Serialize;

use super::response::{self, Body};
use crate::node::Node;

/// How long an answer is kept.
const CACHE_MS: u64 = 5_000;

/// The last answer, and when it was made. The lock is held while a new
/// answer is made.
#[derive(Default)]
pub struct StatsCache(tokio::sync::Mutex<Option<(u64, String)>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    version: &'static str,
    node: NodeStats,
    peers: Vec<PeerStats>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeStats {
    id: String,
    url: Option<String>,
    public_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerStats {
    node_id: String,
    url: Option<String>,
    public_key: Option<String>,
}

/// A public key in base64url, as the original's `/stats` had it.
fn encode(key: &[u8; 32]) -> String {
    let mut text = [0u8; base64url::encoded_len(32)];
    let length = base64url::encode(key, &mut text);
    String::from_utf8_lossy(text.get(..length).unwrap_or_default()).into_owned()
}

/// Answers `GET /stats`: 404 before setup, 503 while the engine is down.
pub async fn serve(node: &Node) -> hyper::Response<Body> {
    let Some(key) = node.node_key() else {
        return response::not_found();
    };
    let mut cache = node.stats_cache().0.lock().await;
    let now = node.now_ms();
    let cached = cache
        .clone()
        .filter(|(at, _)| now.saturating_sub(*at) < CACHE_MS);
    let body = match cached {
        Some((_, body)) => body,
        None => {
            let Some(engine) = node.engine() else {
                return response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
            };
            let Ok(network) = engine.network().await else {
                return response::text(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable");
            };
            let own = network.nodes.iter().find(|view| view.id == network.own);
            let stats = Stats {
                version: crate::VERSION,
                node: NodeStats {
                    id: key.node_id().to_string(),
                    url: own.and_then(|view| view.url.clone()),
                    public_key: encode(key.public_key().as_bytes()),
                },
                peers: network
                    .nodes
                    .iter()
                    .filter(|view| view.advertised)
                    .map(|view| PeerStats {
                        node_id: view.id.to_string(),
                        url: view.url.clone(),
                        public_key: view.public_key.map(|key| encode(key.as_bytes())),
                    })
                    .collect(),
            };
            let body = serde_json::to_string(&stats).unwrap_or_default();
            *cache = Some((now, body.clone()));
            body
        }
    };
    drop(cache);
    let mut response = response::with_body(StatusCode::OK, "application/json", body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
