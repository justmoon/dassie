//! A validator's or a list publisher's manifest, as rippled serializes it:
//! its master key hands its signing to an ephemeral key, or revokes itself
//! (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers).
//!
//! Both keys sign the same bytes, `MAN\0` and its fields but the two
//! signatures: the master key into `MasterSignature`, the signing key into
//! `Signature`. A revocation names no signing key, and only its master
//! signs it. Either key may be Ed25519, which signs the bytes whole, or
//! secp256k1, which signs their SHA-512Half with a low S. Which manifest
//! of a master is current, the highest sequence, is the UNL's to judge.
//!
//! It is read in the canonical form only (`binary`), with the fields of
//! rippled's manifest format and no others, so it writes back to the bytes
//! it came in, which are the bytes its keys signed.

use alloc::string::String;
use alloc::vec::Vec;

use crate::binary::{self, FieldId, Fields, Type};
use crate::codec::DecodeError;
use crate::keys::{Key, PublicKey, Signature, sha512_half};

/// The longest manifest, as rippled bounds it.
pub const MAX_MANIFEST_BYTES: usize = 358;

/// The sequence of a revocation: the master key signs nothing more.
pub const REVOKED: u32 = u32::MAX;

/// The shortest domain a manifest names, and the longest.
const MIN_DOMAIN_BYTES: usize = 4;
const MAX_DOMAIN_BYTES: usize = 128;
/// The longest label of a domain, and the shortest top-level one.
const MAX_LABEL_BYTES: usize = 63;
const MIN_TOP_LABEL_BYTES: usize = 2;

/// What both signatures cover, and the hash, before the fields.
const MANIFEST_PREFIX: [u8; 4] = *b"MAN\0";

// Its fields, in canonical order. `Version` is one too, but only its
// default, which is never written, is understood.
const SEQUENCE: FieldId = FieldId::new(Type::UInt32, 4);
const PUBLIC_KEY: FieldId = FieldId::new(Type::Blob, 1);
const SIGNING_PUB_KEY: FieldId = FieldId::new(Type::Blob, 3);
const SIGNATURE: FieldId = FieldId::new(Type::Blob, 6);
const DOMAIN: FieldId = FieldId::new(Type::Blob, 7);
const MASTER_SIGNATURE: FieldId = FieldId::new(Type::Blob, 18);

/// A manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Its sequence: a higher one replaces a lower. [`REVOKED`] revokes the
    /// master key.
    pub sequence: u32,
    /// The master key, which a validator list names.
    pub master_key: PublicKey,
    /// The key it hands signing to, and that key's signature; none in a
    /// revocation.
    pub signing: Option<(PublicKey, Signature)>,
    /// The domain its owner names, well-formed.
    pub domain: Option<String>,
    /// The master key's signature.
    pub master_signature: Signature,
}

