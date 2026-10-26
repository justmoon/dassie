//! What a proof server's state proofs prove, and whether a new anchor is
//! on the chain of the one kept
//! (spec/designs/xrpl-validation.md#7-resets-under-a-unl), sans-IO.
//!
//! - **A transaction proof** proves a transaction and its metadata, read
//!   in binary from `tx`, by the leaf the node builds from them and the
//!   branch above it to the transaction root of its ledger's header, and **a
//!   ledger** is a proven one's when it is the anchor, or a skip list
//!   proven from the anchor holds its hash: any ledger kept is at most two
//!   lists from the anchor.
//! - **A state proof** proves a ledger's header and, for each key asked,
//!   the object its state holds under it or that it holds none, by the
//!   key's branch to the header's state root: the object is the leaf the
//!   branch starts at. The header is a proven
//!   ledger's only once its hash is: by a quorum's validations
//!   ([`crate::unl`]), or by a skip list of a proven ledger.
//! - **A kept anchor** keeps, beside its index, close time and hash, the
//!   hash of the newest flag ledger at or below it, from its own skip list
//!   of recent ledgers; one below its chain's first flag ledger keeps none.
//! - **Ancestry.** A later anchor is on the kept one's chain when its skip
//!   lists hold the kept one's hash: its list of recent ledgers, within 256
//!   ledgers; past them, its list of flag ledgers holds the kept one's flag
//!   ledger, or, for a kept anchor that keeps none, the flag ledger after
//!   it, whose own list of recent ledgers then holds the kept one. So at
//!   most two state proofs beyond the anchor's own tell, however far the
//!   chain grew, and no old history is needed. A list the state proves
//!   absent, or one that holds another hash, is another chain.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::api::{MAX_PROOF_KEYS, StateProof, TxBlobs, TxProof};
use crate::header::LedgerHeader;
use crate::records::{self, Decoded};
use crate::shamap::{BranchError, Leaf, Tree, held_object, verify, verify_transaction};
use crate::skip_list::{
    FLAG_INTERVAL, SKIP_HASHES, SkipList, flag_above, flag_at_or_below, flag_key, is_flag,
    recent_key,
};
use crate::unl::{Anchor, Relation};
use crate::{from_hex, hex_upper};

/// Why a state proof proves nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofError {
    /// The header is not one.
    Header,
    /// Of another ledger than the one asked for.
    Ledger,
    /// Not one entry for each key asked, or more than
    /// [`MAX_PROOF_KEYS`].
    Keys,
    /// A branch does not hold.
    Branch(BranchError),
    /// A transaction of another hash than asked for, or with bytes or
    /// metadata that do not read.
    Transaction,
}

/// A lie caught, by kind (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Doubt {
    /// An answer that does not read.
    Unreadable,
    /// A header that is not one, or of another ledger than asked for.
    Header,
    /// Not one entry for each key asked.
    Keys,
    /// A branch that does not reach its root.
    Branch,
    /// Validations short of a quorum, after newer manifests were asked.
    NoQuorum,
    /// More validations than are read.
    TooMany,
    /// A validated ledger older than the anchor kept, or one whose
    /// validations are no longer fresh.
    Older,
    /// A validated ledger of an older chain: a higher index that closed
    /// earlier, or another of the index that closed no later.
    OlderChain,
    /// A proven object that is no skip list, or not what its key holds.
    SkipList,
    /// A transaction of another hash than asked for, or whose bytes or
    /// metadata do not read.
    Transaction,
    /// A transaction in a ledger the anchor's chain does not hold.
    Ledger,
    /// A history the proven state proves wrong: a transaction an object
    /// names that did not change it, or that the server says its proven
    /// ledger does not hold.
    History,
}

impl Doubt {
    /// Every kind.
    pub const ALL: [Self; 12] = [
        Self::Unreadable,
        Self::Header,
        Self::Keys,
        Self::Branch,
        Self::NoQuorum,
        Self::TooMany,
        Self::Older,
        Self::OlderChain,
        Self::SkipList,
        Self::Transaction,
        Self::Ledger,
        Self::History,
    ];

    /// The kind of lie a proof that does not hold is.
    #[must_use]
    pub const fn of(error: ProofError) -> Self {
        match error {
            ProofError::Header | ProofError::Ledger => Self::Header,
            ProofError::Keys => Self::Keys,
            ProofError::Branch(_) => Self::Branch,
            ProofError::Transaction => Self::Transaction,
        }
    }
}

