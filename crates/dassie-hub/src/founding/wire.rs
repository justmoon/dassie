//! What a sponsor and its validators say to each other while they found a
//! hub (spec/areas/peer-protocol.md#the-hubs-stream): a request of the
//! sponsor's, and the validator's answer. Canonical OER, as every hub
//! message.

use alloc::vec::Vec;

use dassie_identity::Signature;
use dassie_oer::{Decode, Encode, Reader, Writer, size};

use super::genesis_digest;
use crate::codec::{
    HubDecodeError, KEY_LEN, NODE_LEN, SIGNATURE_LEN, read_digest, read_signature, write_digest,
    write_signature,
};
use crate::rules::{Acceptance, HubId, MAX_ENDPOINT_LEN, Millis, Rules, Terms};
use crate::validators::{MAX_VALIDATORS, ValidatorIndex};

/// The most bytes of a founding request or answer: rules with the most
/// validators, each with the longest endpoint, and a signature of each.
pub const MAX_FOUNDING_MESSAGE_BYTES: usize = 16 * 1024;

/// An acceptance at its largest: the node's two keys, its consensus key,
/// its escrow and payout keys, the payout chain code, the endpoint with its
/// length, and the signature.
const MAX_ACCEPTANCE_BYTES: usize =
    NODE_LEN + KEY_LEN + 33 * 2 + 32 + 2 + MAX_ENDPOINT_LEN + SIGNATURE_LEN;
const MAX_TERMS_BYTES: usize = 512;
const _: () = assert!(
    1 + MAX_TERMS_BYTES
        + 9
        + MAX_VALIDATORS * MAX_ACCEPTANCE_BYTES
        + 8
        + 9
        + MAX_VALIDATORS * (1 + SIGNATURE_LEN)
        <= MAX_FOUNDING_MESSAGE_BYTES,
    "the largest certificate fits a founding message"
);

/// A sponsor's request to a validator.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FoundingRequest {
    /// Accept these terms: make keys under their nonce, and answer the
    /// acceptance.
    Propose(Terms),
    /// Sign the genesis of these rules, stamped `genesis_ms`.
    Genesis {
        /// The rules: the terms and every validator's acceptance.
        rules: Rules,
        /// The genesis block's timestamp.
        genesis_ms: Millis,
    },
    /// Run the hub this certificate founds.
    Certificate(GenesisCertificate),
    /// How the validator stands with the hub of this ID, which it runs.
    Status(HubId),
}

/// A validator's answer.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FoundingAnswer {
    /// It accepted the terms.
    Accepted(Acceptance),
    /// Its consensus key's signature of the genesis
    /// ([`genesis_digest`](super::genesis_digest)).
    Signed(Signature),
    /// It runs the hub.
    Running {
        /// Whether a quorum of the hub's validators checked its escrow, as
        /// this validator's ledger says.
        escrow_checked: bool,
    },
    /// It will not.
    Refused(Refusal),
}

/// Why a validator will not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Its owner does not list the sponsor, or did not turn validating on.
    NotAllowed = 1,
    /// It validates as many hubs as its own limit allows, founding ones
    /// included.
    Full = 2,
    /// The terms or rules do not check.
    Terms = 3,
    /// It signed the genesis of other rules under this nonce.
    OtherGenesis = 4,
    /// It has no public endpoint to name in an acceptance.
    NoEndpoint = 5,
    /// The rules carry no acceptance of its own as it makes them.
    NotInRules = 6,
    /// The genesis timestamp is too far from its clock.
    Time = 7,
    /// The certificate does not carry a quorum's signatures of the genesis.
    Certificate = 8,
    /// It runs no hub of that ID founded by the asker.
    UnknownHub = 9,
    /// It cannot answer now; asked again later, it may.
    Busy = 10,
}

impl Refusal {
    /// The refusal of `code`.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::NotAllowed,
            2 => Self::Full,
            3 => Self::Terms,
            4 => Self::OtherGenesis,
            5 => Self::NoEndpoint,
            6 => Self::NotInRules,
            7 => Self::Time,
            8 => Self::Certificate,
            9 => Self::UnknownHub,
            10 => Self::Busy,
            _ => return None,
        })
    }

    /// Its code on the wire.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// What it means, for the owner.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NotAllowed => "its owner does not validate for this sponsor",
            Self::Full => "it validates as many hubs as it takes",
            Self::Terms => "the terms or rules do not check",
            Self::OtherGenesis => "it signed another genesis under this nonce",
            Self::NoEndpoint => "it has no public endpoint",
            Self::NotInRules => "the rules do not carry its acceptance",
            Self::Time => "the genesis timestamp is too far from its clock",
            Self::Certificate => "the genesis certificate does not check",
            Self::UnknownHub => "it runs no such hub",
            Self::Busy => "it is busy",
        }
    }
}

