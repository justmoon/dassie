//! The minisign signature of a release's `SHA256SUMS`.
//!
//! A minisign public key is `Ed`, an 8-byte key ID and the Ed25519 key, in
//! base64. A signature file has four lines:
//!
//! ```text
//! untrusted comment: <anything>
//! base64(ED ‖ key ID ‖ Ed25519 signature of BLAKE2b-512(file))
//! trusted comment: dassie <version>
//! base64(Ed25519 signature of (the signature above ‖ the trusted comment))
//! ```
//!
//! The release workflow signs with `minisign -S -t "dassie <version>"`. The
//! trusted comment binds the sums to their version: the signed sums of an
//! old release, served under a new version's path, are refused. Legacy
//! signatures (`Ed`, of the file itself rather than its hash) are refused
//! too, since nothing here makes them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest as _};
use ed25519_dalek::{Signature, VerifyingKey};

use crate::{Sums, SumsError, Version};

/// The key's algorithm: Ed25519.
const KEY_ALGORITHM: [u8; 2] = *b"Ed";
/// A signature of the file's BLAKE2b-512, which minisign makes by default.
const PREHASHED: [u8; 2] = *b"ED";
/// The bytes of a key ID.
const KEY_ID: usize = 8;
/// The largest signature file read.
const MAX_SIGNATURE: usize = 1024;
/// What each comment line starts with.
const UNTRUSTED: &str = "untrusted comment: ";
const TRUSTED: &str = "trusted comment: ";

/// A minisign public key: the release key, compiled into the binary and
/// printed in the install script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    id: [u8; KEY_ID],
    key: VerifyingKey,
}

/// Why a key or a signature was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// Not what minisign writes.
    Encoding,
    /// Not Ed25519, or a legacy signature of the file itself.
    Algorithm,
    /// Made with another key.
    KeyId,
    /// The signature does not verify.
    Invalid,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Encoding => "it is not a minisign key or signature",
            Self::Algorithm => "it uses an algorithm releases are not signed with",
            Self::KeyId => "it was made with another key than the release key",
            Self::Invalid => "the signature does not match",
        })
    }
}

/// Why a release's sums were refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The signature is not the release key's, or not of these sums.
    Signature(SignatureError),
    /// The signature is good, but it signs the sums of another version.
    Version {
        /// The trusted comment the signature carries.
        signed: String,
    },
    /// The sums are signed, but malformed.
    Sums(SumsError),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature(error) => write!(f, "SHA256SUMS.minisig was refused: {error}"),
            Self::Version { signed } => {
                write!(f, "SHA256SUMS is signed for another version ({signed:?})")
            }
            Self::Sums(error) => error.fmt(f),
        }
    }
}

impl PublicKey {
    /// Reads a public key as `minisign -G` writes it (`minisign.pub`), or
    /// just the base64 line of it.
    pub fn parse(text: &str) -> Result<Self, SignatureError> {
        let mut lines = text.lines();
        let first = lines.next().ok_or(SignatureError::Encoding)?;
        let encoded = match first.strip_prefix(UNTRUSTED) {
            Some(_) => lines.next().ok_or(SignatureError::Encoding)?,
            None => first,
        };
        if lines.any(|line| !line.is_empty()) {
            return Err(SignatureError::Encoding);
        }
        let bytes = decode::<{ 2 + KEY_ID + 32 }>(encoded)?;
        let (algorithm, rest) = bytes
            .split_first_chunk::<2>()
            .ok_or(SignatureError::Encoding)?;
        let (id, key) = rest
            .split_first_chunk::<KEY_ID>()
            .ok_or(SignatureError::Encoding)?;
        let key: &[u8; 32] = key.try_into().map_err(|_| SignatureError::Encoding)?;
        if *algorithm != KEY_ALGORITHM {
            return Err(SignatureError::Algorithm);
        }
        let key = VerifyingKey::from_bytes(key).map_err(|_| SignatureError::Encoding)?;
        if key.is_weak() {
            return Err(SignatureError::Encoding);
        }
        Ok(Self { id: *id, key })
    }

