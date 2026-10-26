//! Claiming the node and proving ownership.
//!
//! - **Setup** happens once. The node prints a URL with a random token, and
//!   whoever opens it in a browser sends the node key derived from their
//!   phrase, with the login proof derived from that key
//!   (spec/areas/http.md#setup).
//! - **Login** proves knowledge of the phrase by sending the login proof
//!   again (spec/areas/http.md#login-and-logout).
//!
//! Kept from the original on purpose: the one-time token, the request
//! bodies and the derivations. Fixed (RT-10): the key must be
//! exactly 32 bytes of hex, where the original stored whatever it got and
//! then failed to parse it at every start; and every comparison runs in
//! constant time. Changed: the original kept the token in memory, so every
//! restart voided the URL the owner had been given; the node now stores it
//! until setup is done.

use alloc::string::String;
use core::fmt;

use dassie_identity::NodeKey;
use rand_core::CryptoRng;
use subtle::ConstantTimeEq;

use crate::hex;

/// Bytes of randomness in a setup token.
pub const SETUP_TOKEN_BYTES: usize = 32;

/// The token in the setup URL.
///
/// It is drawn the first time the daemon starts without a node key, and
/// stored until setup is done, so the setup URL survives restarts.
#[derive(Clone)]
pub struct SetupToken([u8; SETUP_TOKEN_BYTES]);

impl SetupToken {
    /// Draws a new token.
    pub fn generate(rng: &mut (impl CryptoRng + ?Sized)) -> Self {
        let mut bytes = [0; SETUP_TOKEN_BYTES];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The token stored as `bytes`.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SETUP_TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    /// The token's bytes, to store.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SETUP_TOKEN_BYTES] {
        &self.0
    }

    /// The token as it appears in the setup URL: 64 lowercase hex digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// Whether `candidate` is this token, compared in constant time.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        hex::decode_exact::<SETUP_TOKEN_BYTES>(candidate)
            .is_some_and(|bytes| bool::from(bytes.ct_eq(&self.0)))
    }
}

impl fmt::Debug for SetupToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SetupToken(<redacted>)")
    }
}

/// The body of `POST /api/setup`, with the original's field names.
#[derive(Clone, Copy, Debug)]
pub struct SetupRequest<'a> {
    /// The token from the setup URL.
    pub setup_authorization_token: &'a str,
    /// The node key, 32 bytes as hex.
    pub raw_dassie_key_hex: &'a str,
    /// The login proof for that key, 32 bytes as hex.
    pub login_authorization_signature: &'a str,
}

/// Why setup was refused. The messages are the original's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    /// The token is not this process's setup token.
    InvalidToken,
    /// The key is not 32 bytes of hex.
    MalformedKey,
    /// The proof does not match the key.
    InvalidProof,
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidToken => "Invalid setup authorization token",
            Self::MalformedKey => "Invalid node key",
            Self::InvalidProof => "Invalid login authorization signature",
        })
    }
}

/// Checks a setup request and returns the node key it carries.
pub fn verify_setup(token: &SetupToken, request: &SetupRequest<'_>) -> Result<NodeKey, SetupError> {
    if !token.matches(request.setup_authorization_token) {
        return Err(SetupError::InvalidToken);
    }
    let seed =
        hex::decode_exact::<32>(request.raw_dassie_key_hex).ok_or(SetupError::MalformedKey)?;
    let key = NodeKey::from_bytes(&seed);
    if verify_login(&key, request.login_authorization_signature) {
        Ok(key)
    } else {
        Err(SetupError::InvalidProof)
    }
}