/// What a state proof proved: a ledger's header and hash, and each key's
/// object, or its absence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenState {
    /// The ledger's header.
    pub header: LedgerHeader,
    /// Its hash.
    pub hash: [u8; 32],
    objects: BTreeMap<[u8; 32], Option<Vec<u8>>>,
}

impl ProvenState {
    /// The object its state holds under `key`: `None` if the key was not
    /// proven, `Some(None)` if the state holds none.
    #[must_use]
    pub fn object(&self, key: &[u8; 32]) -> Option<Option<&[u8]>> {
        self.objects.get(key).map(Option::as_deref)
    }

    /// How many of the keys proven it proves absent.
    #[must_use]
    pub fn absences(&self) -> usize {
        self.objects
            .values()
            .filter(|object| object.is_none())
            .count()
    }
}

/// What `answer` proves of `keys`, each distinct, at the ledger of `at`,
/// or at whichever ledger it names when `at` is `None`: every key asked
/// has one entry, and nothing else, whose branch leads to the header's
/// state root, from the key's leaf, whose object it proves, or from an
/// empty slot or another key's leaf, which prove the key absent.
///
/// # Errors
///
/// Why it proves nothing.
pub fn prove_state(
    answer: &StateProof,
    keys: &[[u8; 32]],
    at: Option<&[u8; 32]>,
) -> Result<ProvenState, ProofError> {
    let header = LedgerHeader::from_bytes(&answer.header).map_err(|_| ProofError::Header)?;
    let hash = header.hash();
    if at.is_some_and(|at| *at != hash) {
        return Err(ProofError::Ledger);
    }
    if keys.len() > MAX_PROOF_KEYS || answer.entries.len() != keys.len() {
        return Err(ProofError::Keys);
    }
    let mut objects = BTreeMap::new();
    for entry in &answer.entries {
        if !keys.contains(&entry.key) || objects.contains_key(&entry.key) {
            return Err(ProofError::Keys);
        }
        let object = held_object(&entry.key, &entry.path);
        let leaf = object.map_or(Leaf::Absent(Tree::State), Leaf::State);
        verify(&header.account_hash, &entry.key, leaf, &entry.path).map_err(ProofError::Branch)?;
        objects.insert(entry.key, object.map(<[u8]>::to_vec));
    }
    assert_eq!(objects.len(), keys.len(), "each key asked once");
    Ok(ProvenState {
        header,
        hash,
        objects,
    })
}

/// What a transaction proof proved: the transaction with its metadata, a
/// leaf of the transaction tree of the ledger of its header. The ledger is
/// a proven one's only once its hash is ([`ledger_proof`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenTx {
    /// The ledger's header.
    pub header: LedgerHeader,
    /// Its hash.
    pub ledger_hash: [u8; 32],
    /// What the transaction and its metadata say, of the objects asked.
    pub decoded: Decoded,
}

/// What `answer` proves of the transaction `hash` with `blobs`, its bytes
/// and metadata as `tx` read them: the bytes hash to it, and the leaf built
/// from them, under that hash, with the branch above it leads to the
/// transaction root of the header the answer gives; read as a record of
/// that ledger, with the threads of the objects of `keys`. Bytes that are
/// not the ledger's fail the branch.
///
/// # Errors
///
/// Why it proves nothing.
pub fn prove_transaction(
    answer: &TxProof,
    blobs: &TxBlobs,
    hash: &[u8; 32],
    keys: &[[u8; 32]],
) -> Result<ProvenTx, ProofError> {
    let header = LedgerHeader::from_bytes(&answer.header).map_err(|_| ProofError::Header)?;
    if crate::codec::hash(&blobs.tx) != *hash {
        return Err(ProofError::Transaction);
    }
    verify_transaction(
        &header.transaction_hash,
        &blobs.tx,
        &blobs.meta,
        &answer.path,
    )
    .map_err(ProofError::Branch)?;
    let decoded = records::transaction(&blobs.tx, &blobs.meta, header.sequence, keys)
        .map_err(|_| ProofError::Transaction)?;
    assert_eq!(decoded.record.hash, *hash, "the hash of its own bytes");
    Ok(ProvenTx {
        ledger_hash: header.hash(),
        header,
        decoded,
    })
}

