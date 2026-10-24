//! The ledger's skip lists, which prove older ledgers from a newer one
//! (spec/designs/xrpl-validation.md#4-what-is-proven-and-how): each ledger's
//! state holds a `LedgerHashes` object of the hashes of the 256 ledgers
//! before it, and one for every 65,536 ledgers that holds the hashes of the
//! flag ledgers among them, every 256th. So once a ledger is proven, a
//! state proof at it of one of these proves a whole run of older ledgers
//! at once, and any ledger of its chain is at most two such proofs away:
//! through the flag ledger after it, whose own list of the 256 before it
//! holds it.
//!
//! A hash taken from a list proven at a proven ledger is a proven
//! ledger's. A list is read only as rippled writes it, and bounded.

use alloc::vec::Vec;

use crate::binary::{FieldId, Fields, Type};
use crate::codec::DecodeError;
use crate::keys::sha512_half;

/// The hashes one list holds: of the 256 ledgers before a ledger, or of
/// 256 flag ledgers.
pub const SKIP_HASHES: usize = 256;

/// Every 256th ledger is a flag ledger.
pub const FLAG_INTERVAL: u32 = 256;

/// A `LedgerHashes` object's type.
const LEDGER_HASHES: u16 = 0x0068;

// The fields a list has (rippled's `ledger_entries.macro`).
const LEDGER_ENTRY_TYPE: FieldId = FieldId::new(Type::UInt16, 1);
const LAST_LEDGER_SEQUENCE: FieldId = FieldId::new(Type::UInt32, 27);
const HASHES: FieldId = FieldId::new(Type::Vector256, 2);

/// The key of the list of the 256 ledgers before a ledger.
#[must_use]
pub fn recent_key() -> [u8; 32] {
    sha512_half(&[0, b's'])
}

/// The key of the list that holds the hash of flag ledger `ledger`: one
/// for each 65,536 ledgers.
#[must_use]
pub fn flag_key(ledger: u32) -> [u8; 32] {
    let [a, b, c, d] = (ledger >> 16).to_be_bytes();
    sha512_half(&[0, b's', a, b, c, d])
}

/// Whether ledger `ledger` is a flag ledger.
#[must_use]
pub const fn is_flag(ledger: u32) -> bool {
    ledger.is_multiple_of(FLAG_INTERVAL)
}

/// The newest flag ledger at or below `ledger`.
#[must_use]
pub const fn flag_at_or_below(ledger: u32) -> u32 {
    ledger - ledger % FLAG_INTERVAL
}

/// The oldest flag ledger above `ledger`, if the index has one.
#[must_use]
pub const fn flag_above(ledger: u32) -> Option<u32> {
    flag_at_or_below(ledger).checked_add(FLAG_INTERVAL)
}

/// A skip list as its object says: its newest ledger and the hashes it
/// holds, oldest first, each [`FLAG_INTERVAL`] apart for a flag ledgers'
/// list and one apart for the recent ledgers'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipList {
    /// The index of the newest ledger it holds.
    pub last: u32,
    /// The hashes, oldest first.
    pub hashes: Vec<[u8; 32]>,
}

impl SkipList {
    /// The list in `object`, a `LedgerHashes` object as the state holds it:
    /// its newest ledger and at most [`SKIP_HASHES`] hashes. Fields it does
    /// not read are left as they are; the object is canonical, or nothing.
    ///
    /// # Errors
    ///
    /// It is no such object.
    pub fn parse(object: &[u8]) -> Result<Self, DecodeError> {
        let mut kind = None;
        let mut last = None;
        let mut hashes = None;
        for field in Fields::new(object) {
            let field = field?;
            if field.id == LEDGER_ENTRY_TYPE {
                kind = Some(field.uint16()?);
            } else if field.id == LAST_LEDGER_SEQUENCE {
                last = Some(field.uint32()?);
            } else if field.id == HASHES {
                hashes = Some(field.vector256()?);
            }
        }
        if kind != Some(LEDGER_HASHES) {
            return Err(DecodeError::Value);
        }
        let (Some(last), Some(hashes)) = (last, hashes) else {
            return Err(DecodeError::Field);
        };
        if hashes.len() > SKIP_HASHES {
            return Err(DecodeError::Value);
        }
        Ok(Self {
            last,
            hashes: hashes.to_vec(),
        })
    }