/// A quorum's signatures over a hub's genesis: the hub is founded once a
/// quorum of its validators signed the same rules and timestamp, each
/// having seen the whole set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenesisCertificate {
    /// The rules.
    pub rules: Rules,
    /// The genesis block's timestamp.
    pub genesis_ms: Millis,
    /// Each signer's place in the rules and its consensus key's signature
    /// over [`genesis_digest`], in ascending order of place.
    pub signatures: Vec<(ValidatorIndex, Signature)>,
}

impl GenesisCertificate {
    /// Whether the rules check, and a quorum of their validators signed
    /// the genesis, each once, in order of place.
    #[must_use]
    pub fn verifies(&self) -> bool {
        let Some(set) = self.rules.validator_set() else {
            return false;
        };
        if self.rules.check().is_err() {
            return false;
        }
        let digest = genesis_digest(&self.rules, self.genesis_ms);
        let ascending = self
            .signatures
            .iter()
            .zip(self.signatures.iter().skip(1))
            .all(|((a, _), (b, _))| a < b);
        let valid = self.signatures.iter().all(|(index, signature)| {
            set.key(*index)
                .is_some_and(|key| key.verify(digest.as_bytes(), signature).is_ok())
        });
        ascending && valid && self.signatures.len() >= set.quorum()
    }
}

impl Encode for GenesisCertificate {
    fn encoded_len(&self) -> usize {
        self.rules.encoded_len()
            + 8
            + size::quantity(self.signatures.len())
            + self.signatures.len() * (1 + SIGNATURE_LEN)
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.rules.encode_to(writer);
        writer.write_u64(self.genesis_ms);
        writer.write_quantity(self.signatures.len());
        for (index, signature) in &self.signatures {
            writer.write_u8(u8::try_from(index.get()).unwrap_or(u8::MAX));
            write_signature(writer, signature);
        }
    }
}

impl<'a> Decode<'a> for GenesisCertificate {
    type Error = HubDecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, HubDecodeError> {
        let rules = Rules::decode(reader)?;
        let genesis_ms = reader.read_u64()?;
        let count = reader.read_quantity(MAX_VALIDATORS)?;
        let mut signatures = Vec::with_capacity(count);
        for _ in 0..count {
            let index = ValidatorIndex::new(usize::from(reader.read_u8()?))
                .ok_or(HubDecodeError::Invalid("validator index"))?;
            signatures.push((index, read_signature(reader)?));
        }
        Ok(Self {
            rules,
            genesis_ms,
            signatures,
        })
    }
}

impl Encode for FoundingRequest {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Propose(terms) => terms.encoded_len(),
            Self::Genesis { rules, .. } => rules.encoded_len() + 8,
            Self::Certificate(certificate) => certificate.encoded_len(),
            Self::Status(_) => 32,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Propose(terms) => {
                writer.write_u8(0);
                terms.encode_to(writer);
            }
            Self::Genesis { rules, genesis_ms } => {
                writer.write_u8(1);
                rules.encode_to(writer);
                writer.write_u64(*genesis_ms);
            }
            Self::Certificate(certificate) => {
                writer.write_u8(2);
                certificate.encode_to(writer);
            }
            Self::Status(hub) => {
                writer.write_u8(3);
                write_digest(writer, &hub.0);
            }
        }
    }
}