/// Whether a ledger is a proven one's, against an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerProof {
    /// It is: the anchor, or a ledger its skip lists hold.
    Holds,
    /// It is not: a list holds another hash for its index, or none.
    Wrong,
    /// After the anchor: only a later anchor can tell.
    Later,
    /// To tell, prove the object of `key` at the ledger of `at`.
    Prove {
        /// The ledger, by its hash: the anchor, or a flag ledger its list
        /// proved.
        at: [u8; 32],
        /// The skip list's key.
        key: [u8; 32],
    },
}

/// The hash of a ledger of an anchor's chain, by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerHash {
    /// The anchor's, or the one its skip lists hold: a proven ledger's.
    Proven([u8; 32]),
    /// The chain holds none of that index: a list proven absent, or one
    /// that holds no hash for it, as for a ledger before the chain began.
    None,
    /// After the anchor: only a later anchor can tell.
    Later,
    /// To tell, prove the object of `key` at the ledger of `at`.
    Prove {
        /// The ledger, by its hash: the anchor, or a flag ledger its list
        /// proved.
        at: [u8; 32],
        /// The skip list's key.
        key: [u8; 32],
    },
}

/// The hash of the ledger of `index` on the chain of `anchor`, from the
/// skip lists in `lists`, or which to prove next: its list of recent
/// ledgers within 256 ledgers, and past them its list of flag ledgers,
/// which holds the flag ledger at or after the ledger, and that flag
/// ledger's own list of recent ledgers. At most two beyond the anchor's
/// own.
#[must_use]
pub fn ledger_hash(anchor: &Anchor, index: u32, lists: &ProvenLists) -> LedgerHash {
    let found = |found: Option<[u8; 32]>| found.map_or(LedgerHash::None, LedgerHash::Proven);
    let list = |at: [u8; 32], key: [u8; 32]| match lists.get(&(at, key)) {
        None => Err(LedgerHash::Prove { at, key }),
        Some(None) => Err(LedgerHash::None),
        Some(Some(list)) => Ok(list),
    };
    if index == anchor.index {
        return LedgerHash::Proven(anchor.hash);
    }
    if index > anchor.index {
        return LedgerHash::Later;
    }
    if u64::from(anchor.index - index) <= SKIP_HASHES as u64 {
        return match list(anchor.hash, recent_key()) {
            Ok(recent) => found(recent.hash_of(index, 1)),
            Err(next) => next,
        };
    }
    // Past the recent list: the flag ledger at or after it, below the
    // anchor, as the anchor is more than 256 ledgers on.
    let flag = if is_flag(index) {
        index
    } else {
        match flag_above(index) {
            Some(flag) => flag,
            None => return LedgerHash::None,
        }
    };
    let flag_hash = match list(anchor.hash, flag_key(flag)) {
        Ok(flags) => match flags.hash_of(flag, FLAG_INTERVAL) {
            Some(flag_hash) => flag_hash,
            None => return LedgerHash::None,
        },
        Err(next) => return next,
    };
    if flag == index {
        return LedgerHash::Proven(flag_hash);
    }
    match list(flag_hash, recent_key()) {
        Ok(recent) => found(recent.hash_of(index, 1)),
        Err(next) => next,
    }
}

/// Whether the ledger of `index` is the one of `hash` on the chain of
/// `anchor`, from the skip lists in `lists`, or which to prove next
/// ([`ledger_hash`]).
#[must_use]
pub fn ledger_proof(
    anchor: &Anchor,
    index: u32,
    hash: &[u8; 32],
    lists: &ProvenLists,
) -> LedgerProof {
    match ledger_hash(anchor, index, lists) {
        LedgerHash::Proven(proven) if proven == *hash => LedgerProof::Holds,
        LedgerHash::Proven(_) | LedgerHash::None => LedgerProof::Wrong,
        LedgerHash::Later => LedgerProof::Later,
        LedgerHash::Prove { at, key } => LedgerProof::Prove { at, key },
    }
}

/// What a new anchor is, against the one kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judged {
    /// Current: the first, the same, or past a hint without a hash.
    Take,
    /// Older than the kept: never current.
    Older,
    /// Of an older chain: ignored where the network resets, a fork
    /// elsewhere.
    Ignore,
    /// Later: current if on the kept one's chain.
    Later,
    /// On another chain the kept one is not on: a reset, where the network
    /// resets.
    Reset,
}

