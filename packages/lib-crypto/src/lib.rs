mod utils;

use wasm_bindgen::prelude::*;
use ed25519_dalek::{SecretKey, PublicKey, Keypair, Signature, Signer};

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
pub fn init() {
    utils::set_panic_hook();
}

#[wasm_bindgen]
pub fn sign(seed: &[u8], message: &[u8]) -> Result<Box<[u8]>, JsValue> {
    let secret = SecretKey::from_bytes(seed)
        .map_err(|_| JsValue::from_str("invalid seed length"))?;
    let public = PublicKey::from(&secret);
    let keypair: Keypair = Keypair { secret, public };
    let signature: Signature = keypair.sign(message);
    Ok(Box::new(signature.to_bytes()))
}
