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

use dassie_identity::{Created, Error, NodeId, NodeKey, Phrase, PhraseSecret, Positions, Realm};

fn js(error: Error) -> JsError {
    JsError::new(&error.to_string())
}

fn node_key_from(bytes: &[u8]) -> Result<NodeKey, JsError> {
    let seed: &[u8; 32] = bytes
        .try_into()
        .map_err(|_| JsError::new("a node key has 32 bytes"))?;
    Ok(NodeKey::from_bytes(seed))
}

#[wasm_bindgen]
extern "C" {
    /// The platform's clock, in milliseconds since the Unix epoch.
    #[wasm_bindgen(js_namespace = Date, js_name = now)]
    fn date_now() -> f64;
}

/// The realm named `test` or `live`, the test realm if none is named.
fn realm_of(name: Option<String>) -> Result<Realm, JsError> {
    match name {
        None => Ok(Realm::Test),
        Some(name) => Realm::parse(&name).ok_or_else(|| JsError::new("a realm is test or live")),
    }
}

/// The phrase `text` names in `realm`, if it reads as written: one that
/// would need correcting is refused, since these bindings have no way yet
/// to show a correction for the owner to confirm.
fn read_exact(text: &str, realm: Option<String>) -> Result<(Phrase, Realm), JsError> {
    let realm = realm_of(realm)?;
    let reading = Phrase::read(text, Positions::NONE, realm).map_err(js)?;
    Ok((reading.without_correction().map_err(js)?, realm))
}

/// A new phrase of the test realm from the platform's random number
/// generator, made now by the platform's clock.
///
/// # Errors
///
/// The clock is outside the hours a phrase can record.
#[wasm_bindgen(js_name = generateMnemonic)]
// The bindings are the edge where the browser supplies randomness and the
// time. The clock's milliseconds fit in a u64; a negative or NaN reading
// becomes 0, which is before any phrase and so refused.
#[allow(
    clippy::disallowed_types,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn generate_mnemonic() -> Result<String, JsError> {
    let mut rng = UnwrapErr(getrandom::SysRng);
    let now_ms = date_now() as u64;
    let phrase = Phrase::generate(&mut rng, Realm::Test, now_ms).map_err(js)?;
    Ok(String::from(phrase.text().as_str()))
}

/// Whether `phrase` is a phrase of `realm` (the test realm if none is
/// named) that reads as written.
#[wasm_bindgen(js_name = validateMnemonic)]
#[must_use]
pub fn validate_mnemonic(phrase: &str, realm: Option<String>) -> bool {
    read_exact(phrase, realm).is_ok()
}

/// The phrase of a 16-byte `secret` in `realm` (the test realm if none is
/// named), made `created` hours after 2026-01-01 (0 if not given): how the
/// dev environment turns its vanity table into node keys.
#[wasm_bindgen(js_name = mnemonicFromEntropy)]
pub fn mnemonic_from_entropy(
    secret: &[u8],
    realm: Option<String>,
    created: Option<u32>,
) -> Result<String, JsError> {
    let bytes: [u8; 16] = secret
        .try_into()
        .map_err(|_| JsError::new("a phrase's secret has 16 bytes"))?;
    let created = Created::from_hours(created.unwrap_or(0))
        .ok_or_else(|| JsError::new("the creation hour is outside the field"))?;
    let phrase = Phrase::new(realm_of(realm)?, created, &PhraseSecret::from_bytes(bytes));
    Ok(String::from(phrase.text().as_str()))
}

/// The phrase in canonical form: the words in full, lowercase, single
/// spaces.
#[wasm_bindgen(js_name = normalizeMnemonic)]
pub fn normalize_mnemonic(phrase: &str, realm: Option<String>) -> Result<String, JsError> {
    let (phrase, _) = read_exact(phrase, realm)?;
    Ok(String::from(phrase.text().as_str()))
}

/// The node key (Ed25519 seed) of a phrase of `realm` (the test realm if
/// none is named).
#[wasm_bindgen(js_name = deriveNodeKey)]
pub fn derive_node_key(phrase: &str, realm: Option<String>) -> Result<Vec<u8>, JsError> {
    let (phrase, realm) = read_exact(phrase, realm)?;
    let seed = phrase.master_seed(realm).map_err(js)?;
    Ok(seed.node_key().as_bytes().to_vec())
}

/// The login secret of a node key: what setup and login send as proof.
#[wasm_bindgen(js_name = loginSecret)]
pub fn login_secret(node_key: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.login_secret().as_bytes().to_vec())
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