    /// The key's ID as minisign prints it: 16 uppercase hex digits.
    pub fn id(&self) -> String {
        self.id
            .iter()
            .rev()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }
}

/// The trusted comment a release's signature carries.
pub fn trusted_comment(version: &Version) -> String {
    format!("dassie {version}")
}

/// Checks that `signature` is the release key's signature of `sums` for
/// this version, then reads the sums.
pub fn verify_sums(
    key: &PublicKey,
    sums: &[u8],
    signature: &str,
    version: &Version,
) -> Result<Sums, VerifyError> {
    let signed = verify(key, sums, signature).map_err(VerifyError::Signature)?;
    if signed != trusted_comment(version) {
        return Err(VerifyError::Version { signed });
    }
    Sums::parse(sums).map_err(VerifyError::Sums)
}

/// Verifies both signatures, and returns the trusted comment.
fn verify(key: &PublicKey, file: &[u8], signature: &str) -> Result<String, SignatureError> {
    if signature.len() > MAX_SIGNATURE {
        return Err(SignatureError::Encoding);
    }
    let lines: Vec<&str> = signature.lines().collect();
    let [untrusted, encoded, trusted, global] = lines.as_slice() else {
        return Err(SignatureError::Encoding);
    };
    let comment = trusted
        .strip_prefix(TRUSTED)
        .ok_or(SignatureError::Encoding)?;
    if !untrusted.starts_with(UNTRUSTED) {
        return Err(SignatureError::Encoding);
    }
    let bytes = decode::<{ 2 + KEY_ID + 64 }>(encoded)?;
    let (algorithm, rest) = bytes
        .split_first_chunk::<2>()
        .ok_or(SignatureError::Encoding)?;
    let (id, signed) = rest
        .split_first_chunk::<KEY_ID>()
        .ok_or(SignatureError::Encoding)?;
    let signed: &[u8; 64] = signed.try_into().map_err(|_| SignatureError::Encoding)?;
    if *algorithm != PREHASHED {
        return Err(SignatureError::Algorithm);
    }
    if *id != key.id {
        return Err(SignatureError::KeyId);
    }
    let hash = Blake2b512::digest(file);
    key.key
        .verify_strict(hash.as_slice(), &Signature::from_bytes(signed))
        .map_err(|_| SignatureError::Invalid)?;
    let global = decode::<64>(global)?;
    let mut message = Vec::with_capacity(signed.len().saturating_add(comment.len()));
    message.extend_from_slice(signed);
    message.extend_from_slice(comment.as_bytes());
    key.key
        .verify_strict(&message, &Signature::from_bytes(&global))
        .map_err(|_| SignatureError::Invalid)?;
    Ok(comment.into())
}