impl Manifest {
    /// The manifest in `bytes`, if it is one: at most
    /// [`MAX_MANIFEST_BYTES`], in canonical form, with every field rippled
    /// requires and none it does not know, keys of kinds the ledger knows,
    /// a well-formed domain, and a signing key with its signature exactly
    /// when it is not a revocation, other than the master key. Its
    /// signatures are not checked: see [`Self::verify`].
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(DecodeError::Value);
        }
        let mut sequence = None;
        let mut master_key = None;
        let mut signing_key = None;
        let mut signature = None;
        let mut domain = None;
        let mut master_signature = None;
        for field in Fields::new(bytes) {
            let field = field?;
            match field.id {
                SEQUENCE => sequence = Some(field.uint32()?),
                PUBLIC_KEY => master_key = Some(public_key(field.blob()?)?),
                SIGNING_PUB_KEY => signing_key = Some(public_key(field.blob()?)?),
                SIGNATURE => signature = Some(self::signature(field.blob()?)?),
                DOMAIN => domain = Some(self::domain(field.blob()?)?),
                MASTER_SIGNATURE => master_signature = Some(self::signature(field.blob()?)?),
                _ => return Err(DecodeError::Field),
            }
        }
        let sequence = sequence.ok_or(DecodeError::Field)?;
        let master_key = master_key.ok_or(DecodeError::Field)?;
        let signing = match (sequence == REVOKED, signing_key, signature) {
            (true, None, None) => None,
            (false, Some(key), Some(signature)) if key != master_key => Some((key, signature)),
            _ => return Err(DecodeError::Value),
        };
        Ok(Self {
            sequence,
            master_key,
            signing,
            domain,
            master_signature: master_signature.ok_or(DecodeError::Field)?,
        })
    }

    /// The manifest `master` signs, handing signing to `signing`, or
    /// revoking itself if there is none, which takes [`REVOKED`] as the
    /// sequence. A revocation with a signing key, or a manifest without
    /// one, a signing key that is the master key, or a domain that is not
    /// well-formed, is a bug.
    #[must_use]
    pub fn sign(
        sequence: u32,
        master: &dyn Key,
        signing: Option<&dyn Key>,
        domain: Option<String>,
    ) -> Self {
        assert_eq!(
            sequence == REVOKED,
            signing.is_none(),
            "a revocation alone has no signing key"
        );
        assert!(
            domain
                .as_ref()
                .is_none_or(|domain| self::domain(domain.as_bytes()).is_ok()),
            "a well-formed domain"
        );
        let master_key = master.public_key();
        let signing_key = signing.map(Key::public_key);
        assert!(
            signing_key != Some(master_key),
            "a signing key apart from the master key"
        );
        let mut message = MANIFEST_PREFIX.to_vec();
        let unsigned = Parts {
            sequence,
            master_key: &master_key,
            signing: signing_key.as_ref().map(|key| (key, None)),
            domain: domain.as_deref(),
            master_signature: None,
        };
        unsigned.write(&mut message);
        Self {
            sequence,
            master_key,
            signing: signing.map(|key| (key.public_key(), key.sign(&message))),
            domain,
            master_signature: master.sign(&message),
        }
    }

    /// Whether it revokes its master key.
    #[must_use]
    pub const fn is_revocation(&self) -> bool {
        self.sequence == REVOKED
    }

    /// Its bytes, as rippled writes them.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out, true);
        out
    }

    /// What both its signatures cover: `MAN\0` and its fields but the
    /// signatures.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = MANIFEST_PREFIX.to_vec();
        self.write(&mut out, false);
        out
    }

    /// Its hash, as rippled hashes a manifest: `MAN\0` and all of it.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        let mut message = MANIFEST_PREFIX.to_vec();
        self.write(&mut message, true);
        sha512_half(&message)
    }

    /// Whether its master key signed it, and its signing key too unless it
    /// is a revocation.
    #[must_use]
    pub fn verify(&self) -> bool {
        let message = self.signing_bytes();
        self.master_key.verify(&message, &self.master_signature)
            && self
                .signing
                .is_none_or(|(key, signature)| key.verify(&message, &signature))
    }

    /// Writes its fields, the signatures only if asked.
    fn write(&self, out: &mut Vec<u8>, with_signatures: bool) {
        let signature = |signature| with_signatures.then_some(signature);
        Parts {
            sequence: self.sequence,
            master_key: &self.master_key,
            signing: self
                .signing
                .as_ref()
                .map(|(key, key_signature)| (key, signature(key_signature))),
            domain: self.domain.as_deref(),
            master_signature: signature(&self.master_signature),
        }
        .write(out);
    }
}

/// A manifest's fields to be written, with or without its signatures.
struct Parts<'a> {
    sequence: u32,
    master_key: &'a PublicKey,
    signing: Option<(&'a PublicKey, Option<&'a Signature>)>,
    domain: Option<&'a str>,
    master_signature: Option<&'a Signature>,
}