/// Where `new` stands against `kept`
/// (spec/designs/xrpl-validation.md#7-resets-under-a-unl). Along one chain
/// index and close time both grow: a lower ledger that closed later is of
/// a chain begun since, and a higher one that closed no later, or another
/// of the index that closed no later, is of an older chain.
#[must_use]
pub fn judge(kept: &Anchor, new: &Anchor) -> Judged {
    match new.relation(kept) {
        Relation::Same => Judged::Take,
        Relation::Older => Judged::Older,
        Relation::Newer => Judged::Later,
        Relation::Diverged if new.index <= kept.index && new.close_time > kept.close_time => {
            Judged::Reset
        }
        Relation::Diverged => Judged::Ignore,
    }
}

/// An anchor kept: the ledger, and the newest flag ledger at or below it
/// with its hash, if its chain has one there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeptAnchor {
    /// The ledger.
    pub anchor: Anchor,
    /// The newest flag ledger at or below it, and its hash: none for one
    /// below its chain's first flag ledger.
    pub flag: Option<(u32, [u8; 32])>,
}

impl KeptAnchor {
    /// `anchor` kept, with its flag ledger from `recent`, its own list of
    /// recent ledgers, proven with it: `None` for a ledger without one, the
    /// first of its chain.
    #[must_use]
    pub fn of(anchor: Anchor, recent: Option<&SkipList>) -> Self {
        let flag = flag_at_or_below(anchor.index);
        let hash = if flag == anchor.index {
            Some(anchor.hash)
        } else {
            recent.and_then(|list| list.hash_of(flag, 1))
        };
        Self {
            anchor,
            flag: hash.map(|hash| (flag, hash)),
        }
    }

    /// It as the account's state keeps it.
    #[must_use]
    pub fn state(&self) -> Value {
        let anchor = &self.anchor;
        json!({
            "index": anchor.index,
            "close_time": anchor.close_time,
            "hash": hex_upper(&anchor.hash),
            "flag": self.flag.map(|(index, hash)| json!([index, hex_upper(&hash)])),
        })
    }

    /// What [`Self::state`] wrote; `None` if it is not that, or names a
    /// flag ledger that is none, or above the anchor.
    #[must_use]
    pub fn restore(value: &Value) -> Option<Self> {
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        let hash =
            |value: &Value| -> Option<[u8; 32]> { from_hex(value.as_str()?)?.try_into().ok() };
        let anchor = Anchor {
            index: number(value.get("index")?)?,
            close_time: number(value.get("close_time")?)?,
            hash: hash(value.get("hash")?)?,
        };
        let flag = match value.get("flag")? {
            Value::Null => None,
            flag => Some((number(flag.get(0)?)?, hash(flag.get(1)?)?)),
        };
        let kept = Self { anchor, flag };
        kept.is_consistent().then_some(kept)
    }

    /// Whether its flag ledger is the newest at or below it, and its own
    /// hash if it is one.
    fn is_consistent(&self) -> bool {
        let index = self.anchor.index;
        self.flag.is_none_or(|(flag, hash)| {
            flag == flag_at_or_below(index) && (flag != index || hash == self.anchor.hash)
        })
    }

    /// Asserts what holds of it.
    pub fn check_invariants(&self) {
        assert!(
            self.is_consistent(),
            "a kept anchor's flag ledger is its newest"
        );
    }
}

/// Whether a later anchor is on the kept one's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ancestry {
    /// It is: its skip lists hold the kept anchor.
    OnChain,
    /// It is not: a list proven absent, or holding another hash.
    OffChain,
    /// To tell, prove the object of `key` at the ledger of `at`.
    Prove {
        /// The ledger, by its hash: the new anchor, or a flag ledger its
        /// list proved.
        at: [u8; 32],
        /// The skip list's key.
        key: [u8; 32],
    },
}

/// Skip lists proven so far, each by the ledger it was proven at and its
/// key: `None` for one proven absent.
pub type ProvenLists = BTreeMap<([u8; 32], [u8; 32]), Option<SkipList>>;

