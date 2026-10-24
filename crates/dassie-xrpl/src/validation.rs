//! A validator's validation of a ledger, as rippled serializes it
//! (`STValidation`), and its signature
//! (spec/designs/xrpl-validation.md#8-the-verifier).
//!
//! A validation signs `VAL\0` and its fields but the signature, with its
//! validator's signing key, which is always secp256k1: ECDSA over the
//! SHA-512Half of those bytes. It counts toward a ledger's quorum only when
//! it is full and its signature fully canonical, which its flags say and
//! [`Validation::verify`] checks; whose key it is, whether it is fresh and
//! whether its validator is trusted are the quorum's to judge. A
//! validation signs no network ID: the validators' keys alone tell the
//! networks apart.
//!
//! It is read in the canonical form only (`binary`), with the fields of
//! rippled's validation format and no others, so it writes back to the
//! bytes it came in, which are the bytes its validator signed.

use alloc::vec::Vec;

use crate::binary::{self, FieldId, Fields, Type};
use crate::codec::{Amount, DecodeError};
use crate::keys::{KeyType, PublicKey, Secp256k1Key, Signature};

/// The longest validation read. A flag ledger's validation lists the
/// amendments its validator votes for, 32 bytes each; this holds over two
/// hundred, more than rippled knows.
pub const MAX_VALIDATION_BYTES: usize = 8_192;

/// The flag of a full validation: of a ledger its validator built and
/// would have the network accept. A partial one does not count.
pub const FULL: u32 = 0x0000_0001;

/// The flag of a validation whose signature is fully canonical: in strict
/// DER, with a low S.
pub const FULLY_CANONICAL: u32 = 0x8000_0000;

/// What a validation's signature covers, before its fields.
const VALIDATION_PREFIX: [u8; 4] = *b"VAL\0";

// Its fields, in canonical order.
const FLAGS: FieldId = FieldId::new(Type::UInt32, 2);
const LEDGER_SEQUENCE: FieldId = FieldId::new(Type::UInt32, 6);
const CLOSE_TIME: FieldId = FieldId::new(Type::UInt32, 7);
const SIGNING_TIME: FieldId = FieldId::new(Type::UInt32, 9);
const LOAD_FEE: FieldId = FieldId::new(Type::UInt32, 24);
const GAS_LIMIT: FieldId = FieldId::new(Type::UInt32, 81);
const BYTECODE_SIZE_LIMIT: FieldId = FieldId::new(Type::UInt32, 82);
const GAS_PRICE: FieldId = FieldId::new(Type::UInt32, 83);
const COOKIE: FieldId = FieldId::new(Type::UInt64, 10);
const SERVER_VERSION: FieldId = FieldId::new(Type::UInt64, 11);
const LEDGER_HASH: FieldId = FieldId::new(Type::Hash256, 1);
const CONSENSUS_HASH: FieldId = FieldId::new(Type::Hash256, 23);
const VALIDATED_HASH: FieldId = FieldId::new(Type::Hash256, 25);
const BASE_FEE_DROPS: FieldId = FieldId::new(Type::Amount, 22);
const RESERVE_BASE_DROPS: FieldId = FieldId::new(Type::Amount, 23);
const RESERVE_INCREMENT_DROPS: FieldId = FieldId::new(Type::Amount, 24);
const SIGNING_PUB_KEY: FieldId = FieldId::new(Type::Blob, 3);
const SIGNATURE: FieldId = FieldId::new(Type::Blob, 6);
const AMENDMENTS: FieldId = FieldId::new(Type::Vector256, 3);