/// Whether `proof_hex` is the login proof of `key`, compared in constant
/// time. Anything that is not 32 bytes of hex is wrong.
#[must_use]
pub fn verify_login(key: &NodeKey, proof_hex: &str) -> bool {
    hex::decode_exact::<32>(proof_hex).is_some_and(|proof| key.login_secret().verify_proof(&proof))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::near_hex;
    use dassie_kit::sim::SeededRng;
    use proptest::prelude::*;

    fn request<'a>(token: &'a str, key: &'a str, proof: &'a str) -> SetupRequest<'a> {
        SetupRequest {
            setup_authorization_token: token,
            raw_dassie_key_hex: key,
            login_authorization_signature: proof,
        }
    }

    fn key_and_proof() -> (String, String) {
        let key = NodeKey::from_bytes(&[7; 32]);
        (
            hex::encode(key.as_bytes()),
            hex::encode(key.login_secret().as_bytes()),
        )
    }

    #[test]
    fn accepts_a_matching_request() {
        let token = SetupToken::generate(&mut SeededRng::new(1));
        let (key, proof) = key_and_proof();
        let result = verify_setup(&token, &request(&token.to_hex(), &key, &proof));
        assert_eq!(result.map(|key| *key.as_bytes()), Ok([7; 32]));
    }

    #[test]
    fn refuses_a_wrong_token_or_proof() {
        let token = SetupToken::generate(&mut SeededRng::new(1));
        let other = SetupToken::generate(&mut SeededRng::new(2));
        let (key, proof) = key_and_proof();
        let wrong_token = verify_setup(&token, &request(&other.to_hex(), &key, &proof));
        assert_eq!(wrong_token.err(), Some(SetupError::InvalidToken));
        let wrong_proof = verify_setup(&token, &request(&token.to_hex(), &key, &key));
        assert_eq!(wrong_proof.err(), Some(SetupError::InvalidProof));
    }

    // RT-10: the original stored a key of any length, which then broke every
    // start of the node.
    #[test]
    fn refuses_a_malformed_key() {
        let token = SetupToken::generate(&mut SeededRng::new(1));
        let (key, proof) = key_and_proof();
        for bad in [&key[2..], "", &key.replace('0', "x")] {
            let result = verify_setup(&token, &request(&token.to_hex(), bad, &proof));
            assert_eq!(result.err(), Some(SetupError::MalformedKey), "{bad}");
        }
    }

    #[test]
    fn login_proofs() {
        let key = NodeKey::from_bytes(&[7; 32]);
        let proof = hex::encode(key.login_secret().as_bytes());
        assert!(verify_login(&key, &proof));
        assert!(verify_login(&key, &proof.to_ascii_uppercase()));
        assert!(!verify_login(&key, &proof[2..]));
        assert!(!verify_login(&key, &hex::encode(&[0; 32])));
    }

    /// `bytes` as hex, each digit in upper case where `upper` says so.
    fn mixed_case(bytes: &[u8], upper: &[bool]) -> String {
        hex::encode(bytes)
            .chars()
            .zip(upper.iter().cycle())
            .map(|(digit, &upper)| {
                if upper {
                    digit.to_ascii_uppercase()
                } else {
                    digit
                }
            })
            .collect()
    }

    proptest! {
        // The token matches its own hex in any case, and nothing else.
        #[test]
        fn token_matching_never_panics(
            bytes: [u8; 32],
            candidate in prop_oneof![any::<String>(), near_hex(32)],
            upper in proptest::collection::vec(any::<bool>(), 1..64),
        ) {
            let token = SetupToken::from_bytes(bytes);
            let expected = hex::decode_exact::<32>(&candidate) == Some(bytes);
            prop_assert_eq!(token.matches(&candidate), expected);
            prop_assert!(token.matches(&mixed_case(&bytes, &upper)));
        }

        // Setup checks the token, then the key, then the proof, and gives
        // back the key it was sent.
        #[test]
        fn setup_never_panics(
            token_bytes: [u8; 32],
            key_bytes: [u8; 32],
            fields in proptest::array::uniform3(prop_oneof![any::<String>(), near_hex(32)]),
            valid: [bool; 3],
            upper in proptest::collection::vec(any::<bool>(), 1..64),
        ) {
            let token = SetupToken::from_bytes(token_bytes);
            let key = NodeKey::from_bytes(&key_bytes);
            let [token_hex, key_hex, proof_hex] = fields;
            let token_hex = if valid[0] { mixed_case(&token_bytes, &upper) } else { token_hex };
            let key_hex = if valid[1] { mixed_case(&key_bytes, &upper) } else { key_hex };
            let proof_hex = if valid[2] { mixed_case(key.login_secret().as_bytes(), &upper) } else { proof_hex };
            let result = verify_setup(&token, &request(&token_hex, &key_hex, &proof_hex));
            let seed = hex::decode_exact::<32>(&key_hex);
            let expected = if !token.matches(&token_hex) {
                Err(SetupError::InvalidToken)
            } else if let Some(seed) = seed {
                if verify_login(&NodeKey::from_bytes(&seed), &proof_hex) { Ok(seed) } else { Err(SetupError::InvalidProof) }
            } else {
                Err(SetupError::MalformedKey)
            };
            prop_assert_eq!(result.map(|key| *key.as_bytes()), expected);
            if valid == [true; 3] {
                prop_assert_eq!(expected, Ok(key_bytes));
            }
        }

        // A login passes with the key's proof in any case, and nothing else.
        #[test]
        fn login_never_panics(
            key_bytes: [u8; 32],
            proof in prop_oneof![any::<String>(), near_hex(32)],
            upper in proptest::collection::vec(any::<bool>(), 1..64),
        ) {
            let key = NodeKey::from_bytes(&key_bytes);
            let secret = *key.login_secret().as_bytes();
            let expected = hex::decode_exact::<32>(&proof) == Some(secret);
            prop_assert_eq!(verify_login(&key, &proof), expected);
            prop_assert!(verify_login(&key, &mixed_case(&secret, &upper)));
        }
    }
}
