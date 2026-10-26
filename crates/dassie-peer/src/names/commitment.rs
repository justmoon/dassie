//! The commitment that seals a name (spec/designs/names.md#3-registering).
//!
//! `C = SHA-256(oid ‖ name ‖ holder ‖ salt)`, over the canonical OER of
//!
//! ```text
//! NameCommitment ::= SEQUENCE {
//!     domain  OBJECT IDENTIFIER,                 -- COMMITMENT_OID
//!     name    IA5String (SIZE (3..32)),          -- the name
//!     holder  IA5String (SIZE (2..45)),          -- the holder's node ID
//!     salt    OCTET STRING (SIZE (32))
//! }
//! ```
//!
//! so the bytes hashed are: the OID with its length (`0d 2b 06 01 04 01 83
//! a8 78 05 8a 77 01 04`), the name's length in one byte and its
//! characters, the node ID's length in one byte and its characters, and the
//! 32 bytes of salt. Every field has its length ahead of it or a fixed one,
//! so no two inputs hash the same bytes.
//!
//! - **The domain OID** keeps a commitment from passing for any other hash
//!   Dassie signs or compares (spec/areas/identity.md#object-identifiers).
//! - **The salt** keeps a dictionary of likely names from opening a
//!   commitment.
//! - **The holder** makes a copied commitment useless: opening it registers
//!   the name only to the node that made it.

use alloc::vec;

use dassie_identity::NodeId;
use dassie_oer::{Writer, oid};
use sha2::{Digest, Sha256};

use super::Name;
use super::name::{name_len, write_name};
use crate::wire::{node_id_len, write_node_id};

/// The domain of a name's commitment, `…1399.1.4`: the arc after the link
/// state's under the peer protocol's (spec/areas/identity.md#object-identifiers).
pub const COMMITMENT_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 4]);

/// The bytes of a salt.
pub const SALT_BYTES: usize = 32;

/// The bytes of a commitment: a SHA-256 digest.
pub const COMMITMENT_BYTES: usize = 32;

/// The salt a registrant picks for a commitment, at random, and keeps
/// until it unseals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Salt(pub [u8; SALT_BYTES]);

/// A commitment to a name, its holder and a salt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Commitment(pub [u8; COMMITMENT_BYTES]);

impl Commitment {
    /// The commitment to `name`, held by `holder`, under `salt`.
    #[must_use]
    pub fn seal(name: &Name, holder: &NodeId, salt: &Salt) -> Self {
        Self(Sha256::digest(preimage(name, holder, salt)).into())
    }

    /// Whether `name`, `holder` and `salt` open this commitment: whether it
    /// is the commitment to them.
    #[must_use]
    pub fn is_opened_by(&self, name: &Name, holder: &NodeId, salt: &Salt) -> bool {
        Self::seal(name, holder, salt) == *self
    }
}

/// The bytes a commitment hashes.
fn preimage(name: &Name, holder: &NodeId, salt: &Salt) -> alloc::vec::Vec<u8> {
    let length = COMMITMENT_OID.len() + name_len(name) + node_id_len(holder) + SALT_BYTES;
    let mut bytes = vec![0u8; length];
    let mut writer = Writer::new(&mut bytes);
    writer.write_bytes(&COMMITMENT_OID);
    write_name(&mut writer, name);
    write_node_id(&mut writer, holder);
    writer.write_bytes(&salt.0);
    assert_eq!(
        writer.position(),
        length,
        "a commitment's preimage fills it"
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder() -> NodeId {
        dassie_identity::NodeKey::from_bytes(&[1; 32]).node_id()
    }

    fn alice() -> Name {
        Name::parse("alice").unwrap()
    }

    // The bytes hashed, pinned field by field: a change to them is a change
    // to the protocol, which every registrar must agree on.
    #[test]
    fn the_preimage_is_pinned() {
        let salt = Salt([7; SALT_BYTES]);
        let bytes = preimage(&alice(), &holder(), &salt);
        let mut expected = vec![
            0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x01, 0x04,
        ];
        expected.push(5);
        expected.extend(b"alice");
        expected.extend(holder().to_bytes());
        expected.extend([7; SALT_BYTES]);
        assert_eq!(bytes, expected);
        assert_eq!(COMMITMENT_OID[..13], crate::wire::NODE_INFO_OID[..13]);
    }

    #[test]
    fn a_commitment_opens_only_with_what_it_sealed() {
        let salt = Salt([7; SALT_BYTES]);
        let commitment = Commitment::seal(&alice(), &holder(), &salt);
        assert!(commitment.is_opened_by(&alice(), &holder(), &salt));
        let bob = Name::parse("bob").unwrap();
        assert!(!commitment.is_opened_by(&bob, &holder(), &salt));
        let other = dassie_identity::NodeKey::from_bytes(&[3; 32]).node_id();
        assert!(!commitment.is_opened_by(&alice(), &other, &salt));
        assert!(!commitment.is_opened_by(&alice(), &holder(), &Salt([8; SALT_BYTES])));
    }
}
