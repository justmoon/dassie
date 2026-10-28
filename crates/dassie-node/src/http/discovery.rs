//! `GET /.well-known/dassie/node` (spec/areas/http.md#the-nodes-discovery-document):
//! who the node is and where it answers requests for its snapshot, for
//! anyone.
//!
//! It needs no authentication and is not signed: whoever fetches it over
//! HTTPS already trusts the node's name by its certificate, and the ID is
//! the key's hash, which a reader checks. It names only what the node
//! holds without asking its engine, so it costs no more than a static file.

use hyper::StatusCode;
use hyper::header::{self, HeaderValue};
use serde::Serialize;

use super::response::{self, Body};
use crate::node::Node;

/// Where it is served.
pub const PATH: &str = "/.well-known/dassie/node";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    /// The node ID: the hash of `public_key` and `slh_dsa_public_key`.
    id: String,
    /// The node key's public half, in base64url.
    public_key: String,
    /// The node's SLH-DSA public key, in base64url
    /// (spec/areas/identity.md#node-id).
    slh_dsa_public_key: String,
    /// The UDP port it answers requests for its snapshot on, at the host it
    /// was fetched from, while it answers: a core node that offers swaps
    /// and has the socket (spec/areas/rate-feed.md#finding-a-node).
    #[serde(skip_serializing_if = "Option::is_none")]
    feed_port: Option<u16>,
}

/// A public key in base64url.
fn encode(key: &[u8; 32]) -> String {
    let mut text = [0u8; dassie_base64url::encoded_len(32)];
    let length = dassie_base64url::encode(key, &mut text);
    String::from_utf8_lossy(text.get(..length).unwrap_or_default()).into_owned()
}

/// Answers it: 404 before setup, when the node has no identity yet.
pub fn serve(node: &Node) -> hyper::Response<Body> {
    let Some(key) = node.node_key() else {
        return response::not_found();
    };
    let identity = key.identity();
    let document = Discovery {
        id: identity.node_id().to_string(),
        public_key: encode(identity.key().as_bytes()),
        slh_dsa_public_key: encode(identity.slh_dsa().as_bytes()),
        feed_port: node
            .own_info()
            .checked()
            .feed_port
            .filter(|_| crate::feed::serving(node)),
    };
    let body = serde_json::to_string(&document).unwrap_or_default();
    let mut response = response::with_body(StatusCode::OK, "application/json", body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