/// A validation: a validator's signature on a ledger's hash and index, with
/// what else its validator says of it. Times are seconds since 2000-01-01.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// [`FULL`] and [`FULLY_CANONICAL`], or others.
    pub flags: u32,
    /// The ledger's hash.
    pub ledger_hash: [u8; 32],
    /// The ledger's index.
    pub ledger_sequence: u32,
    /// When the ledger closed, by its validator.
    pub close_time: Option<u32>,
    /// Its validator's load fee.
    pub load_fee: Option<u32>,
    /// The amendments its validator votes for, on a flag ledger.
    pub amendments: Option<Vec<[u8; 32]>>,
    /// When its validator signed it.
    pub signing_time: u32,
    /// Its validator's signing key, secp256k1.
    pub signing_key: PublicKey,
    /// The signature.
    pub signature: Signature,
    /// The hash its validator's consensus round agreed on.
    pub consensus_hash: Option<[u8; 32]>,
    /// A number its validator drew at start, never zero.
    pub cookie: Option<u64>,
    /// The newest ledger its validator saw validated.
    pub validated_hash: Option<[u8; 32]>,
    /// Its validator's version.
    pub server_version: Option<u64>,
    /// The base fee its validator votes for, on a flag ledger.
    pub base_fee_drops: Option<u64>,
    /// The base reserve its validator votes for.
    pub reserve_base_drops: Option<u64>,
    /// The owner reserve its validator votes for.
    pub reserve_increment_drops: Option<u64>,
    /// Smart escrows' gas limit its validator votes for.
    pub gas_limit: Option<u32>,
    /// Smart escrows' bytecode limit its validator votes for.
    pub bytecode_size_limit: Option<u32>,
    /// Smart escrows' gas price its validator votes for.
    pub gas_price: Option<u32>,
}

impl Validation {
    /// The validation in `bytes`, if it is one: at most
    /// [`MAX_VALIDATION_BYTES`], in canonical form, with every field
    /// rippled requires and none it does not know, each of its type, a
    /// cookie only if it is not zero, and a secp256k1 signing key. Its
    /// signature is not checked: see [`Self::verify`].
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_VALIDATION_BYTES {
            return Err(DecodeError::Value);
        }
        let mut read = Read::default();
        for field in Fields::new(bytes) {
            read.field(&field?)?;
        }
        read.finish()
    }

    /// Its bytes, as rippled writes them.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out, true);
        out
    }

    /// What its signature covers: `VAL\0` and its fields but the signature.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = VALIDATION_PREFIX.to_vec();
        self.write(&mut out, false);
        out
    }

    /// Whether it is full, says its signature is fully canonical, and is
    /// signed by its signing key, as rippled requires of a validation to
    /// count: the flags first, as they cost nothing to check.
    #[must_use]
    pub fn verify(&self) -> bool {
        self.flags & FULL != 0
            && self.flags & FULLY_CANONICAL != 0
            && self
                .signing_key
                .verify(&self.signing_bytes(), &self.signature)
    }

    /// It signed by `key`, as its validator signs it: with the flag of a
    /// fully canonical signature.
    #[must_use]
    pub fn signed(mut self, key: &Secp256k1Key) -> Self {
        self.flags |= FULLY_CANONICAL;
        self.signing_key = key.public_key();
        self.signature = key.sign(&self.signing_bytes());
        self
    }

    /// Writes its fields in canonical order, the signature only if asked.
    fn write(&self, out: &mut Vec<u8>, with_signature: bool) {
        let start = out.len();
        let uint32 = |out: &mut Vec<u8>, id, value: Option<u32>| {
            if let Some(value) = value {
                binary::write_field(out, id, &value.to_be_bytes());
            }
        };
        uint32(out, FLAGS, Some(self.flags));
        uint32(out, LEDGER_SEQUENCE, Some(self.ledger_sequence));
        uint32(out, CLOSE_TIME, self.close_time);
        uint32(out, SIGNING_TIME, Some(self.signing_time));
        uint32(out, LOAD_FEE, self.load_fee);
        uint32(out, GAS_LIMIT, self.gas_limit);
        uint32(out, BYTECODE_SIZE_LIMIT, self.bytecode_size_limit);
        uint32(out, GAS_PRICE, self.gas_price);
        for (id, value) in [(COOKIE, self.cookie), (SERVER_VERSION, self.server_version)] {
            if let Some(value) = value {
                binary::write_field(out, id, &value.to_be_bytes());
            }
        }
        for (id, hash) in [
            (LEDGER_HASH, Some(self.ledger_hash)),
            (CONSENSUS_HASH, self.consensus_hash),
            (VALIDATED_HASH, self.validated_hash),
        ] {
            if let Some(hash) = hash {
                binary::write_field(out, id, &hash);
            }
        }
        for (id, drops) in [
            (BASE_FEE_DROPS, self.base_fee_drops),
            (RESERVE_BASE_DROPS, self.reserve_base_drops),
            (RESERVE_INCREMENT_DROPS, self.reserve_increment_drops),
        ] {
            if let Some(drops) = drops {
                binary::write_field(out, id, &binary::xrp(drops));
            }
        }
        binary::write_field(out, SIGNING_PUB_KEY, self.signing_key.as_bytes());
        if with_signature {
            binary::write_field(out, SIGNATURE, self.signature.as_bytes());
        }
        if let Some(amendments) = &self.amendments {
            binary::write_field(out, AMENDMENTS, amendments.as_flattened());
        }
        assert!(
            binary::check(out.get(start..).unwrap_or_default()).is_ok(),
            "a validation is written in canonical form"
        );
    }
}