    /// The hash of ledger `ledger` it holds, if it holds that ledger, its
    /// hashes `step` ledgers apart: 1 for the recent ledgers' list,
    /// [`FLAG_INTERVAL`] for a flag ledgers'.
    #[must_use]
    pub fn hash_of(&self, ledger: u32, step: u32) -> Option<[u8; 32]> {
        if step == 0 || ledger > self.last || !(self.last - ledger).is_multiple_of(step) {
            return None;
        }
        let back = usize::try_from((self.last - ledger) / step).ok()?;
        let at = self.hashes.len().checked_sub(back.checked_add(1)?)?;
        self.hashes.get(at).copied()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;
    use crate::{from_hex, hex_upper};

    fn data() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/data/ledger-objects.json")).unwrap()
    }

    fn object(data: &serde_json::Value, label: &str) -> serde_json::Value {
        (data["objects"].as_array().unwrap().iter())
            .find(|object| object["label"] == label)
            .unwrap()
            .clone()
    }

    // The keys and the lists of a mainnet ledger, as rippled wrote them.
    #[test]
    fn mainnet_s_lists_read_as_rippled_wrote_them() {
        let data = data();
        let header = crate::header::LedgerHeader::from_bytes(
            &from_hex(data["ledger_data"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let recent = object(&data, "LedgerHashes (recent)");
        assert_eq!(
            hex_upper(&recent_key()),
            String::from(recent["index"].as_str().unwrap())
        );
        let list =
            SkipList::parse(&from_hex(recent["node_binary"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(list.hashes.len(), SKIP_HASHES);
        assert_eq!(list.last, header.sequence - 1);
        assert_eq!(list.hash_of(list.last, 1), Some(header.parent_hash));
        assert_eq!(list.hash_of(header.sequence, 1), None, "not itself");
        assert_eq!(list.hash_of(header.sequence - 257, 1), None, "too old");
        assert!(list.hash_of(header.sequence - 256, 1).is_some());
        let flags = object(&data, "LedgerHashes (flag ledgers)");
        let flag_ledger = u32::try_from(flags["flag_ledger"].as_u64().unwrap()).unwrap();
        assert!(is_flag(flag_ledger));
        assert_eq!(
            hex_upper(&flag_key(flag_ledger)),
            String::from(flags["index"].as_str().unwrap())
        );
        let list =
            SkipList::parse(&from_hex(flags["node_binary"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(list.last, flag_ledger);
        assert!(list.hash_of(flag_ledger, FLAG_INTERVAL).is_some());
        assert_eq!(list.hash_of(flag_ledger - 1, FLAG_INTERVAL), None);
        // The fee settings are no list.
        let fees = object(&data, "FeeSettings");
        assert_eq!(
            SkipList::parse(&from_hex(fees["node_binary"].as_str().unwrap()).unwrap()),
            Err(DecodeError::Value)
        );
    }

    #[test]
    fn flag_ledgers_are_every_256th() {
        assert_eq!(flag_at_or_below(5_000_191), 4_999_936);
        assert_eq!(flag_at_or_below(5_000_192), 5_000_192);
        assert_eq!(flag_above(5_000_192), Some(5_000_448));
        assert_eq!(flag_above(4_999_937), Some(5_000_192));
        assert_eq!(flag_above(u32::MAX), None);
        assert_eq!(flag_key(65_535 * 256), flag_key(65_535 * 256 + 255));
        assert_ne!(flag_key(65_536), flag_key(65_535));
    }

    #[test]
    fn a_list_names_only_the_ledgers_it_holds() {
        let list = SkipList {
            last: 1_000,
            hashes: vec![[1; 32], [2; 32], [3; 32]],
        };
        assert_eq!(list.hash_of(1_000, 1), Some([3; 32]));
        assert_eq!(list.hash_of(998, 1), Some([1; 32]));
        assert_eq!(list.hash_of(997, 1), None);
        assert_eq!(list.hash_of(1_001, 1), None);
        assert_eq!(list.hash_of(1_000, 0), None);
        let flags = SkipList {
            last: 1_024,
            hashes: vec![[1; 32], [2; 32]],
        };
        assert_eq!(flags.hash_of(768, FLAG_INTERVAL), Some([1; 32]));
        assert_eq!(flags.hash_of(1_000, FLAG_INTERVAL), None);
        assert_eq!(flags.hash_of(512, FLAG_INTERVAL), None);
    }
}
