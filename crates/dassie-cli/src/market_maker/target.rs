//! Who the target is, from its `GET /stats` (spec/areas/http.md#get-stats):
//! its node ID, its public key, and its feed's port if it names one. The
//! ID must be the key's, so a feed that verifies under the key is the
//! target's.

use dassie_identity::{NodeId, PublicKey, base64url};
use dassie_node::peer_client::{PeerClient, WebRequest};
use serde_json::Value;

/// How long `/stats` may take.
const STATS_TIMEOUT_MS: u64 = 10_000;

/// Most bytes of `/stats` read: it lists the target's peers too.
const MAX_STATS_BYTES: usize = 256 * 1024;

/// The target, as its `/stats` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    /// Its node ID.
    pub node_id: NodeId,
    /// Its public key, of which the ID is the hash.
    pub public_key: [u8; 32],
    /// Its feed's UDP port, if it names one.
    pub feed_port: Option<u16>,
}

/// Fetches and reads the target's `/stats`.
///
/// # Errors
///
/// It cannot be fetched, or [`parse_stats`] refuses it.
pub async fn fetch(client: &PeerClient, url: &str) -> Result<TargetInfo, String> {
    let stats_url = format!("{url}/stats");
    let response = client
        .web(WebRequest {
            method: http::Method::GET,
            url: stats_url.clone(),
            headers: Vec::new(),
            body: None,
            timeout_ms: STATS_TIMEOUT_MS,
            max_response_bytes: MAX_STATS_BYTES,
        })
        .await
        .map_err(|error| format!("{stats_url}: {error}"))?;
    if response.status != 200 {
        return Err(format!("{stats_url} answered {}", response.status));
    }
    parse_stats(&response.body).map_err(|problem| format!("{stats_url}: {problem}"))
}

/// Reads `/stats`: the node's ID and key, the ID the key's and the key not
/// weak, and its feed's port, where the node or the answer names one.
///
/// # Errors
///
/// What is wrong with it.
pub fn parse_stats(body: &[u8]) -> Result<TargetInfo, String> {
    let value: Value = serde_json::from_slice(body).map_err(|_| "not JSON".to_owned())?;
    let node = value.get("node").ok_or("no node")?;
    let id = node
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| NodeId::parse(id).ok())
        .ok_or("no node ID")?;
    let encoded = node
        .get("publicKey")
        .and_then(Value::as_str)
        .ok_or("no public key")?;
    let mut public_key = [0u8; 32];
    let length = base64url::decode(encoded.as_bytes(), &mut public_key)
        .map_err(|_| "a public key that is not base64url")?;
    if length != public_key.len() {
        return Err("a public key that is not 32 bytes".into());
    }
    let key = PublicKey::from_bytes(&public_key).map_err(|_| "a public key that is no point")?;
    if key.is_weak() {
        return Err("a weak public key".into());
    }
    if key.node_id() != id {
        return Err("a node ID that is not its key's".into());
    }
    let port = node.get("feedPort").or_else(|| value.get("feedPort"));
    let feed_port = match port {
        None | Some(Value::Null) => None,
        Some(port) => Some(
            port.as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or("a feed port that is not one")?,
        ),
    };
    Ok(TargetInfo {
        node_id: id,
        public_key,
        feed_port,
    })
}

#[cfg(test)]
mod tests {
    use dassie_identity::NodeKey;
    use serde_json::json;

    use super::*;

    fn encode(key: &[u8; 32]) -> String {
        let mut text = [0u8; 64];
        let length = base64url::encode(key, &mut text);
        String::from_utf8(text[..length].to_vec()).unwrap()
    }

    fn stats(id: &str, key: &[u8; 32], port: Option<Value>) -> Vec<u8> {
        let mut node = json!({ "id": id, "url": "https://c.example", "publicKey": encode(key) });
        if let Some(port) = port {
            node["feedPort"] = port;
        }
        json!({ "version": "0.2.3", "node": node, "peers": [] })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn the_id_must_be_the_keys() {
        let key = NodeKey::from_bytes(&[1; 32]).public_key();
        let id = key.node_id();
        let info = parse_stats(&stats(id.as_str(), key.as_bytes(), Some(json!(7768)))).unwrap();
        assert_eq!(info.node_id, id);
        assert_eq!(&info.public_key, key.as_bytes());
        assert_eq!(info.feed_port, Some(7768));
        let without = parse_stats(&stats(id.as_str(), key.as_bytes(), None)).unwrap();
        assert_eq!(without.feed_port, None);

        let other = NodeKey::from_bytes(&[2; 32]).public_key().node_id();
        assert!(parse_stats(&stats(other.as_str(), key.as_bytes(), None)).is_err());
        assert!(parse_stats(&stats(id.as_str(), key.as_bytes(), Some(json!(0)))).is_err());
        assert!(parse_stats(&stats(id.as_str(), key.as_bytes(), Some(json!("x")))).is_err());
        assert!(parse_stats(b"{}").is_err());
        assert!(parse_stats(b"not json").is_err());
        // A weak key (the identity point) is refused, whatever its ID.
        let mut weak = [0u8; 32];
        weak[0] = 1;
        let weak_id = NodeId::from_public_key(&weak);
        assert!(parse_stats(&stats(weak_id.as_str(), &weak, None)).is_err());
    }
}
