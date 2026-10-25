//! Entry payments: what a node new to us paid on the ledger before it
//! peered, held for it until it peers (spec/decisions/pay-to-enter.md).
//!
//! A downlink is a free receiver until it paid us, and free receivers take
//! at most half of the downlinks' places
//! (spec/decisions/free-receivers-half.md). A newcomer that would pay can
//! show it only by paying first: a Payment to our account with the scheme's
//! memo naming its node, as a settlement from a peer is, of at least one
//! largest packet. The schemes on the XRP Ledger follow every transaction
//! that touches the account, so they see it arrive, proven, and hold it
//! here, off the books, rather than taking it for the owner's deposit:
//!
//! - **Bound to its payer.** It counts only for the node its memo names, and
//!   only while that node asks to peer naming the address it came from, as
//!   a settlement counts only from a peer's own address. The request is
//!   authenticated by the node's key, so no one else can use it.
//! - **Booked once.** Once the node peers, from that address, it is booked
//!   as its settlement, under the transaction's hash, and leaves the table;
//!   one never claimed within [`ENTRY_HOLD_LEDGERS`] is the owner's deposit.
//!   Either way the books take it once: the scheme's cursor reads each
//!   transaction once, and the table keeps each hash once.
//! - **Bounded.** At most [`MAX_ENTRIES`] at once, each of at least the
//!   least settlement; past that, a payment is the owner's deposit, as one
//!   from any address no peer gave.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_settlement::EntryPayment;
use serde_json::{Value, json};

use crate::keys::AccountId;
use crate::{from_hex, hex_upper};

/// Most entry payments held at once. Filling the table takes one largest
/// packet each, which the owner keeps once it expires.
pub const MAX_ENTRIES: usize = 256;

/// How long an entry payment is held for its node, in validated ledgers
/// from when the scheme read it: about an hour at four seconds a ledger. A
/// newcomer asks right after it paid; the hold covers a restart or two on
/// either side, and holding longer would let fewer payments fill the table
/// for longer.
pub const ENTRY_HOLD_LEDGERS: u32 = 900;

/// One entry payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldEntry {
    /// The node its memo names.
    pub node: Name,
    /// The address it came from.
    pub from: AccountId,
    /// What it delivered, in internal units: what the books take.
    pub amount: u64,
    /// Its transaction's hash, which books it once.
    pub hash: [u8; 32],
    /// The validated ledger at which it goes to the owner.
    pub until: u32,
}

/// The entry payments a scheme holds, off its books.
#[derive(Clone, Debug, Default)]
pub struct Entries {
    /// The least an entry payment is, in internal units: one largest
    /// packet. `None`: the scheme takes none.
    minimum: Option<u64>,
    /// In the order they arrived.
    held: Vec<HeldEntry>,
}

impl Entries {
    /// Takes entry payments of at least `minimum`, in internal units.
    pub(crate) fn take_from(&mut self, minimum: u64) {
        assert!(minimum > 0, "an entry payment is at least one unit");
        self.minimum = Some(minimum);
    }

    /// Whether the scheme takes entry payments.
    pub(crate) const fn taken(&self) -> bool {
        self.minimum.is_some()
    }

    /// What the table holds, in internal units: never on the books.
    pub(crate) fn total(&self) -> u64 {
        self.held.iter().map(|entry| entry.amount).sum()
    }

    /// Holds a payment of `amount` from `from`, naming `node`, read at
    /// validated ledger `validated`, unless it is less than an entry
    /// payment, the scheme takes none, or the table is full. Returns
    /// whether it holds it; one it does not is the owner's deposit.
    pub(crate) fn hold(
        &mut self,
        node: Name,
        from: AccountId,
        amount: u64,
        hash: [u8; 32],
        validated: u32,
    ) -> bool {
        let Some(minimum) = self.minimum else {
            return false;
        };
        // The cursor reads each transaction once; one held already is
        // never held, or booked, again.
        if self.held.iter().any(|entry| entry.hash == hash) {
            return true;
        }
        if amount < minimum
            || self.held.len() >= MAX_ENTRIES
            || self.total().saturating_add(amount) > MAX_TOTAL
        {
            return false;
        }
        self.held.push(HeldEntry {
            node,
            from,
            amount,
            hash,
            until: validated.saturating_add(ENTRY_HOLD_LEDGERS),
        });
        true
    }

    /// Whether one is held from `node`, from `from`.
    pub(crate) fn entry(&self, node: Name, from: Option<AccountId>) -> EntryPayment {
        if !self.taken() {
            return EntryPayment::NotTaken;
        }
        let held = from.is_some_and(|from| {
            self.held
                .iter()
                .any(|entry| entry.node == node && entry.from == from)
        });
        if held {
            EntryPayment::Held
        } else {
            EntryPayment::Missing
        }
    }

    /// Takes every one held from `node`, from `from`, out of the table.
    pub(crate) fn take(&mut self, node: Name, from: AccountId) -> Vec<HeldEntry> {
        self.take_where(|entry| entry.node == node && entry.from == from)
    }

    /// Takes every one whose hold is over at validated ledger `validated`
    /// out of the table.
    pub(crate) fn expire(&mut self, validated: u32) -> Vec<HeldEntry> {
        self.take_where(|entry| entry.until <= validated)
    }

