//! The fake ledger's chain of closed ledgers
//! (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
//! each with a real header, whose hash binds its parent's, the root of its
//! transaction tree and the root of its state tree, in which every object
//! is under its key in rippled's binary form; the skip lists of ledger
//! hashes as rippled keeps them; and who signed each.
//!
//! The state tree follows the model: as a ledger closes, every object is
//! written again from what the fake holds, and only those that changed are
//! hashed again. What a test sets by hand between closes, outside any
//! transaction, shows in the next ledger's tree, naming no transaction.
//!
//! What is kept is bounded: headers for [`MAX_KEPT_LEDGERS`]; the state of
//! each of the last [`MAX_STATE_LEDGERS`], and of each flag ledger among
//! those kept, so that any ledger kept is two state proofs from the
//! newest; and who signed each of the last [`MAX_VALIDATED_LEDGERS`] and
//! each flag ledger, whose validations a lying server replays as an old
//! anchor's. An honest server gives only those of the ledgers that closed
//! within [`VALIDATIONS_HELD_S`], as rippled holds them; the skip list
//! reaches anything older.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::FEE_BASE;
use super::FakeLedger;
use super::objects::{self, Object};
use super::shamap::ShaMap;
use super::signers::signer_list_index;
use super::tokens::line_index;
use super::validators::SignerKey;
use crate::MAX_DROPS;
use crate::header::LedgerHeader;
use crate::shamap::Tree;

/// The most ledgers whose headers are kept: days of ledgers, the history
/// bound every proof server keeps ([`crate::account::HISTORY_BOUND`]) back
/// from the newest, and no more, so that what lies beyond it is history
/// the fake no longer has, as a proof server's soon is.
pub const MAX_KEPT_LEDGERS: usize = crate::account::HISTORY_BOUND as usize + 1;
const _: () = assert!(
    MAX_KEPT_LEDGERS as u64 >= crate::account::HISTORY_BOUND as u64,
    "the fake keeps the history every proof server keeps"
);
/// The most recent ledgers whose state is kept, for proofs of their state:
/// more than close, at a ledger every three seconds, while a validation
/// stays fresh and a minute after, so that a lie of an old anchor finds
/// one between the flag ledgers.
pub const MAX_STATE_LEDGERS: usize = 128;
/// The most recent ledgers whose signers are kept, beside the flag
/// ledgers': more than an honest server serves validations of.
pub const MAX_VALIDATED_LEDGERS: usize = 256;
/// How long rippled holds a ledger's validations, in seconds: an honest
/// server serves those of the ledgers that closed this long before the
/// newest, or since.
pub const VALIDATIONS_HELD_S: u32 = 600;
use crate::skip_list::FLAG_INTERVAL;
use crate::skip_list::SKIP_HASHES;
/// How finely close times are rounded, in seconds, as on mainnet.
pub(super) const CLOSE_TIME_RESOLUTION_S: u8 = 10;

/// A closed ledger: its header and hash, and while it is recent, its state
/// and who signed it.
#[derive(Clone, Debug)]
pub(super) struct Sealed {
    pub header: LedgerHeader,
    pub hash: [u8; 32],
    /// Its state tree, for the last [`MAX_STATE_LEDGERS`] and flag
    /// ledgers.
    pub state: Option<ShaMap>,
    /// The keys its validators signed it with, for the last
    /// [`MAX_VALIDATED_LEDGERS`] and flag ledgers.
    pub signers: Vec<SignerKey>,
    /// Its validations, once asked for.
    pub validations: Option<Vec<Vec<u8>>>,
}

/// The chain the fake keeps.
#[derive(Clone, Debug)]
pub(super) struct Chain {
    /// The state tree as the last ledger closed.
    state: ShaMap,
    /// The last [`SKIP_HASHES`] ledgers' hashes, oldest first, and the
    /// index of the last.
    recent: (u32, Vec<[u8; 32]>),
    /// Flag ledgers' hashes, by their index shifted down 16 bits: the last
    /// one's index and the hashes, oldest first.
    flags: BTreeMap<u32, (u32, Vec<[u8; 32]>)>,
    /// The ledgers kept, by index.
    ledgers: BTreeMap<u32, Sealed>,
    /// The index of each ledger kept, by its hash.
    by_hash: BTreeMap<[u8; 32], u32>,
}

impl Chain {
    pub fn new() -> Self {
        Self {
            state: ShaMap::new(Tree::State),
            recent: (0, Vec::new()),
            flags: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            by_hash: BTreeMap::new(),
        }
    }

    /// The last ledger closed.
    pub fn latest(&self) -> Option<&Sealed> {
        self.ledgers.last_key_value().map(|(_, sealed)| sealed)
    }

    /// The ledger of `index`, if it is kept.
    pub fn get(&self, index: u32) -> Option<&Sealed> {
        self.ledgers.get(&index)
    }