/// A validation's fields as read so far.
#[derive(Default)]
struct Read {
    flags: Option<u32>,
    ledger_hash: Option<[u8; 32]>,
    ledger_sequence: Option<u32>,
    signing_time: Option<u32>,
    signing_key: Option<PublicKey>,
    signature: Option<Signature>,
    rest: Optional,
}

/// What a validation may leave out.
#[derive(Default)]
struct Optional {
    close_time: Option<u32>,
    load_fee: Option<u32>,
    amendments: Option<Vec<[u8; 32]>>,
    consensus_hash: Option<[u8; 32]>,
    cookie: Option<u64>,
    validated_hash: Option<[u8; 32]>,
    server_version: Option<u64>,
    base_fee_drops: Option<u64>,
    reserve_base_drops: Option<u64>,
    reserve_increment_drops: Option<u64>,
    gas_limit: Option<u32>,
    bytecode_size_limit: Option<u32>,
    gas_price: Option<u32>,
}

impl Read {
    /// Takes `field`, one of a validation's. Each comes at most once, as
    /// the walker reads them only ascending.
    fn field(&mut self, field: &binary::Field<'_>) -> Result<(), DecodeError> {
        let rest = &mut self.rest;
        match field.id {
            FLAGS => self.flags = Some(field.uint32()?),
            LEDGER_SEQUENCE => self.ledger_sequence = Some(field.uint32()?),
            CLOSE_TIME => rest.close_time = Some(field.uint32()?),
            SIGNING_TIME => self.signing_time = Some(field.uint32()?),
            LOAD_FEE => rest.load_fee = Some(field.uint32()?),
            GAS_LIMIT => rest.gas_limit = Some(field.uint32()?),
            BYTECODE_SIZE_LIMIT => rest.bytecode_size_limit = Some(field.uint32()?),
            GAS_PRICE => rest.gas_price = Some(field.uint32()?),
            COOKIE => {
                // Zero is rippled's default, which it never writes.
                let cookie = field.uint64()?;
                if cookie == 0 {
                    return Err(DecodeError::Value);
                }
                rest.cookie = Some(cookie);
            }
            SERVER_VERSION => rest.server_version = Some(field.uint64()?),
            LEDGER_HASH => self.ledger_hash = Some(field.hash256()?),
            CONSENSUS_HASH => rest.consensus_hash = Some(field.hash256()?),
            VALIDATED_HASH => rest.validated_hash = Some(field.hash256()?),
            BASE_FEE_DROPS => rest.base_fee_drops = Some(drops(field)?),
            RESERVE_BASE_DROPS => rest.reserve_base_drops = Some(drops(field)?),
            RESERVE_INCREMENT_DROPS => rest.reserve_increment_drops = Some(drops(field)?),
            SIGNING_PUB_KEY => {
                let bytes: [u8; 33] = field.blob()?.try_into().map_err(|_| DecodeError::Value)?;
                let key = PublicKey::from_bytes(bytes).ok_or(DecodeError::Value)?;
                if key.key_type() != KeyType::Secp256k1 {
                    return Err(DecodeError::Value);
                }
                self.signing_key = Some(key);
            }
            SIGNATURE => {
                self.signature =
                    Some(Signature::from_slice(field.blob()?).ok_or(DecodeError::Value)?);
            }
            AMENDMENTS => rest.amendments = Some(field.vector256()?.to_vec()),
            _ => return Err(DecodeError::Field),
        }
        Ok(())
    }