/// Whether `new`, a higher ledger that closed later than `kept`, is on
/// its chain, from the skip lists in `lists`, or which to prove next. At
/// most two beyond the new anchor's own list of recent ledgers.
#[must_use]
pub fn ancestry(kept: &KeptAnchor, new: &Anchor, lists: &ProvenLists) -> Ancestry {
    assert!(
        new.index > kept.anchor.index,
        "ancestry is asked of a later anchor"
    );
    let list = |at: [u8; 32], key: [u8; 32]| match lists.get(&(at, key)) {
        None => Err(Ancestry::Prove { at, key }),
        Some(None) => Err(Ancestry::OffChain),
        Some(Some(list)) => Ok(list),
    };
    let holds = |list: &SkipList, index: u32, step: u32, hash: [u8; 32]| {
        if list.hash_of(index, step) == Some(hash) {
            Ancestry::OnChain
        } else {
            Ancestry::OffChain
        }
    };
    let kept_ledger = kept.anchor;
    let behind = u64::from(new.index - kept_ledger.index);
    if behind <= SKIP_HASHES as u64 {
        return match list(new.hash, recent_key()) {
            Ok(recent) => holds(recent, kept_ledger.index, 1, kept_ledger.hash),
            Err(next) => next,
        };
    }
    // Past the recent list: its flag ledger, or the one after it, which is
    // below the new anchor, as the new anchor is more than 256 ledgers on.
    let (flag, known) = match kept.flag {
        Some((flag, hash)) => (flag, Some(hash)),
        None => match flag_above(kept_ledger.index) {
            Some(flag) => (flag, None),
            None => return Ancestry::OffChain,
        },
    };
    let flag_hash = match list(new.hash, flag_key(flag)) {
        Ok(flags) => match flags.hash_of(flag, FLAG_INTERVAL) {
            Some(hash) => hash,
            None => return Ancestry::OffChain,
        },
        Err(next) => return next,
    };
    match known {
        Some(hash) if hash == flag_hash => Ancestry::OnChain,
        Some(_) => Ancestry::OffChain,
        // The flag ledger after the kept anchor holds it among the 256
        // before it.
        None => match list(flag_hash, recent_key()) {
            Ok(recent) => holds(recent, kept_ledger.index, 1, kept_ledger.hash),
            Err(next) => next,
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {

    use super::*;

    fn hash(n: u32) -> [u8; 32] {
        let mut out = [0; 32];
        out[..4].copy_from_slice(&n.to_be_bytes());
        out[31] = 1;
        out
    }

    fn anchor(index: u32) -> Anchor {
        Anchor {
            index,
            close_time: index * 4,
            hash: hash(index),
        }
    }

    /// The recent list of the ledger of `index`, of a chain that began at
    /// `first`.
    fn recent(index: u32, first: u32) -> SkipList {
        let from = index.saturating_sub(256).max(first);
        SkipList {
            last: index - 1,
            hashes: (from..index).map(hash).collect(),
        }
    }

    /// The flag list holding flag ledger `flag` as of the ledger of
    /// `index`: the flag ledgers of its 65,536 below `index`, of a chain
    /// that began at `first`.
    fn flags(flag: u32, index: u32, first: u32) -> SkipList {
        let group = flag & !0xFFFF;
        let last = flag_at_or_below((index - 1).min(group + 0xFFFF));
        let start = group.max(first.next_multiple_of(256));
        SkipList {
            last,
            hashes: (start..=last).step_by(256).map(hash).collect(),
        }
    }

    #[test]
    fn a_kept_anchor_keeps_its_newest_flag_ledger() {
        let kept = KeptAnchor::of(anchor(5_000_100), Some(&recent(5_000_100, 4_999_000)));
        assert_eq!(kept.flag, Some((4_999_936, hash(4_999_936))));
        let own = KeptAnchor::of(anchor(4_999_936), Some(&recent(4_999_936, 4_999_000)));
        assert_eq!(own.flag, Some((4_999_936, hash(4_999_936))));
        // A chain that began after its flag ledger has none there.
        let young = KeptAnchor::of(anchor(5_000_100), Some(&recent(5_000_100, 5_000_000)));
        assert_eq!(young.flag, None);
        assert_eq!(KeptAnchor::of(anchor(5_000_000), None).flag, None);
        for kept in [kept, own, young] {
            assert_eq!(KeptAnchor::restore(&kept.state()), Some(kept));
        }
        let mut wrong = kept.state();
        wrong["flag"][0] = 4_999_680.into();
        assert_eq!(KeptAnchor::restore(&wrong), None);
    }

    // spec/designs/xrpl-validation.md#7-resets-under-a-unl: within 256
    // ledgers the new anchor's recent list tells; past them its flag list,
    // and, for a kept anchor below its chain's first flag ledger, that flag
    // ledger's own recent list.
    #[test]
    fn ancestry_takes_at_most_two_more_proofs() {
        let first = 5_000_000;
        let kept = KeptAnchor::of(anchor(5_000_100), Some(&recent(5_000_100, first)));
        assert_eq!(kept.flag, None);
        let mut lists = ProvenLists::new();
        // Near: the recent list.
        let near = anchor(5_000_300);
        assert_eq!(
            ancestry(&kept, &near, &lists),
            Ancestry::Prove {
                at: near.hash,
                key: recent_key()
            }
        );
        lists.insert((near.hash, recent_key()), Some(recent(near.index, first)));
        assert_eq!(ancestry(&kept, &near, &lists), Ancestry::OnChain);
        // Far: the flag list, then the flag ledger's recent list.
        let far = anchor(5_100_000);
        let flag = 5_000_192;
        lists.insert((far.hash, recent_key()), Some(recent(far.index, first)));
        assert_eq!(
            ancestry(&kept, &far, &lists),
            Ancestry::Prove {
                at: far.hash,
                key: flag_key(flag)
            }
        );
        lists.insert(
            (far.hash, flag_key(flag)),
            Some(flags(flag, far.index, first)),
        );
        assert_eq!(
            ancestry(&kept, &far, &lists),
            Ancestry::Prove {
                at: hash(flag),
                key: recent_key()
            }
        );
        lists.insert((hash(flag), recent_key()), Some(recent(flag, first)));
        assert_eq!(ancestry(&kept, &far, &lists), Ancestry::OnChain);
        // Another chain's lists hold other hashes, or none.
        let mut other = lists.clone();
        other.insert((hash(flag), recent_key()), Some(recent(flag, 5_000_150)));
        assert_eq!(ancestry(&kept, &far, &other), Ancestry::OffChain);
        other.insert((far.hash, flag_key(flag)), None);
        assert_eq!(ancestry(&kept, &far, &other), Ancestry::OffChain);
        other.insert(
            (near.hash, recent_key()),
            Some(recent(near.index, 5_000_200)),
        );
        assert_eq!(ancestry(&kept, &near, &other), Ancestry::OffChain);
        // A kept anchor with a flag ledger needs the flag list alone.
        let older = KeptAnchor::of(anchor(5_000_300), Some(&recent(5_000_300, first)));
        assert_eq!(older.flag, Some((flag, hash(flag))));
        assert_eq!(ancestry(&older, &far, &lists), Ancestry::OnChain);
        let mut forged = older;
        forged.flag = Some((flag, [9; 32]));
        assert_eq!(ancestry(&forged, &far, &lists), Ancestry::OffChain);
    }

    // A ledger's hash by its index, as a state proof at it needs: the
    // anchor's own, its recent list within 256 ledgers, and past them its
    // flag list and the flag ledger's recent list; none before the chain
    // began, and only a later anchor tells a later ledger.
    #[test]
    fn a_ledger_s_hash_is_at_most_two_lists_from_the_anchor() {
        let first = 5_000_000;
        let anchor = anchor(5_100_000);
        let mut lists = ProvenLists::new();
        assert_eq!(
            ledger_hash(&anchor, anchor.index, &lists),
            LedgerHash::Proven(anchor.hash)
        );
        assert_eq!(
            ledger_hash(&anchor, anchor.index + 1, &lists),
            LedgerHash::Later
        );
        let near = anchor.index - 10;
        assert_eq!(
            ledger_hash(&anchor, near, &lists),
            LedgerHash::Prove {
                at: anchor.hash,
                key: recent_key()
            }
        );
        lists.insert(
            (anchor.hash, recent_key()),
            Some(recent(anchor.index, first)),
        );
        assert_eq!(
            ledger_hash(&anchor, near, &lists),
            LedgerHash::Proven(hash(near))
        );
        assert_eq!(
            ledger_proof(&anchor, near, &hash(near), &lists),
            LedgerProof::Holds
        );
        assert_eq!(
            ledger_proof(&anchor, near, &[9; 32], &lists),
            LedgerProof::Wrong
        );
        let far = 5_000_100;
        let flag = 5_000_192;
        assert_eq!(
            ledger_hash(&anchor, far, &lists),
            LedgerHash::Prove {
                at: anchor.hash,
                key: flag_key(flag)
            }
        );
        lists.insert(
            (anchor.hash, flag_key(flag)),
            Some(flags(flag, anchor.index, first)),
        );
        assert_eq!(
            ledger_hash(&anchor, flag, &lists),
            LedgerHash::Proven(hash(flag))
        );
        lists.insert((hash(flag), recent_key()), Some(recent(flag, first)));
        assert_eq!(
            ledger_hash(&anchor, far, &lists),
            LedgerHash::Proven(hash(far))
        );
        // Before the chain began.
        assert_eq!(ledger_hash(&anchor, first - 1, &lists), LedgerHash::None);
    }
}