    /// The ledger of `index`, to cache what is asked of it.
    pub fn get_mut(&mut self, index: u32) -> Option<&mut Sealed> {
        self.ledgers.get_mut(&index)
    }

    /// The index of the ledger of `hash`, if it is kept.
    pub fn index_of(&self, hash: &[u8; 32]) -> Option<u32> {
        self.by_hash.get(hash).copied()
    }

    /// The newest ledger kept with its state and its signers that closed
    /// before `close_time`.
    pub fn closed_before(&self, close_time: u32) -> Option<u32> {
        (self.ledgers.iter().rev())
            .find(|(_, sealed)| {
                sealed.header.close_time < close_time
                    && sealed.state.is_some()
                    && !sealed.signers.is_empty()
            })
            .map(|(index, _)| *index)
    }

    /// Forgets the ledgers before `index`, but the last.
    pub fn forget_before(&mut self, index: u32) {
        let last = self.ledgers.last_key_value().map(|(last, _)| *last);
        self.ledgers
            .retain(|kept, _| *kept >= index || Some(*kept) == last);
        self.by_hash
            .retain(|_, kept| *kept >= index || Some(*kept) == last);
        self.check_invariants();
    }

    /// Keeps `sealed` as the ledger of its index, and forgets the oldest
    /// beyond [`MAX_KEPT_LEDGERS`].
    fn insert(&mut self, sealed: Sealed) {
        let index = sealed.header.sequence;
        self.remove(index);
        self.by_hash.insert(sealed.hash, index);
        self.ledgers.insert(index, sealed);
        while self.ledgers.len() > MAX_KEPT_LEDGERS {
            if let Some((_, oldest)) = self.ledgers.pop_first() {
                self.by_hash.remove(&oldest.hash);
            }
        }
    }

    /// Forgets the ledger of `index`, if it is kept.
    fn remove(&mut self, index: u32) {
        if let Some(sealed) = self.ledgers.remove(&index) {
            self.by_hash.remove(&sealed.hash);
        }
    }

    /// The skip lists as objects.
    fn skip_lists(&self) -> Vec<([u8; 32], Object)> {
        let mut out = Vec::new();
        let (last, hashes) = &self.recent;
        if !hashes.is_empty() {
            out.push((objects::skip_key(), objects::ledger_hashes(*last, hashes)));
        }
        for (last, hashes) in self.flags.values() {
            out.push((
                objects::flag_skip_key(*last),
                objects::ledger_hashes(*last, hashes),
            ));
        }
        out
    }

    /// Notes `parent`, of the ledger before `index`, in the skip lists, as
    /// rippled does as it builds ledger `index`.
    fn add_parent(&mut self, index: u32, parent: [u8; 32]) {
        let previous = index.saturating_sub(1);
        if previous.is_multiple_of(FLAG_INTERVAL) {
            let (last, hashes) = self
                .flags
                .entry(previous >> 16)
                .or_insert_with(|| (previous, Vec::new()));
            hashes.push(parent);
            *last = previous;
        }
        let (last, hashes) = &mut self.recent;
        if hashes.len() == SKIP_HASHES {
            hashes.remove(0);
        }
        hashes.push(parent);
        *last = previous;
    }

    /// What holds after every change. What each ledger keeps is trimmed as
    /// it crosses a bound ([`FakeLedger::seal`]), so this walks no chain.
    fn check_invariants(&self) {
        assert!(
            self.ledgers.len() <= MAX_KEPT_LEDGERS,
            "a bounded chain of headers"
        );
        assert_eq!(
            self.by_hash.len(),
            self.ledgers.len(),
            "each ledger kept found by its hash"
        );
        assert!(self.recent.1.len() <= SKIP_HASHES, "a skip list's bound");
        assert!(
            (self.flags.values()).all(|(_, hashes)| hashes.len() <= SKIP_HASHES),
            "a flag skip list's bound: 65,536 ledgers hold 256 flag ledgers"
        );
    }
}

impl FakeLedger {
    /// Starts the chain again at the current ledger, as the first of a new
    /// one: no parent, and no skip list.
    pub(super) fn seal_first(&mut self) {
        self.chain = Chain::new();
        self.seal(([0; 32], 0));
    }

    /// The current ledger closed after the last one sealed: its parent goes
    /// into the skip lists, as rippled updates them after a ledger's
    /// transactions.
    pub(super) fn seal_next(&mut self) {
        let parent = self
            .chain
            .latest()
            .map_or(([0; 32], 0), |last| (last.hash, last.header.close_time));
        self.chain.add_parent(self.ledger, parent.0);
        self.seal(parent);
    }

    /// The current ledger closed again at a new close time, as the first
    /// of a chain started then.
    pub(super) fn reseal(&mut self) {
        let Some(latest) = self.chain.latest() else {
            return;
        };
        let parent = (latest.header.parent_hash, latest.header.parent_close_time);
        self.chain.remove(self.ledger);
        self.seal(parent);
    }