    /// The validation, if it has every field it must.
    fn finish(self) -> Result<Validation, DecodeError> {
        let missing = DecodeError::Field;
        let rest = self.rest;
        Ok(Validation {
            flags: self.flags.ok_or(missing)?,
            ledger_hash: self.ledger_hash.ok_or(missing)?,
            ledger_sequence: self.ledger_sequence.ok_or(missing)?,
            close_time: rest.close_time,
            load_fee: rest.load_fee,
            amendments: rest.amendments,
            signing_time: self.signing_time.ok_or(missing)?,
            signing_key: self.signing_key.ok_or(missing)?,
            signature: self.signature.ok_or(missing)?,
            consensus_hash: rest.consensus_hash,
            cookie: rest.cookie,
            validated_hash: rest.validated_hash,
            server_version: rest.server_version,
            base_fee_drops: rest.base_fee_drops,
            reserve_base_drops: rest.reserve_base_drops,
            reserve_increment_drops: rest.reserve_increment_drops,
            gas_limit: rest.gas_limit,
            bytecode_size_limit: rest.bytecode_size_limit,
            gas_price: rest.gas_price,
        })
    }
}

/// A fee a validator votes for: XRP.
fn drops(field: &binary::Field<'_>) -> Result<u64, DecodeError> {
    match field.amount()? {
        Amount::Xrp(drops) => Ok(drops),
        Amount::Token(_) => Err(DecodeError::Value),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::from_hex;

    // A mainnet validation of ledger 107862130, as s1.ripple.com streamed it
    // (testdata of `tests/ledger_binary.rs`).
    const DATA: &str = "228000000126066DD87229326ED58A3AA634B3860AC9C61C5151966685A066F132C7DBAC4B4FAFEE4BB04198FCF67A06005DC179AD63C1C7C25017698092EA3D07537EFA8A0EE1A92201702252EDA5ECD961FE8B9D0ECBCCD202DA5019DB32F9B497B62C9E467CF37ED9181A31AF823716E8809C07EA4E95B0BEBBA1737321034B2562011A5A06608454583621331D1234FC597177321E276242124CD1DF111A76473045022100BC3150D4776F4FB66E9962472669157CAE30149A960C358A8DB01804DEA5BD98022035575E983E35786483FA88B41C95B4D6123C02E8FBC616FC05C8B435C068A3A9";

    fn live() -> Validation {
        Validation::from_bytes(&from_hex(DATA).unwrap()).unwrap()
    }

    #[test]
    fn a_live_validation_reads_writes_back_and_verifies() {
        let validation = live();
        assert_eq!(validation.flags, FULL | FULLY_CANONICAL);
        assert_eq!(validation.ledger_sequence, 107_862_130);
        assert_eq!(validation.signing_time, 846_124_426);
        assert_eq!(validation.cookie, Some(11_976_394_697_372_976_668));
        assert_eq!(
            validation.signing_key.to_node_public(),
            "n9MhLZsK7Av6ny2gV5SAGLDsnFXE9p85aYR8diD8xvuvuucqad85"
        );
        assert_eq!(validation.to_bytes(), from_hex(DATA).unwrap());
        assert!(validation.verify());
    }

    #[test]
    fn a_partial_or_changed_validation_does_not_verify() {
        let partial = Validation {
            flags: FULLY_CANONICAL,
            ..live()
        };
        assert!(!partial.verify());
        let unflagged = Validation {
            flags: FULL,
            ..live()
        };
        assert!(!unflagged.verify());
        let other = Validation {
            ledger_sequence: 107_862_131,
            ..live()
        };
        assert!(!other.verify());
        // Signed again by another key, it verifies under that key alone.
        let key = Secp256k1Key::from_secret(&[0x11; 32]).unwrap();
        let resigned = live().signed(&key);
        assert!(resigned.verify());
        let swapped = Validation {
            signing_key: live().signing_key,
            ..resigned
        };
        assert!(!swapped.verify());
    }

    #[test]
    fn a_live_signature_made_malleable_does_not_verify() {
        // The same signature with a high S, or its DER padded, is ECDSA's
        // all the same, but not fully canonical: a relay that changes it
        // gets nothing that counts.
        let validation = live();
        let parsed = k256::ecdsa::Signature::from_der(validation.signature.as_bytes()).unwrap();
        let (r, s) = parsed.split_scalars();
        let high = k256::ecdsa::Signature::from_scalars(r.to_bytes(), (-*s).to_bytes()).unwrap();
        let high = Validation {
            signature: Signature::from_slice(high.to_der().as_bytes()).unwrap(),
            ..live()
        };
        assert!(!high.verify());
        let der = validation.signature.as_bytes();
        // SEQUENCE, then INTEGER r with a needless zero in front.
        let r_length = usize::from(der[3]);
        let mut padded = vec![0x30, der[1] + 1, 0x02, der[3] + 1, 0x00];
        padded.extend_from_slice(&der[4..4 + r_length]);
        padded.extend_from_slice(&der[4 + r_length..]);
        let padded = Validation {
            signature: Signature::from_slice(&padded).unwrap(),
            ..live()
        };
        assert!(Validation::from_bytes(&padded.to_bytes()).is_ok());
        assert!(!padded.verify());
    }

    #[test]
    fn its_signing_bytes_are_its_bytes_without_the_signature() {
        // Cut out of the bytes as received, field by field.
        let bytes = from_hex(DATA).unwrap();
        let mut cut = VALIDATION_PREFIX.to_vec();
        for field in Fields::new(&bytes) {
            let field = field.unwrap();
            if field.id != SIGNATURE {
                binary::write_field(&mut cut, field.id, field.value);
            }
        }
        assert_eq!(live().signing_bytes(), cut);
        // Less the signature, its header and its length.
        let signature = live().signature.as_bytes().len();
        assert_eq!(
            cut.len(),
            VALIDATION_PREFIX.len() + bytes.len() - signature - 2
        );
    }

    #[test]
    fn only_rippled_s_validation_format_reads() {
        let bytes = from_hex(DATA).unwrap();
        let without = |id: FieldId| {
            let mut out = Vec::new();
            for field in Fields::new(&bytes) {
                let field = field.unwrap();
                if field.id != id {
                    binary::write_field(&mut out, field.id, field.value);
                }
            }
            out
        };
        for required in [
            FLAGS,
            LEDGER_HASH,
            LEDGER_SEQUENCE,
            SIGNING_TIME,
            SIGNING_PUB_KEY,
            SIGNATURE,
        ] {
            assert_eq!(
                Validation::from_bytes(&without(required)),
                Err(DecodeError::Field)
            );
        }
        assert!(Validation::from_bytes(&without(COOKIE)).is_ok());
        // A field rippled's format has not.
        let mut more = bytes.clone();
        binary::write_field(&mut more, FieldId::new(Type::Blob, 7), b"x");
        assert_eq!(Validation::from_bytes(&more), Err(DecodeError::Field));
        // A cookie of zero, written all the same.
        let mut zero = Vec::new();
        for field in Fields::new(&bytes) {
            let field = field.unwrap();
            let value: &[u8] = if field.id == COOKIE {
                &[0; 8]
            } else {
                field.value
            };
            binary::write_field(&mut zero, field.id, value);
        }
        assert_eq!(Validation::from_bytes(&zero), Err(DecodeError::Value));
        assert_eq!(
            Validation::from_bytes(&vec![0x22; MAX_VALIDATION_BYTES + 1]),
            Err(DecodeError::Value)
        );
    }

    #[test]
    fn an_ed25519_signing_key_is_refused() {
        let mut validation = live();
        let mut ed25519 = [0xED; 33];
        ed25519[1..].copy_from_slice(&[7; 32]);
        validation.signing_key = PublicKey::from_bytes(ed25519).unwrap();
        assert_eq!(
            Validation::from_bytes(&validation.to_bytes()),
            Err(DecodeError::Value)
        );
    }

    #[test]
    fn every_optional_field_writes_back() {
        let validation = Validation {
            close_time: Some(1),
            load_fee: Some(256),
            amendments: Some(vec![[1; 32], [2; 32]]),
            consensus_hash: None,
            server_version: Some(7),
            base_fee_drops: Some(10),
            reserve_base_drops: Some(1_000_000),
            reserve_increment_drops: Some(200_000),
            gas_limit: Some(1),
            bytecode_size_limit: Some(2),
            gas_price: Some(3),
            ..live()
        }
        .signed(&Secp256k1Key::from_secret(&[0x22; 32]).unwrap());
        let bytes = validation.to_bytes();
        assert_eq!(Validation::from_bytes(&bytes).unwrap(), validation);
        assert!(validation.verify());
    }
}
