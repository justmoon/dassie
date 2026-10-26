//! JavaScript bindings (wasm-bindgen) for the wallet and the dev tools.
//!
//! Byte-oriented on purpose: keys and secrets cross the boundary as
//! `Uint8Array`s and phrases as strings. This is the edge where randomness
//! comes from the platform (`crypto.getRandomValues`), so the core stays
//! deterministic (spec/decisions/tigerstyle.md).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use rand_core::UnwrapErr;
use wasm_bindgen::prelude::*;

use dassie_identity::{Error, Mnemonic, NodeId, NodeKey, WordCount};

fn js(error: Error) -> JsError {
    JsError::new(&error.to_string())
}

fn node_key_from(bytes: &[u8]) -> Result<NodeKey, JsError> {
    let seed: &[u8; 32] = bytes
        .try_into()
        .map_err(|_| JsError::new("a node key has 32 bytes"))?;
    Ok(NodeKey::from_bytes(seed))
}

/// A new 12-word phrase from the platform's random number generator.
#[wasm_bindgen(js_name = generateMnemonic)]
#[must_use]
// The bindings are the edge where the browser supplies randomness.
#[allow(clippy::disallowed_types)]
pub fn generate_mnemonic() -> String {
    let mut rng = UnwrapErr(getrandom::SysRng);
    let mnemonic = Mnemonic::generate(&mut rng, WordCount::Twelve);
    String::from(mnemonic.phrase().as_str())
}

/// Whether `phrase` is a valid BIP-39 phrase: known words and a matching
/// checksum.
#[wasm_bindgen(js_name = validateMnemonic)]
#[must_use]
pub fn validate_mnemonic(phrase: &str) -> bool {
    Mnemonic::parse(phrase).is_ok()
}

/// The phrase of `entropy` (16 to 32 bytes, in steps of 4): how the dev
/// environment turns its vanity table into node keys.
#[wasm_bindgen(js_name = mnemonicFromEntropy)]
pub fn mnemonic_from_entropy(entropy: &[u8]) -> Result<String, JsError> {
    let mnemonic = Mnemonic::from_entropy(entropy).map_err(js)?;
    Ok(String::from(mnemonic.phrase().as_str()))
}

/// The phrase in canonical form: lowercase, single spaces.
#[wasm_bindgen(js_name = normalizeMnemonic)]
pub fn normalize_mnemonic(phrase: &str) -> Result<String, JsError> {
    let mnemonic = Mnemonic::parse(phrase).map_err(js)?;
    Ok(String::from(mnemonic.phrase().as_str()))
}

/// The node key (Ed25519 seed) of a phrase.
#[wasm_bindgen(js_name = deriveNodeKey)]
pub fn derive_node_key(phrase: &str) -> Result<Vec<u8>, JsError> {
    let seed = Mnemonic::parse(phrase).map_err(js)?.to_seed();
    Ok(seed.node_key().as_bytes().to_vec())
}

/// The login secret of a node key: what setup and login send as proof.
#[wasm_bindgen(js_name = loginSecret)]
pub fn login_secret(node_key: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.login_secret().as_bytes().to_vec())
}

/// The development session token of a phrase (dev environment only).
#[wasm_bindgen(js_name = devSessionToken)]
pub fn dev_session_token(phrase: &str) -> Result<Vec<u8>, JsError> {
    let seed = Mnemonic::parse(phrase).map_err(js)?.to_seed();
    Ok(seed.dev_session_token().as_bytes().to_vec())
}

/// The Ed25519 public key of a node key.
#[wasm_bindgen(js_name = publicKey)]
pub fn public_key(node_key: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.public_key().as_bytes().to_vec())
}

/// The node ID of an Ed25519 public key.
#[wasm_bindgen(js_name = nodeId)]
pub fn node_id(public_key: &[u8]) -> Result<String, JsError> {
    let key: &[u8; 32] = public_key
        .try_into()
        .map_err(|_| JsError::new("a public key has 32 bytes"))?;
    Ok(String::from(NodeId::from_public_key(key).as_str()))
}

/// Signs `message` with a node key.
#[wasm_bindgen]
pub fn sign(node_key: &[u8], message: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.sign(message).as_bytes().to_vec())
}
