//! What a payer signs for its payee on a hold
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub): a commitment with
//! every Prepare, and a cumulative claim with its reply to every Fulfill.
//! The payee takes from the hold with either, without the payer's help.
//!
//! Both travel between members only. The hub sees a claim when the payee
//! redeems it, and commitments only in a dispute.

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Encode, Reader, Writer};

use crate::codec::{
    DIGEST_LEN, HubDecodeError, SIGNATURE_LEN, read_digest, read_signature, write_digest,
    write_signature,
};
use crate::digest::{Digest, Domain};
use crate::rules::{Amount, HubId, Millis};

/// A hold's number in its hub, assigned in order and never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HoldNumber(pub u64);

/// The payer's promise, sent with a Prepare, to pay `amount` from the hold
/// to whoever shows the preimage of `condition` in a block stamped before
/// `expiry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commitment {
    /// The hold it draws on.
    pub hold: HoldNumber,
    /// Increasing per hold, one per Prepare.
    pub sequence: u64,
    /// The Prepare's amount, in the hub's asset.
    pub amount: Amount,
    /// The Prepare's execution condition.
    pub condition: [u8; 32],
    /// The Prepare's expiry less the hub's margin M.
    pub expiry: Millis,
}

/// The payer's statement, sent with its reply to a Fulfill, that it owes
/// `total` on the hold, covering every fulfilled packet up to `through`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The hold it draws on.
    pub hold: HoldNumber,
    /// T: everything owed on the hold so far. Only grows.
    pub total: Amount,
    /// W: the commitment sequence up to which `total` covers every
    /// fulfilled packet. Only grows.
    pub through: u64,
}

/// A commitment or a claim with the payer's signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signed<T> {
    /// What was signed.
    pub value: T,
    /// The payer's account key's signature over the value's digest.
    pub signature: Signature,
}

/// The encoded size of a commitment.
pub const COMMITMENT_LEN: usize = 8 * 4 + 32;
/// The encoded size of a claim.
pub const CLAIM_LEN: usize = 8 * 3;

impl Commitment {
    /// The digest the payer signs: the hub ID and the commitment.
    #[must_use]
    pub fn digest(&self, hub: &HubId) -> Digest {
        Digest::of(
            Domain::Commitment,
            &[hub.0.as_bytes(), &self.to_vec_fixed()],
        )
    }

    fn to_vec_fixed(self) -> [u8; COMMITMENT_LEN] {
        let mut bytes = [0; COMMITMENT_LEN];
        self.encode(&mut bytes);
        bytes
    }

    /// Whether `fulfillment` is the preimage of the condition.
    #[must_use]
    pub fn fulfilled_by(&self, fulfillment: &[u8; 32]) -> bool {
        use sha2::{Digest as _, Sha256};
        let hash: [u8; 32] = Sha256::digest(fulfillment).into();
        hash == self.condition
    }
}

impl Claim {
    /// The digest the payer signs: the hub ID and the claim.
    #[must_use]
    pub fn digest(&self, hub: &HubId) -> Digest {
        let mut bytes = [0; CLAIM_LEN];
        self.encode(&mut bytes);
        Digest::of(Domain::Claim, &[hub.0.as_bytes(), &bytes])
    }
}

/// What can be signed by a payer.
pub trait Promise: Encode + Copy {
    /// The digest the payer signs.
    fn promise_digest(&self, hub: &HubId) -> Digest;
}

impl Promise for Commitment {
    fn promise_digest(&self, hub: &HubId) -> Digest {
        self.digest(hub)
    }
}

impl Promise for Claim {
    fn promise_digest(&self, hub: &HubId) -> Digest {
        self.digest(hub)
    }
}

impl<T: Promise> Signed<T> {
    /// Signs `value` with the payer's account key.
    #[must_use]
    pub fn sign(value: T, hub: &HubId, key: &NodeKey) -> Self {
        let signature = key.sign(value.promise_digest(hub).as_bytes());
        Self { value, signature }
    }

    /// Whether the payer's key `payer` signed it.
    #[must_use]
    pub fn verifies(&self, hub: &HubId, payer: &PublicKey) -> bool {
        payer
            .verify(self.value.promise_digest(hub).as_bytes(), &self.signature)
            .is_ok()
    }
}

impl Encode for Commitment {
    fn encoded_len(&self) -> usize {
        COMMITMENT_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.hold.0);
        writer.write_u64(self.sequence);
        writer.write_u64(self.amount);
        writer.write_bytes(&self.condition);
        writer.write_u64(self.expiry);
    }
}

impl Commitment {
    /// Decodes a commitment.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            hold: HoldNumber(reader.read_u64()?),
            sequence: reader.read_u64()?,
            amount: reader.read_u64()?,
            condition: *reader.read_array::<32>()?,
            expiry: reader.read_u64()?,
        })
    }
}

impl Encode for Claim {
    fn encoded_len(&self) -> usize {
        CLAIM_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.hold.0);
        writer.write_u64(self.total);
        writer.write_u64(self.through);
    }
}

impl Claim {
    /// Decodes a claim.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            hold: HoldNumber(reader.read_u64()?),
            total: reader.read_u64()?,
            through: reader.read_u64()?,
        })
    }
}

impl<T: Promise> Encode for Signed<T> {
    fn encoded_len(&self) -> usize {
        self.value.encoded_len() + SIGNATURE_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.value.encode_to(writer);
        write_signature(writer, &self.signature);
    }
}

impl Signed<Commitment> {
    /// Decodes a signed commitment.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            value: Commitment::decode(reader)?,
            signature: read_signature(reader)?,
        })
    }
}

impl Signed<Claim> {
    /// Decodes a signed claim.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            value: Claim::decode(reader)?,
            signature: read_signature(reader)?,
        })
    }
}

/// A commitment with the fulfillment that settles it, as `claim` carries
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fulfilled {
    /// The payer's signed commitment.
    pub commitment: Signed<Commitment>,
    /// The preimage of its condition.
    pub fulfillment: [u8; 32],
}

impl Encode for Fulfilled {
    fn encoded_len(&self) -> usize {
        COMMITMENT_LEN + SIGNATURE_LEN + DIGEST_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.commitment.encode_to(writer);
        write_digest(writer, &Digest(self.fulfillment));
    }
}

impl Fulfilled {
    /// Decodes a fulfilled commitment.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            commitment: Signed::<Commitment>::decode(reader)?,
            fulfillment: read_digest(reader)?.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn signatures_bind_the_hub() {
        let key = NodeKey::from_bytes(&[3; 32]);
        let hub = HubId(Digest([1; 32]));
        let other = HubId(Digest([2; 32]));
        let claim = Claim {
            hold: HoldNumber(4),
            total: 500,
            through: 9,
        };
        let signed = Signed::sign(claim, &hub, &key);
        assert!(signed.verifies(&hub, &key.public_key()));
        assert!(!signed.verifies(&other, &key.public_key()));
    }

    #[test]
    fn fulfillment_matches_condition() {
        let fulfillment = [5; 32];
        let commitment = Commitment {
            hold: HoldNumber(1),
            sequence: 1,
            amount: 10,
            condition: Sha256::digest(fulfillment).into(),
            expiry: 1_000,
        };
        assert!(commitment.fulfilled_by(&fulfillment));
        assert!(!commitment.fulfilled_by(&[6; 32]));
    }
}