impl Parts<'_> {
    /// Writes them in canonical order.
    fn write(&self, out: &mut Vec<u8>) {
        let start = out.len();
        binary::write_field(out, SEQUENCE, &self.sequence.to_be_bytes());
        binary::write_field(out, PUBLIC_KEY, self.master_key.as_bytes());
        if let Some((key, signature)) = self.signing {
            binary::write_field(out, SIGNING_PUB_KEY, key.as_bytes());
            if let Some(signature) = signature {
                binary::write_field(out, SIGNATURE, signature.as_bytes());
            }
        }
        if let Some(domain) = self.domain {
            binary::write_field(out, DOMAIN, domain.as_bytes());
        }
        if let Some(signature) = self.master_signature {
            binary::write_field(out, MASTER_SIGNATURE, signature.as_bytes());
        }
        assert!(
            binary::check(out.get(start..).unwrap_or_default()).is_ok(),
            "a manifest is written in canonical form"
        );
    }
}

fn public_key(bytes: &[u8]) -> Result<PublicKey, DecodeError> {
    PublicKey::from_bytes(bytes.try_into().map_err(|_| DecodeError::Value)?)
        .ok_or(DecodeError::Value)
}

fn signature(bytes: &[u8]) -> Result<Signature, DecodeError> {
    Signature::from_slice(bytes).ok_or(DecodeError::Value)
}