    fn take_where(&mut self, taken: impl Fn(&HeldEntry) -> bool) -> Vec<HeldEntry> {
        let (out, kept) = core::mem::take(&mut self.held)
            .into_iter()
            .partition(|entry| taken(entry));
        self.held = kept;
        out
    }

    /// Forgets them all: the account is gone, and what they held with it.
    pub(crate) fn clear(&mut self) {
        self.held.clear();
    }

    /// Mixes what is kept into a scheme's fingerprint.
    pub(crate) fn fingerprint(&self, add: &mut impl FnMut(u64)) {
        add(self.held.len() as u64);
        for entry in &self.held {
            for word in entry.hash.as_chunks::<8>().0 {
                add(u64::from_le_bytes(*word));
            }
            add(entry.amount);
            add(u64::from(entry.until));
        }
    }

    /// As a scheme's state keeps them.
    pub(crate) fn value(&self) -> Value {
        self.held
            .iter()
            .map(|entry| {
                json!([
                    entry.node.as_str(),
                    entry.from.to_address(),
                    entry.amount,
                    hex_upper(&entry.hash),
                    entry.until,
                ])
            })
            .collect()
    }

    /// Reads them back from a scheme's state: none in a state written
    /// before the scheme held any. More than the table holds, or a payment
    /// twice, is a state no scheme wrote.
    pub(crate) fn restore(&mut self, state: &Value) -> Option<()> {
        let held = match state.get("entries") {
            None => return Some(()),
            Some(held) => held.as_array()?,
        };
        if held.len() > MAX_ENTRIES {
            return None;
        }
        for entry in held {
            let node = Name::new(entry.get(0)?.as_str()?).ok()?;
            let from = AccountId::parse(entry.get(1)?.as_str()?)?;
            let amount = entry.get(2)?.as_u64().filter(|amount| *amount > 0)?;
            let hash: [u8; 32] = from_hex(entry.get(3)?.as_str()?)?.try_into().ok()?;
            let until = u32::try_from(entry.get(4)?.as_u64()?).ok()?;
            if self.held.iter().any(|held| held.hash == hash)
                || self.total().checked_add(amount)? > MAX_TOTAL
            {
                return None;
            }
            self.held.push(HeldEntry {
                node,
                from,
                amount,
                hash,
                until,
            });
        }
        Some(())
    }

    /// Asserts its bounds.
    pub(crate) fn check_invariants(&self) {
        assert!(self.held.len() <= MAX_ENTRIES, "the entries' bound");
        assert!(self.total() <= MAX_TOTAL, "the books could take them all");
        assert!(
            self.held.iter().all(|entry| entry.amount > 0),
            "an entry payment pays"
        );
        let mut hashes = BTreeSet::new();
        assert!(
            self.held.iter().all(|entry| hashes.insert(entry.hash)),
            "each payment held once"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn node(n: u8) -> Name {
        Name::new(&alloc::format!("node{n}")).unwrap()
    }

    // spec/decisions/pay-to-enter.md: a scheme told no least amount takes
    // none; one that takes them holds what is at least that, bound to its
    // node and address, each payment once, and no more than the table's
    // bound. What it holds goes back and forth through its state as it was.
    #[test]
    fn the_table_holds_what_counts_once_and_within_its_bound() {
        let mut entries = Entries::default();
        let (from, other) = (AccountId([1; 20]), AccountId([2; 20]));
        assert!(!entries.hold(node(1), from, 10, [1; 32], 5));
        assert_eq!(entries.entry(node(1), Some(from)), EntryPayment::NotTaken);
        entries.take_from(10);
        assert!(!entries.hold(node(1), from, 9, [1; 32], 5), "too little");
        assert!(entries.hold(node(1), from, 10, [1; 32], 5));
        assert!(entries.hold(node(1), from, 10, [1; 32], 5), "held already");
        assert_eq!(entries.total(), 10);
        assert_eq!(entries.entry(node(1), Some(from)), EntryPayment::Held);
        assert_eq!(entries.entry(node(1), Some(other)), EntryPayment::Missing);
        assert_eq!(entries.entry(node(2), Some(from)), EntryPayment::Missing);
        assert_eq!(entries.entry(node(1), None), EntryPayment::Missing);
        let mut kept = Entries::default();
        kept.restore(&json!({ "entries": entries.value() }))
            .unwrap();
        assert_eq!(kept.held, entries.held);
        let value = entries.value();
        let twice = json!({ "entries": [value[0], value[0]] });
        assert!(Entries::default().restore(&twice).is_none());
        for n in 1..MAX_ENTRIES {
            let mut hash = [0; 32];
            hash[..8].copy_from_slice(&(n as u64).to_be_bytes());
            assert!(entries.hold(node(2), other, 10, hash, 5));
        }
        assert!(!entries.hold(node(3), other, 10, [3; 32], 5), "full");
        entries.check_invariants();
        assert_eq!(entries.take(node(1), from).len(), 1);
        assert!(entries.expire(5 + ENTRY_HOLD_LEDGERS - 1).is_empty());
        assert_eq!(
            entries.expire(5 + ENTRY_HOLD_LEDGERS).len(),
            MAX_ENTRIES - 1
        );
        assert_eq!(entries.total(), 0);
    }
}