impl<'a> Decode<'a> for FoundingRequest {
    type Error = HubDecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, HubDecodeError> {
        Ok(match reader.read_u8()? {
            0 => Self::Propose(Terms::decode(reader)?),
            1 => Self::Genesis {
                rules: Rules::decode(reader)?,
                genesis_ms: reader.read_u64()?,
            },
            2 => Self::Certificate(GenesisCertificate::decode(reader)?),
            3 => Self::Status(HubId(read_digest(reader)?)),
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

impl Encode for FoundingAnswer {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Accepted(acceptance) => acceptance.encoded_len(),
            Self::Signed(_) => SIGNATURE_LEN,
            Self::Running { .. } | Self::Refused(_) => 1,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Accepted(acceptance) => {
                writer.write_u8(0);
                acceptance.encode_to(writer);
            }
            Self::Signed(signature) => {
                writer.write_u8(1);
                write_signature(writer, signature);
            }
            Self::Running { escrow_checked } => {
                writer.write_u8(2);
                writer.write_bool(*escrow_checked);
            }
            Self::Refused(refusal) => {
                writer.write_u8(3);
                writer.write_u8(refusal.code());
            }
        }
    }
}

impl<'a> Decode<'a> for FoundingAnswer {
    type Error = HubDecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, HubDecodeError> {
        Ok(match reader.read_u8()? {
            0 => Self::Accepted(Acceptance::decode(reader)?),
            1 => Self::Signed(read_signature(reader)?),
            2 => Self::Running {
                escrow_checked: reader.read_bool()?,
            },
            3 => Self::Refused(
                Refusal::from_code(reader.read_u8()?).ok_or(HubDecodeError::Invalid("refusal"))?,
            ),
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

#[cfg(test)]
mod tests {
    use dassie_identity::NodeKey;

    use super::*;
    use crate::rules::tests::rules;

    fn round_trip<T>(value: &T)
    where
        T: Encode + for<'a> Decode<'a, Error = HubDecodeError> + PartialEq + core::fmt::Debug,
    {
        let bytes = value.to_vec();
        assert!(bytes.len() <= MAX_FOUNDING_MESSAGE_BYTES);
        assert_eq!(&T::decode_exact(&bytes).unwrap(), value);
    }

    #[test]
    fn messages_read_back() {
        let (rules, keys) = rules(4);
        let digest = genesis_digest(&rules, 5);
        let signatures = keys
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, key)| (ValidatorIndex::new(i).unwrap(), key.sign(digest.as_bytes())))
            .collect();
        let certificate = GenesisCertificate {
            rules: rules.clone(),
            genesis_ms: 5,
            signatures,
        };
        assert!(certificate.verifies());
        round_trip(&FoundingRequest::Propose(rules.terms.clone()));
        round_trip(&FoundingRequest::Genesis {
            rules: rules.clone(),
            genesis_ms: 5,
        });
        round_trip(&FoundingRequest::Certificate(certificate));
        round_trip(&FoundingRequest::Status(rules.id()));
        round_trip(&FoundingAnswer::Accepted(rules.validators[0].clone()));
        let key = NodeKey::from_bytes(&[3; 32]);
        round_trip(&FoundingAnswer::Signed(key.sign(b"x")));
        round_trip(&FoundingAnswer::Running {
            escrow_checked: true,
        });
        for code in 1..=10 {
            let refusal = Refusal::from_code(code).unwrap();
            assert_eq!(refusal.code(), code);
            round_trip(&FoundingAnswer::Refused(refusal));
        }
        assert!(FoundingAnswer::decode_exact(&[3, 0]).is_err());
        assert!(FoundingRequest::decode_exact(&[9]).is_err());
    }

    // A certificate counts only a quorum of distinct validators of its
    // rules, each with its own consensus key's signature of this genesis.
    #[test]
    fn a_certificate_needs_a_quorum_of_real_signatures() {
        let (rules, keys) = rules(4);
        let digest = genesis_digest(&rules, 5);
        let sign = |i: usize| {
            (
                ValidatorIndex::new(i).unwrap(),
                keys[i].sign(digest.as_bytes()),
            )
        };
        let certificate = |signatures| GenesisCertificate {
            rules: rules.clone(),
            genesis_ms: 5,
            signatures,
        };
        assert!(certificate(alloc::vec![sign(0), sign(1), sign(3)]).verifies());
        assert!(
            !certificate(alloc::vec![sign(0), sign(1)]).verifies(),
            "too few"
        );
        assert!(
            !certificate(alloc::vec![sign(0), sign(1), sign(1)]).verifies(),
            "twice"
        );
        assert!(
            !certificate(alloc::vec![sign(1), sign(0), sign(2)]).verifies(),
            "out of order"
        );
        // A signature of another genesis, or by another key, is forged.
        let other = genesis_digest(&rules, 6);
        let forged = (
            ValidatorIndex::new(2).unwrap(),
            keys[2].sign(other.as_bytes()),
        );
        assert!(!certificate(alloc::vec![sign(0), sign(1), forged]).verifies());
        let stranger = NodeKey::from_bytes(&[99; 32]);
        let forged = (
            ValidatorIndex::new(2).unwrap(),
            stranger.sign(digest.as_bytes()),
        );
        assert!(!certificate(alloc::vec![sign(0), sign(1), forged]).verifies());
        let past = (
            ValidatorIndex::new(5).unwrap(),
            keys[0].sign(digest.as_bytes()),
        );
        assert!(!certificate(alloc::vec![sign(0), sign(1), past]).verifies());
    }
}