/// The domain in `bytes`, if it is well-formed as rippled has it: four to
/// 128 bytes of labels apart by dots, each of letters, digits and hyphens,
/// neither starting nor ending with a hyphen, at most 63 long, the last
/// two to 63 letters.
fn domain(bytes: &[u8]) -> Result<String, DecodeError> {
    if !(MIN_DOMAIN_BYTES..=MAX_DOMAIN_BYTES).contains(&bytes.len()) {
        return Err(DecodeError::Value);
    }
    let dot = bytes
        .iter()
        .rposition(|byte| *byte == b'.')
        .ok_or(DecodeError::Value)?;
    let (labels, top) = bytes.split_at(dot);
    let top = top.get(1..).unwrap_or_default();
    let top_fits = (MIN_TOP_LABEL_BYTES..=MAX_LABEL_BYTES).contains(&top.len())
        && top.iter().all(u8::is_ascii_alphabetic);
    let label_fits = |label: &[u8]| {
        (1..=MAX_LABEL_BYTES).contains(&label.len())
            && label
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            && label.first() != Some(&b'-')
            && label.last() != Some(&b'-')
    };
    if !top_fits || !labels.split(|byte| *byte == b'.').all(label_fits) {
        return Err(DecodeError::Value);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::Value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use alloc::format;

    use super::*;
    use crate::keys::{KeyType, Secp256k1Key, Wallet};
    use crate::{from_hex, hex_upper};

    // vl.ripple.com's publisher manifest (testdata of
    // `tests/golden_ledger.rs`).
    const PUBLISHER: &str = "24000000017121ED2677ABFFD1B33AC6FBC3062B71F1E8397C1505E1C42C64D11AD1B28FF73F47347321ED5D009C48B90F8A1D63D5F6B31F9C63C07738736326F32101144FB531E65A70217640C5720E77740412D560CC4032C8B36C30C07A5272B5278E9D62670C135D55D7B1A4712F3B72722A2DA3726022619DBA45EE6DBAE37AA08D534D9FED8FFEEB0D01701240DCCD04394FB7F61981F2FC1F81EAE75D9203435FA8D1D9EE5E35FBDE80CB82C69CC17CCB05539D069480B09C180FE9D6F1869269C38712C59A3E5A1FD3912C02";

    #[test]
    fn domains_are_well_formed_or_refused() {
        for good in ["xrp.vet", "ripple.com", "a-b.c-d.example.io", "x1.co"] {
            assert!(domain(good.as_bytes()).is_ok(), "{good}");
        }
        for bad in [
            "com", "a.b", "-a.com", "a-.com", "a..com", ".a.com", "a.c0m", "a.com.", "a_b.com",
            "a.c",
        ] {
            assert!(domain(bad.as_bytes()).is_err(), "{bad}");
        }
        let long = format!("{}.com", "a".repeat(64));
        assert!(domain(long.as_bytes()).is_err());
        let longest = format!("{}.{}.com", "a".repeat(63), "b".repeat(60));
        assert_eq!(longest.len(), MAX_DOMAIN_BYTES);
        assert!(domain(longest.as_bytes()).is_ok());
        assert!(domain(format!("a{longest}").as_bytes()).is_err());
    }

    #[test]
    fn signed_manifests_verify_and_revocations_need_no_signing_key() {
        let master = Wallet::from_seed([1; 16]);
        let signing = Secp256k1Key::from_secret(&[2; 32]).unwrap();
        let manifest = Manifest::sign(
            3,
            &master,
            Some(&signing as &dyn Key),
            Some("example.com".into()),
        );
        assert!(manifest.verify());
        assert_eq!(
            Manifest::from_bytes(&manifest.to_bytes()).unwrap(),
            manifest
        );
        assert!(!manifest.is_revocation());
        let revocation = Manifest::sign(REVOKED, &master, None, None);
        assert!(revocation.is_revocation() && revocation.verify());
        assert_eq!(
            Manifest::from_bytes(&revocation.to_bytes()).unwrap(),
            revocation
        );
        // A signature by the other key, or on other bytes, does not verify.
        let mut swapped = manifest.clone();
        swapped.master_signature = manifest.signing.unwrap().1;
        assert!(!swapped.verify());
        let mut later = manifest.clone();
        later.sequence = 4;
        assert!(!later.verify());
        let mut other = manifest.clone();
        other.signing = Some((signing.public_key(), master.sign(b"other")));
        assert!(!other.verify());
    }

    #[test]
    fn a_revocation_with_a_signing_key_and_a_manifest_without_are_refused() {
        let master = Wallet::from_seed([1; 16]);
        let signing = Secp256k1Key::from_secret(&[2; 32]).unwrap();
        let manifest = Manifest::sign(3, &master, Some(&signing as &dyn Key), None);
        let revoking = Manifest {
            sequence: REVOKED,
            ..manifest.clone()
        };
        assert_eq!(
            Manifest::from_bytes(&revoking.to_bytes()),
            Err(DecodeError::Value)
        );
        let keyless = Manifest {
            signing: None,
            ..manifest.clone()
        };
        assert_eq!(
            Manifest::from_bytes(&keyless.to_bytes()),
            Err(DecodeError::Value)
        );
        let itself = Manifest {
            signing: Some((master.public_key(), manifest.master_signature)),
            ..manifest.clone()
        };
        assert_eq!(
            Manifest::from_bytes(&itself.to_bytes()),
            Err(DecodeError::Value)
        );
        // A version, even rippled's default, and a field it has not.
        let mut versioned = Vec::new();
        binary::write_field(&mut versioned, FieldId::new(Type::UInt16, 16), &[0, 0]);
        versioned.extend_from_slice(&manifest.to_bytes());
        assert_eq!(Manifest::from_bytes(&versioned), Err(DecodeError::Field));
        let mut more = manifest.to_bytes();
        binary::write_field(&mut more, FieldId::new(Type::Blob, 19), b"x");
        assert_eq!(Manifest::from_bytes(&more), Err(DecodeError::Field));
        assert_eq!(
            Manifest::from_bytes(&[0x24; MAX_MANIFEST_BYTES + 1]),
            Err(DecodeError::Value)
        );
    }

    #[test]
    fn a_publisher_manifest_reads_and_verifies() {
        let bytes = from_hex(PUBLISHER).unwrap();
        let manifest = Manifest::from_bytes(&bytes).unwrap();
        assert_eq!(manifest.sequence, 1);
        assert_eq!(manifest.master_key.key_type(), KeyType::Ed25519);
        assert_eq!(
            manifest.master_key.to_node_public(),
            "nHBe4vqSAzjpPRLKwSFzRFtmvzXaf5wPPmuVrQCAoJoS1zskgDA4"
        );
        assert_eq!(manifest.to_bytes(), bytes);
        assert!(manifest.verify());
        assert_eq!(hex_upper(&manifest.hash()).len(), 64);
    }
}