    /// Seals the current ledger on `parent`, its hash and close time: its
    /// state tree from the model, its transactions, its header, and who
    /// signs it.
    fn seal(&mut self, (parent_hash, parent_close_time): ([u8; 32], u32)) {
        let objects = self.state_objects();
        let keys: alloc::collections::BTreeSet<[u8; 32]> =
            objects.iter().map(|(key, _)| *key).collect();
        for (key, object) in &objects {
            self.chain.state.insert_object(*key, &object.to_bytes());
        }
        self.chain.state.retain(|key| keys.contains(key));
        self.threads.retain(|key, _| keys.contains(key));
        let header = LedgerHeader {
            sequence: self.ledger,
            total_drops: MAX_DROPS.saturating_sub(self.burned.saturating_add(self.wiped)),
            parent_hash,
            transaction_hash: self.transactions(self.ledger).root(),
            account_hash: self.chain.state.root(),
            parent_close_time,
            close_time: u32::try_from(self.close_time).unwrap_or(u32::MAX),
            close_time_resolution: CLOSE_TIME_RESOLUTION_S,
            close_flags: 0,
        };
        let sealed = Sealed {
            header,
            hash: header.hash(),
            state: Some(self.chain.state.clone()),
            signers: self.validators.signers(),
            validations: None,
        };
        let chain = &mut self.chain;
        chain.insert(sealed);
        // What each close leaves behind; older ledgers were trimmed as the
        // ones after them closed, so only flag ledgers keep a state or
        // signers past their bounds.
        let ledgers = chain.ledgers.values_mut().rev();
        for (age, sealed) in ledgers.enumerate() {
            // Past the newest ledger beyond the bound, one trimmed already,
            // and the flag ledgers on the way.
            assert!(
                age <= MAX_VALIDATED_LEDGERS + 3,
                "only the ledgers that crossed a bound are trimmed"
            );
            if sealed.header.sequence.is_multiple_of(FLAG_INTERVAL) {
                continue;
            }
            if age >= MAX_STATE_LEDGERS {
                sealed.state = None;
            }
            if age >= MAX_VALIDATED_LEDGERS {
                if sealed.signers.is_empty() && sealed.validations.is_none() {
                    break;
                }
                sealed.signers = Vec::new();
                sealed.validations = None;
            }
        }
        chain.check_invariants();
    }

    /// The transaction tree of the ledger of `index`, from the history.
    pub(super) fn transactions(&self, index: u32) -> ShaMap {
        let mut tree = ShaMap::new(Tree::Transactions);
        let start = self
            .history
            .partition_point(|applied| applied.ledger < index);
        let end = self
            .history
            .partition_point(|applied| applied.ledger <= index);
        for applied in self.history.get(start..end).unwrap_or_default() {
            let key = tree.insert_transaction(&applied.blob, &applied.meta);
            assert_eq!(key, applied.hash, "a transaction under its hash");
        }
        tree
    }

    /// Every object the ledger holds, by its key.
    fn state_objects(&self) -> Vec<([u8; 32], Object)> {
        let mut out = Vec::new();
        for (id, account) in &self.accounts {
            let key = objects::account_key(*id);
            let root = objects::account_root(*id, account, self.amm_of(*id), self.thread(&key));
            out.push((key, root));
        }
        for (line, state) in &self.lines {
            let key = line_index(line);
            out.push((key, objects::ripple_state(state, self.thread(&key))));
        }
        for (owner, list) in &self.signer_lists {
            let key = signer_list_index(*owner);
            out.push((key, objects::signer_list(list, self.thread(&key))));
        }
        for (token, pool) in &self.pools {
            let key = objects::amm_key(*token);
            out.push((key, objects::amm(*token, pool, self.thread(&key))));
        }
        out.extend(self.chain.skip_lists());
        out.push((
            objects::fees_key(),
            objects::fee_settings(FEE_BASE, self.reserve),
        ));
        out
    }

    /// The last ledger's header, if the chain has one.
    #[must_use]
    pub fn header(&self, index: u32) -> Option<LedgerHeader> {
        self.chain.get(index).map(|sealed| sealed.header)
    }

    /// The hash of the ledger of `index`, if it is kept.
    #[must_use]
    pub fn ledger_hash(&self, index: u32) -> Option<[u8; 32]> {
        self.chain.get(index).map(|sealed| sealed.hash)
    }

    /// The validations of the ledger of `index` the fake holds, signed as
    /// validations are signed now.
    pub(super) fn validations_of(&mut self, index: u32) -> Vec<Vec<u8>> {
        let Some(sealed) = self.chain.get(index) else {
            return Vec::new();
        };
        if let Some(validations) = &sealed.validations {
            return validations.clone();
        }
        let ledger = (sealed.hash, index, sealed.header.close_time);
        let validations = self.validators.validations(ledger, &sealed.signers);
        if let Some(sealed) = self.chain.get_mut(index) {
            sealed.validations = Some(validations.clone());
        }
        validations
    }
}