/// Exactly `N` bytes, in standard base64.
fn decode<const N: usize>(encoded: &str) -> Result<[u8; N], SignatureError> {
    let bytes = STANDARD
        .decode(encoded.trim_end_matches('\r'))
        .map_err(|_| SignatureError::Encoding)?;
    bytes.try_into().map_err(|_| SignatureError::Encoding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use proptest::prelude::*;

    const PUBLIC_KEY: &str = include_str!("../../../testdata/release/test.pub");
    const SUMS: &str = include_str!("../../../testdata/release/SHA256SUMS");
    const SIGNATURE: &str = include_str!("../../../testdata/release/SHA256SUMS.minisig");
    const LEGACY: &str = include_str!("../../../testdata/release/SHA256SUMS.legacy.minisig");

    fn key() -> PublicKey {
        PublicKey::parse(PUBLIC_KEY).unwrap_or_else(|e| panic!("the test key: {e}"))
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn verifies_what_minisign_signs() {
        let sums = verify_sums(&key(), SUMS.as_bytes(), SIGNATURE, &version("0.1.0"));
        assert_eq!(sums.map(|sums| sums.names().count()), Ok(4));
        assert_eq!(key().id(), "91CCFD418EEBE0EA");
    }

    #[test]
    fn reads_the_key_alone_too() {
        let Some(line) = PUBLIC_KEY.lines().nth(1) else {
            panic!("minisign.pub has two lines");
        };
        assert_eq!(PublicKey::parse(line), Ok(key()));
        // "ED" for "Ed": the algorithm of a signature, not of a key.
        assert_eq!(
            PublicKey::parse(&PUBLIC_KEY.replace("RWT", "RUT")),
            Err(SignatureError::Algorithm)
        );
        assert_eq!(
            PublicKey::parse(&PUBLIC_KEY.replace("RWT", "RW")),
            Err(SignatureError::Encoding)
        );
        assert_eq!(PublicKey::parse(""), Err(SignatureError::Encoding));
        assert_eq!(
            PublicKey::parse(&[PUBLIC_KEY, "more\n"].concat()),
            Err(SignatureError::Encoding)
        );
    }

    #[test]
    fn refuses_tampered_sums() {
        let tampered = SUMS.replacen('4', "5", 1);
        assert_eq!(
            verify_sums(&key(), tampered.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
        let longer = [SUMS, "0"].concat();
        assert_eq!(
            verify_sums(&key(), longer.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_the_sums_of_another_version() {
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), SIGNATURE, &version("0.2.0")),
            Err(VerifyError::Version {
                signed: "dassie 0.1.0".into()
            })
        );
        // A trusted comment changed to match: the global signature fails.
        let relabelled = SIGNATURE.replace("dassie 0.1.0", "dassie 0.2.0");
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), &relabelled, &version("0.2.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_other_keys_and_legacy_signatures() {
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), LEGACY, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Algorithm))
        );
        let mut other = key();
        other.id = [0; KEY_ID];
        assert_eq!(
            verify_sums(&other, SUMS.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::KeyId))
        );
        // The same ID with another key: the signature does not verify.
        let mut impostor = key();
        impostor.key = VerifyingKey::from_bytes(&[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ])
        .unwrap_or_else(|e| panic!("RFC 8032's first key: {e}"));
        assert_eq!(
            verify_sums(&impostor, SUMS.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_malformed_signatures() {
        let lines: Vec<&str> = SIGNATURE.lines().collect();
        let cases = [
            String::new(),
            lines.iter().take(3).map(|l| [l, "\n"].concat()).collect(),
            [SIGNATURE, "extra\n"].concat(),
            SIGNATURE.replace("untrusted comment:", "comment:"),
            SIGNATURE.replace("trusted comment: dassie", "comment: dassie"),
            SIGNATURE.replace("RUT", "RUTT"),
            "x".repeat(MAX_SIGNATURE + 1),
        ];
        for signature in cases {
            assert_eq!(
                verify_sums(&key(), SUMS.as_bytes(), &signature, &version("0.1.0")),
                Err(VerifyError::Signature(SignatureError::Encoding)),
                "{signature:?}"
            );
        }
    }

    #[test]
    fn signs_for_the_version() {
        assert_eq!(
            trusted_comment(&version("canary-2026-10-15-15bd7aa")),
            "dassie canary-2026-10-15-15bd7aa"
        );
        assert_eq!(
            VerifyError::Version { signed: "x".into() }.to_string(),
            "SHA256SUMS is signed for another version (\"x\")"
        );
    }

    proptest! {
        #[test]
        fn never_panics(key_text in "\\PC{0,120}", signature in "\\PC{0,400}", sums in "\\PC{0,200}") {
            let _ = PublicKey::parse(&key_text);
            let _ = verify_sums(&key(), sums.as_bytes(), &signature, &version("0.1.0"));
        }

        /// Any change to a signed byte is caught.
        #[test]
        fn any_flipped_bit_is_refused(position in 0..SUMS.len(), bit in 0u8..8) {
            let mut sums = SUMS.as_bytes().to_vec();
            if let Some(byte) = sums.get_mut(position) {
                *byte ^= 1 << bit;
            }
            prop_assert!(verify_sums(&key(), &sums, SIGNATURE, &version("0.1.0")).is_err());
        }
    }
}
