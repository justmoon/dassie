//! The node's own Open Payments key, and the keys of its clients.
//!
//! The node signs its requests to other wallets with an Ed25519 key derived
//! from the node key at `node/open-payments-client`, so the same passphrase
//! gives the same key, and publishes it at each wallet address's
//! `jwks.json`. Clients' keys come from their wallet addresses' `jwks.json`,
//! fetched as the node's Open Payments client fetches anything, checked at
//! the default front door (spec/areas/payments.md#signed-responses), and
//! kept a few minutes.

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_identity::{NodeKey, SeedPath};
use dassie_payments::jwk::{Jwk, Jwks};
use dassie_payments::pointer;
use dassie_payments::{SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};

use super::client::{self, ClientError};
use crate::node::Node;
use crate::outbound::WebRequest;

/// The seed path of the node's Open Payments key.
pub const CLIENT_KEY_PATH: &str = "node/open-payments-client";

/// Most clients' key sets kept.
const MAX_CACHED_KEY_SETS: usize = 256;
/// How long a fetched key set is used.
const KEY_SET_TTL_MS: u64 = 5 * 60 * 1_000;
/// The longest key set read.
const MAX_KEY_SET_BYTES: usize = 64 * 1024;
/// How long fetching one may take.
const FETCH_TIMEOUT_MS: u64 = 10_000;

/// The node's Open Payments key.
pub struct ClientKey {
    signing: SigningKey,
    kid: String,
}

impl ClientKey {
    /// Derives the key from the node key.
    ///
    /// # Errors
    ///
    /// The derivation failed, which only a bug can cause.
    pub fn derive(node_key: &NodeKey) -> Result<Self, String> {
        let path = SeedPath::parse(CLIENT_KEY_PATH).map_err(|e| e.to_string())?;
        let seed = node_key.derive(&path).map_err(|e| e.to_string())?;
        let signing = SigningKey::from_bytes(seed.as_bytes());
        let digest = Sha256::digest(signing.verifying_key().as_bytes());
        let kid = format!(
            "dassie-{}",
            digest
                .iter()
                .take(8)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        Ok(Self { signing, kid })
    }

    /// Its key ID.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The key to sign with.
    #[must_use]
    pub const fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    /// The key set every wallet address of the node publishes.
    #[must_use]
    pub fn jwks(&self) -> Jwks {
        Jwks {
            keys: vec![Jwk::ed25519(&self.kid, &self.signing.verifying_key())],
        }
    }
}

/// Why a client's key could not be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The wallet address is not one a client may have.
    InvalidAddress,
    /// Its key set could not be fetched or read.
    Unavailable(String),
    /// It has no usable key with that ID.
    UnknownKey,
}

/// Clients' key sets, by wallet address, with when they were fetched.
#[derive(Default)]
pub struct KeyCache {
    sets: BTreeMap<String, (u64, Arc<Jwks>)>,
}

impl KeyCache {
    fn get(&self, address: &str, now_ms: u64) -> Option<Arc<Jwks>> {
        self.sets
            .get(address)
            .filter(|(at, _)| now_ms.saturating_sub(*at) < KEY_SET_TTL_MS)
            .map(|(_, set)| Arc::clone(set))
    }

    /// Keeps a set, making room by dropping the oldest.
    fn put(&mut self, address: String, now_ms: u64, set: Arc<Jwks>) {
        if self.sets.len() >= MAX_CACHED_KEY_SETS && !self.sets.contains_key(&address) {
            let oldest = self
                .sets
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.sets.remove(&oldest);
            }
        }
        self.sets.insert(address, (now_ms, set));
    }
}

/// The key `kid` of the client at `wallet_address`, from the cache or
/// fetched.
///
/// # Errors
///
/// See [`KeyError`].
pub async fn client_key(
    node: &Node,
    wallet_address: &str,
    kid: &str,
    now_ms: u64,
) -> Result<(Jwk, VerifyingKey), KeyError> {
    pointer::check_wallet_address(wallet_address).map_err(|_| KeyError::InvalidAddress)?;
    let cache = &node.open_payments().keys;
    let cached = crate::node::lock(cache).get(wallet_address, now_ms);
    let set = match cached {
        Some(set) => set,
        None => {
            let set = Arc::new(fetch_key_set(node, wallet_address).await?);
            crate::node::lock(cache).put(wallet_address.to_string(), now_ms, Arc::clone(&set));
            set
        }
    };
    let key = set.find(kid).ok_or(KeyError::UnknownKey)?;
    let jwk = set
        .keys
        .iter()
        .find(|jwk| jwk.kid == kid)
        .cloned()
        .ok_or(KeyError::UnknownKey)?;
    Ok((jwk, key))
}

/// Fetches the key set of the client at `wallet_address`. At the default
/// front door it is taken only from the node of the address's host, so a
/// front door cannot vouch for a key of its own as another node's.
async fn fetch_key_set(node: &Node, wallet_address: &str) -> Result<Jwks, KeyError> {
    let url = format!("{}/jwks.json", wallet_address.trim_end_matches('/'));
    let request = WebRequest {
        method: http::Method::GET,
        url,
        headers: vec![(http::header::ACCEPT, "application/json".into())],
        body: None,
        timeout_ms: FETCH_TIMEOUT_MS,
        max_response_bytes: MAX_KEY_SET_BYTES,
    };
    let response = client::exchange(node, request)
        .await
        .map_err(|error: ClientError| KeyError::Unavailable(error.to_string()))?;
    if response.status != 200 {
        return Err(KeyError::Unavailable(format!(
            "its key set answered {}",
            response.status
        )));
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| KeyError::Unavailable("its key set is not a JWKS".into()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_key_follows_the_node_key() {
        let node_key = NodeKey::from_bytes(&[5; 32]);
        let key = ClientKey::derive(&node_key).unwrap();
        let again = ClientKey::derive(&node_key).unwrap();
        assert_eq!(key.kid(), again.kid());
        assert!(key.kid().starts_with("dassie-") && key.kid().len() == 23);
        let jwks = key.jwks();
        assert_eq!(
            jwks.find(key.kid()),
            Some(key.signing_key().verifying_key())
        );
    }

    #[test]
    fn the_cache_keeps_what_is_fresh_and_has_room() {
        let mut cache = KeyCache::default();
        let set = Arc::new(Jwks { keys: Vec::new() });
        cache.put("https://a.example/".into(), 0, Arc::clone(&set));
        assert!(
            cache
                .get("https://a.example/", KEY_SET_TTL_MS - 1)
                .is_some()
        );
        assert!(cache.get("https://a.example/", KEY_SET_TTL_MS).is_none());
        for index in 0..MAX_CACHED_KEY_SETS + 5 {
            let at = u64::try_from(index).unwrap() + 1;
            cache.put(format!("https://{index}.example/"), at, Arc::clone(&set));
        }
        assert_eq!(cache.sets.len(), MAX_CACHED_KEY_SETS);
        assert!(cache.get("https://a.example/", 2).is_none());
    }
}
