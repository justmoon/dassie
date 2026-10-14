//! Ed25519 keys as JSON Web Keys, the only kind Open Payments uses.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

/// The most keys a key set may have to be used.
pub const MAX_KEYS: usize = 16;
/// The longest key ID accepted, in bytes.
pub const MAX_KID_BYTES: usize = 256;

/// The key is not an Ed25519 public key in the Open Payments form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidKey;

/// A public key (`json-web-key` in the specs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwk {
    /// The key's ID, which signatures name in `keyid`.
    pub kid: String,
    /// Always `EdDSA`.
    pub alg: String,
    /// `sig` when present.
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub key_use: Option<String>,
    /// Always `OKP`.
    pub kty: String,
    /// Always `Ed25519`.
    pub crv: String,
    /// The public key, unpadded base64url.
    pub x: String,
}

impl Jwk {
    /// The JWK of an Ed25519 public key.
    #[must_use]
    pub fn ed25519(kid: &str, key: &VerifyingKey) -> Self {
        Self {
            kid: kid.to_string(),
            alg: "EdDSA".to_string(),
            key_use: Some("sig".to_string()),
            kty: "OKP".to_string(),
            crv: "Ed25519".to_string(),
            x: URL_SAFE_NO_PAD.encode(key.as_bytes()),
        }
    }

    /// The public key, after checking the key is an Ed25519 signing key
    /// and not a weak (small-order) point.
    ///
    /// # Errors
    ///
    /// [`InvalidKey`] otherwise.
    pub fn verifying_key(&self) -> Result<VerifyingKey, InvalidKey> {
        let shape = self.alg == "EdDSA"
            && self.kty == "OKP"
            && self.crv == "Ed25519"
            && self
                .key_use
                .as_deref()
                .is_none_or(|key_use| key_use == "sig")
            && !self.kid.is_empty()
            && self.kid.len() <= MAX_KID_BYTES;
        if !shape {
            return Err(InvalidKey);
        }
        let bytes = URL_SAFE_NO_PAD.decode(&self.x).map_err(|_| InvalidKey)?;
        let bytes = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| InvalidKey)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| InvalidKey)?;
        if key.is_weak() {
            return Err(InvalidKey);
        }
        Ok(key)
    }
}

/// A key set, as `/jwks.json` serves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwks {
    /// The keys.
    pub keys: Vec<Jwk>,
}

impl Jwks {
    /// The usable key with this ID. A set larger than [`MAX_KEYS`] is
    /// not searched.
    #[must_use]
    pub fn find(&self, kid: &str) -> Option<VerifyingKey> {
        if self.keys.len() > MAX_KEYS {
            return None;
        }
        self.keys
            .iter()
            .find(|key| key.kid == kid)
            .and_then(|key| key.verifying_key().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn a_key_survives_the_round_trip() {
        let key = SigningKey::from_bytes(&[3; 32]).verifying_key();
        let jwk = Jwk::ed25519("k1", &key);
        let json = serde_json::to_string(&Jwks {
            keys: alloc::vec![jwk],
        })
        .unwrap();
        let parsed: Jwks = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.find("k1"), Some(key));
        assert_eq!(parsed.find("k2"), None);
    }

    #[test]
    fn refuses_other_keys() {
        let key = SigningKey::from_bytes(&[3; 32]).verifying_key();
        let mut jwk = Jwk::ed25519("k1", &key);
        jwk.crv = "X25519".into();
        assert_eq!(jwk.verifying_key(), Err(InvalidKey));
        let mut short = Jwk::ed25519("k1", &key);
        short.x = "AAAA".into();
        assert_eq!(short.verifying_key(), Err(InvalidKey));
        // The identity point: small order, so one signature would verify
        // every message.
        let mut weak = Jwk::ed25519("k1", &key);
        let mut identity = [0u8; 32];
        identity[0] = 1;
        weak.x = URL_SAFE_NO_PAD.encode(identity);
        assert_eq!(weak.verifying_key(), Err(InvalidKey));
    }
}
