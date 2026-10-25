//! What the account reads, proven
//! (spec/designs/xrpl-validation.md#4-what-is-proven-and-how): the state
//! it follows, proven at each anchor, and the transactions it tells, each
//! proven in a ledger of the anchor's chain and found by the history the
//! proven state names.
//!
//! - **State.** Each anchoring round proves, at the anchor, the account's
//!   root, each line its schemes follow, those lines' issuers' roots and
//!   the fee settings ([`View`]). That is what the account holds, its
//!   sequence and the reserve, as of the anchor. A root proven absent is
//!   the account gone. An issuer's account or a pool a scheme asks for is
//!   read at once, in one state proof at the anchor: a pool is its `AMM`
//!   object, which names the pool's account, and that account's root and
//!   line, all at one ledger.
//! - **Transactions.** A transaction is told only once `tx_proof` proves
//!   it: its bytes and metadata, which `tx` gives in binary once the proof
//!   came, hash to it, and the leaf the account builds from them, with the
//!   proof's branch above it, leads to the transaction root of a header
//!   whose ledger is the anchor, or one the anchor's skip lists hold
//!   ([`ledger_proof`]). What the schemes read is decoded from those bytes
//!   ([`crate::records`]).
//! - **The history.** Each object the account follows names the
//!   transaction that last changed it, and each proven transaction's
//!   metadata the one before. From each anchor, the account walks these
//!   back to what it told, or to the earliest cursor of its schemes, and
//!   tells what it found in ledger order, no new anchor taken meanwhile. A
//!   line deleted since, which the anchor proves absent or made again, is
//!   taken up from the transaction that deleted it, which the root's chain
//!   finds, as it changed the root's owner count too: its metadata names
//!   what changed the line before. So
//!   a transaction a server leaves out of its history, or of the stream, is
//!   found by its hash; they only name hashes sooner, as hints to prove
//!   them. The walk finds hashes and nothing else: the state is the
//!   anchor's, and the newest transaction that changed an object must leave
//!   it as the anchor proves it. A walk finds at
//!   most [`MAX_WALKED`] transactions; what lies beyond, and history the
//!   server no longer has beyond [`HISTORY_BOUND`], is a gap the schemes
//!   match through suspense, to the balance the anchor proves, which they
//!   are told as when they sync once the walk ends, synced or not. Whether
//!   a ledger is beyond the bound is judged only on an index the account
//!   proved.
//! - **Absences.** One of our transactions is cancelled only once proven
//!   absent: the account's sequence, proven at an anchor past its last
//!   ledger, is not past its own; or the history told another of the
//!   account's transactions that used its number.
//!
//! What a server says otherwise is a hint, which may steer what is asked
//! for, and never reaches the books. An answer that does not prove is a
//! lie, counted by kind, and the account connects anew a while later.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde_json::Value;

use super::anchors::{HISTORY, NO_PROOFS, REFUSED};
use super::{
    Doubt, LineKey, MAX_ACCOUNT_DROPS, MAX_IN_FLIGHT, MAX_LIST_ASKS, MAX_LISTS, MAX_SCHEMES, News,
    Phase, Request, TOO_MUCH_XRP, XrplAccount,
};
use crate::amount::TokenValue;
use crate::anchor::{
    LedgerProof, ProvenState, ProvenTx, ledger_proof, prove_state, prove_transaction,
};
use crate::api::{
    self, AccountInfo, AccountSettings, ApiError, MAX_PROOF_KEYS, TrustLine, TxRecord,
};
use crate::keys::AccountId;
use crate::peg::PoolReading;
use crate::records::{self, Fees, Root, Thread};
use crate::skip_list::{SkipList, recent_key};
use crate::unl::Anchor;

/// The history every proof server keeps, in ledgers: about two days at
/// four seconds a ledger. A transaction in a ledger further back than
/// this from the anchor, as a proven index says, that the server no longer
/// has, is history it does not have; one within it that it says it no
/// longer has stalls the account, whose server keeps too short a history.
/// So it stays below what the proof servers keep even right after rippled
/// deleted its oldest ledgers, when it holds only `online_delete` of them
/// (`resources/proof-server/xrpld-*.cfg`), with a margin for the ledgers
/// that close while the account walks; `tests/proof_servers.rs` reads
/// both configs to hold the two apart.
pub const HISTORY_BOUND: u32 = 40_000;
/// Most transaction proofs asked for at once.
pub const MAX_PROVING: usize = 16;
/// Most proven transactions kept until they are told: what the walk found,
/// what the hints and the schemes' lookups named, and ours.
pub const MAX_PROOFS: usize = 256;
/// Most transactions one walk of the history finds, by hash and place:
/// beyond them, what the walk did not reach is a gap.
pub const MAX_WALKED: usize = 4096;
/// How long after a server answered that it had not saved a transaction
/// with its ledger yet its bytes are asked again: rippled saves a ledger
/// within moments of validating it.
pub const UNSAVED_RETRY_MS: u64 = 1_000;
/// How often the bytes of one transaction are asked again so: past it, the
/// server is taken as refusing them.
pub const MAX_UNSAVED_TRIES: u32 = 8;
/// Most places one walk keeps where an object's chain is taken up again:
/// each a transaction found that changed the object, which its chain has
/// not reached. Beyond them, what the walk did not reach is a gap.
const MAX_RESUMES: usize = MAX_WALKED;
/// Most turns of one walk's progress at once: each finds a transaction or
/// ends a chain, or tells one; a chain taken up again ends once more.
const MAX_WALK_TURNS: usize = MAX_WALKED + MAX_RESUMES + MAX_SCHEMES + 2;

/// A pool a scheme reads, and the account its `AMM` object last named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PoolWatch {
    /// The pool's account, once its object named it.
    pub account: Option<AccountId>,
    /// Whether a scheme asked for the pool since it was last told.
    pub asked: bool,
}

/// What an anchor proved of a pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PoolSeen {
    /// There is none.
    Gone,
    /// Its object names this account, whose root and line the next anchor
    /// proves.
    Named(AccountId),
    /// What it holds.
    Read(PoolReading),
}

/// What the account follows, as proven at an anchor.
#[derive(Clone, Debug)]
pub(super) struct View {
    anchor: Anchor,
    /// The anchor's list of the ledgers before it.
    recent: Option<SkipList>,
    /// The account's root; none if it does not exist.
    root: Option<Root>,
    /// Each line its schemes follow that the round proved.
    lines: BTreeMap<LineKey, Option<TrustLine>>,
    /// The root's key and each line's, with the transaction that last
    /// changed each.
    threads: BTreeMap<[u8; 32], Option<Thread>>,
    /// The accounts of the lines' issuers, and of those the schemes asked
    /// for.
    issuers: BTreeMap<AccountId, Option<AccountSettings>>,
    fees: Option<Fees>,
    pools: BTreeMap<LineKey, PoolSeen>,
}

/// A proven transaction, kept until it is told.
#[derive(Clone, Debug)]
pub(super) struct Proof {
    proven: ProvenTx,
    /// Whether its ledger is proven on the anchor's chain.
    pub(super) holds: bool,
}

/// A transaction whose proof came, but whose bytes the server had not
/// saved with its ledger yet.
#[derive(Clone, Copy, Debug)]
pub(super) struct Unsaved {
    /// The ledger its proof was asked in.
    pub ledger: Option<u32>,
    /// When its bytes are asked again; none while they are asked.
    pub due_ms: Option<u64>,
    /// How often they were asked again.
    pub tries: u32,
}

/// A walk of the history back from an anchor's state.
#[derive(Clone, Debug)]
pub(super) struct Walk {
    view: View,
    /// What was told already: at or before it.
    told: (u32, u32),
    /// Each object's chain, by its key: the next transaction to prove, by
    /// its hash and its proven ledger; none once it reached what was told.
    chains: BTreeMap<[u8; 32], Option<Thread>>,
    /// The transactions found, by their place.
    found: BTreeMap<(u32, u32), [u8; 32]>,
    /// Where each object's chain is taken up again once it ends, by its
    /// key and the place of a transaction found that changed it, which its
    /// chain has not reached: the transaction that changed it before. A
    /// line deleted, found by the root's chain, starts so an earlier
    /// stretch of the line's chain; one its chain reaches goes.
    resumes: BTreeMap<([u8; 32], (u32, u32)), Thread>,
    /// Whether it left a gap: what it did not reach.
    gap: bool,
}

impl Walk {
    /// Asserts what holds of it.
    pub(super) fn check_invariants(&self) {
        assert!(
            self.found.len() <= MAX_WALKED,
            "a walk finds within its bound"
        );
        assert!(
            self.chains.len() <= 1 + MAX_SCHEMES,
            "the root's and the lines'"
        );
        assert!(
            self.found.keys().all(|at| *at > self.told),
            "only what was not told is found"
        );
        assert!(
            self.chains
                .values()
                .flatten()
                .all(|(_, ledger)| *ledger <= self.view.anchor.index),
            "a chain names ledgers up to its anchor"
        );
        assert!(
            self.resumes.len() <= MAX_RESUMES,
            "a walk takes up chains within its bound"
        );
        assert!(
            (self.resumes.iter()).all(|((key, at), (_, ledger))| {
                self.chains.contains_key(key) && *at > self.told && *ledger <= at.0
            }),
            "a chain is taken up before what was found, of what was not told"
        );
    }

    /// Takes up the chain of the object of `key`, once it ended, from the
    /// newest transaction found that changed the object and that the chain
    /// did not reach.
    fn take_up(&mut self, key: [u8; 32]) {
        if !matches!(self.chains.get(&key), Some(None)) {
            return;
        }
        let newest = (self.resumes)
            .range((key, (0, 0))..=(key, (u32::MAX, u32::MAX)))
            .next_back()
            .map(|(at, before)| (*at, *before));
        if let Some((at, before)) = newest {
            self.resumes.remove(&at);
            self.chains.insert(key, Some(before));
        }
    }
}

/// What checking a transaction's proof came to.
enum Checked {
    /// It holds.
    Holds,
    /// Not proven yet.
    Missing,
    /// Its ledger is not proven yet.
    Waits,
    /// It lied, and the account connects anew.
    Lie,
}

/// What a step of the walk came to.
enum Step {
    Moved,
    Waits,
    Stopped,
}

/// Adds `key` to `keys`, once.
fn push_new(keys: &mut Vec<[u8; 32]>, key: [u8; 32]) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}

/// The settings an issuer's root holds, as of `anchor`.
const fn settings(root: &Root, anchor: &Anchor) -> AccountSettings {
    AccountSettings {
        flags: root.flags,
        transfer_rate: root.transfer_rate,
        regular_key: root.regular_key,
        ledger_index: anchor.index,
        validated: true,
    }
}

/// What the anchor proves.
impl XrplAccount {
    /// The keys an anchoring round proves: the list of recent ledgers, the
    /// account's root, the fee settings, each line the schemes follow and
    /// the lines' issuers. What else the schemes ask for, an issuer's
    /// account or a pool, is read at the anchor when they ask
    /// ([`read_asked`](Self::read_asked)).
    pub(super) fn followed_keys(&self) -> Vec<[u8; 32]> {
        let us = self.account();
        let mut keys = alloc::vec![recent_key(), records::account_key(us), records::fees_key()];
        for key in self.lines.keys() {
            push_new(&mut keys, records::line_key(us, key.issuer, key.currency));
        }
        for key in self.lines.keys() {
            push_new(&mut keys, records::account_key(key.issuer));
        }
        assert!(keys.len() <= MAX_PROOF_KEYS, "one state proof's worth");
        keys
    }

    /// The keys whose history the account walks: its root's and its
    /// lines'.
    fn thread_keys(&self) -> Vec<[u8; 32]> {
        let us = self.account();
        let lines = (self.lines.keys()).map(|key| records::line_key(us, key.issuer, key.currency));
        core::iter::once(records::account_key(us))
            .chain(lines)
            .collect()
    }

    /// What `proven`, the state proof of a validated ledger, says of what
    /// the account follows. An object it cannot read, or that is not what
    /// its key holds, is a lie.
    pub(super) fn read_view(
        &self,
        anchor: Anchor,
        proven: &ProvenState,
        recent: Option<SkipList>,
    ) -> Result<View, Doubt> {
        let us = self.account();
        let unreadable = |_| Doubt::Unreadable;
        let root_key = records::account_key(us);
        let root = match proven.object(&root_key) {
            Some(Some(bytes)) => Some(records::account_root(bytes).map_err(unreadable)?),
            Some(None) => None,
            None => return Err(Doubt::Keys),
        };
        if root.is_some_and(|root| root.account != us) {
            return Err(Doubt::Unreadable);
        }
        let mut threads = BTreeMap::new();
        threads.insert(root_key, root.and_then(|root| root.thread));
        let mut lines = BTreeMap::new();
        for key in self.lines.keys() {
            let line_key = records::line_key(us, key.issuer, key.currency);
            let (line, thread) = match proven.object(&line_key) {
                None => continue,
                Some(None) => (None, None),
                Some(Some(bytes)) => {
                    let (line, thread) = records::ripple_state(bytes).map_err(unreadable)?;
                    if !line.is_between(us, key.issuer, key.currency) {
                        return Err(Doubt::Unreadable);
                    }
                    (Some(line), thread)
                }
            };
            lines.insert(*key, line);
            threads.insert(line_key, thread);
        }
        let fees = match proven.object(&records::fees_key()) {
            Some(Some(bytes)) => Some(records::fee_settings(bytes).map_err(unreadable)?),
            _ => None,
        };
        Ok(View {
            anchor,
            recent,
            root,
            lines,
            threads,
            issuers: self.read_issuers(anchor, proven)?,
            fees,
            pools: self.read_pools(anchor, proven)?,
        })
    }

    /// The issuers' roots `proven` holds, of the lines' issuers and those
    /// the schemes asked for.
    fn read_issuers(
        &self,
        anchor: Anchor,
        proven: &ProvenState,
    ) -> Result<BTreeMap<AccountId, Option<AccountSettings>>, Doubt> {
        let mut issuers = BTreeMap::new();
        let asked =
            (self.lines.keys().map(|key| key.issuer)).chain(self.issuers_asked.iter().copied());
        for issuer in asked {
            let settings = match proven.object(&records::account_key(issuer)) {
                None => continue,
                Some(None) => None,
                Some(Some(bytes)) => {
                    let root = records::account_root(bytes).map_err(|_| Doubt::Unreadable)?;
                    if root.account != issuer {
                        return Err(Doubt::Unreadable);
                    }
                    Some(settings(&root, &anchor))
                }
            };
            issuers.insert(issuer, settings);
        }
        Ok(issuers)
    }

    /// What `proven` holds of each pool the schemes read: its `AMM`
    /// object, and, where that names the account the account knew, the
    /// account's root and its line to the issuer, all of one ledger.
    fn read_pools(
        &self,
        anchor: Anchor,
        proven: &ProvenState,
    ) -> Result<BTreeMap<LineKey, PoolSeen>, Doubt> {
        let unreadable = |_| Doubt::Unreadable;
        let mut pools = BTreeMap::new();
        for (key, watch) in &self.pools {
            let seen = match proven.object(&records::amm_key(key.issuer, key.currency)) {
                None => continue,
                Some(None) => PoolSeen::Gone,
                Some(Some(bytes)) => {
                    let object = records::amm(bytes).map_err(unreadable)?;
                    if !object.pairs(key.issuer, key.currency) {
                        return Err(Doubt::Unreadable);
                    }
                    let account = object.account;
                    let root = proven.object(&records::account_key(account));
                    let line = proven.object(&records::line_key(account, key.issuer, key.currency));
                    match (watch.account == Some(account), root, line) {
                        (true, Some(Some(root)), Some(line)) => {
                            let root = records::account_root(root).map_err(unreadable)?;
                            let value = match line {
                                None => TokenValue::ZERO,
                                Some(line) => {
                                    let (line, _) =
                                        records::ripple_state(line).map_err(unreadable)?;
                                    let view = line.view(account).ok_or(Doubt::Unreadable)?;
                                    view.balance
                                }
                            };
                            if root.account != account || value.is_negative() {
                                return Err(Doubt::Unreadable);
                            }
                            PoolSeen::Read(PoolReading {
                                account,
                                drops: root.balance,
                                value,
                                ledger_index: anchor.index,
                            })
                        }
                        (true, Some(None), _) => PoolSeen::Gone,
                        _ => PoolSeen::Named(account),
                    }
                }
            };
            pools.insert(*key, seen);
        }
        Ok(pools)
    }
}

/// The anchor's state, and the history up to it.
impl XrplAccount {
    /// `view` is what its anchor, now current, proves: while the account
    /// starts, it is read from it; while synced, the history up to it is
    /// walked, and then taken.
    pub(super) fn anchored_view(&mut self, view: View) {
        self.lists.clear();
        self.lists
            .insert((view.anchor.hash, recent_key()), view.recent.clone());
        match self.phase {
            Phase::Starting { .. } => self.begin(view),
            Phase::Unfunded if view.root.is_some() => self.begin(view),
            Phase::Unfunded => self.fund(),
            Phase::Synced if view.root.is_none() => self.absent(),
            Phase::Synced => {
                let told = self.at.unwrap_or((view.anchor.index, u32::MAX));
                self.start_walk(view, Some(told));
            }
            Phase::CatchingUp { .. } | Phase::Disconnected => {}
        }
        self.advance();
    }

    /// The account as `view` proves it, as it starts: gone, or read, and its
    /// history walked from the earliest cursor of its schemes. A line its
    /// schemes followed since the round began is proven by the next anchor.
    fn begin(&mut self, view: View) {
        let Some(root) = view.root else {
            return self.absent();
        };
        if self.lines.keys().any(|key| !view.lines.contains_key(key)) {
            return;
        }
        if root.balance > MAX_ACCOUNT_DROPS {
            self.failed = Some(TOO_MUCH_XRP);
            return;
        }
        self.proven_sequence = root.sequence;
        self.reset_sequence();
        let info = AccountInfo {
            balance: root.balance,
            sequence: root.sequence,
            owner_count: root.owner_count,
            ledger_index: view.anchor.index,
        };
        let from = (self.followers.values())
            .filter_map(|follower| follower.cursor)
            .min();
        let anchor = view.anchor.index;
        self.phase = Phase::CatchingUp { info };
        self.start_walk(view, from);
        if let Some(cursor) = from {
            self.at = self.at.max(Some(cursor));
            // The history the server keeps names what the walk will find,
            // sooner: a hint.
            self.send(Request::AccountTx(cursor.0, anchor));
        }
    }

    /// Walks the history from `view`'s objects back to `told`; with none,
    /// there is nothing to tell, and the view is taken at once.
    fn start_walk(&mut self, view: View, told: Option<(u32, u32)>) {
        let chains = if told.is_some() {
            view.threads.clone()
        } else {
            BTreeMap::new()
        };
        self.walk = Some(Walk {
            view,
            told: told.unwrap_or((0, 0)),
            chains,
            found: BTreeMap::new(),
            resumes: BTreeMap::new(),
            gap: false,
        });
    }

    /// Takes what came in as far as it goes: the walk, our transactions
    /// looked for, the schemes' lookups; then asks for the proofs they
    /// need.
    pub(super) fn advance(&mut self) {
        self.advance_walk();
        if self.walk.is_none() && self.phase == Phase::Synced {
            self.advance_ours();
        }
        self.advance_lookups();
        self.dispatch_proofs();
    }

    fn advance_walk(&mut self) {
        for _ in 0..MAX_WALK_TURNS {
            if self.failed.is_some() || self.stalled_until_ms > 0 {
                return;
            }
            let Some(walk) = &self.walk else {
                return;
            };
            let open: Vec<[u8; 32]> = (walk.chains.iter())
                .filter_map(|(key, next)| next.map(|_| *key))
                .collect();
            if open.is_empty() {
                match self.tell_found() {
                    Step::Moved => return self.finish_walk(),
                    Step::Waits | Step::Stopped => return,
                }
            }
            let mut moved = false;
            for key in open {
                match self.step_chain(key) {
                    Step::Moved => moved = true,
                    Step::Waits => {}
                    Step::Stopped => return,
                }
            }
            if !moved {
                return;
            }
        }
    }

    /// One step back along the chain of the object of `key`: past what was
    /// told it ends; otherwise its next transaction, once proven, is found,
    /// and names the one before. A transaction found anew that changed
    /// another object, whose chain has not reached it, is where that chain
    /// is taken up again once it ends: so a line the anchor proves absent
    /// is walked from the transaction that deleted it.
    fn step_chain(&mut self, key: [u8; 32]) -> Step {
        let Some(walk) = &self.walk else {
            return Step::Stopped;
        };
        let (anchor, told) = (walk.view.anchor, walk.told);
        let Some(Some((hash, ledger))) = walk.chains.get(&key).copied() else {
            return Step::Waits;
        };
        if (ledger, u32::MAX) <= told {
            return self.chain_to(key, None);
        }
        match self.check(&hash, Some(ledger), &anchor) {
            Checked::Holds => {}
            Checked::Missing | Checked::Waits => return Step::Waits,
            Checked::Lie => return Step::Stopped,
        }
        let Some(proof) = self.proofs.get(&hash) else {
            return Step::Waits;
        };
        let record = &proof.proven.decoded.record;
        let at = (record.ledger_index, record.index);
        let before = (proof.proven.decoded.threads.iter()).find(|(of, _)| *of == key);
        let (Some((_, before)), true) = (before.copied(), record.ledger_index == ledger) else {
            // The object names a transaction that did not change it, or one
            // of another ledger. The proof goes, so that the next
            // connection proves it anew rather than doubting it again.
            self.proofs.remove(&hash);
            self.doubted(Doubt::History);
            return Step::Stopped;
        };
        if at <= told {
            return self.chain_to(key, None);
        }
        let others: Vec<([u8; 32], Thread)> = (proof.proven.decoded.threads.iter())
            .filter(|(of, _)| *of != key)
            .filter_map(|(of, earlier)| earlier.map(|earlier| (*of, earlier)))
            .filter(|(_, earlier)| earlier.1 <= at.0)
            .collect();
        let Some(walk) = &mut self.walk else {
            return Step::Stopped;
        };
        let new = !walk.found.contains_key(&at);
        if walk.found.len() >= MAX_WALKED && new {
            self.left_a_gap();
            return self.chain_to(key, None);
        }
        walk.found.insert(at, hash);
        walk.resumes.remove(&(key, at));
        if new {
            for (of, earlier) in others {
                if !walk.chains.contains_key(&of) {
                    continue;
                }
                if walk.resumes.len() >= MAX_RESUMES {
                    self.left_a_gap();
                    break;
                }
                walk.resumes.insert((of, at), earlier);
                walk.take_up(of);
            }
        }
        self.chain_to(key, before)
    }

    /// The chain of the object of `key` goes on at `next`; once it ends, it
    /// is taken up again where a transaction found names it.
    fn chain_to(&mut self, key: [u8; 32], next: Option<Thread>) -> Step {
        if let Some(walk) = &mut self.walk
            && let Some(chain) = walk.chains.get_mut(&key)
        {
            *chain = next;
            walk.take_up(key);
        }
        Step::Moved
    }

    /// The walk left a gap: what it did not reach the schemes match
    /// through suspense, counted once a walk.
    fn left_a_gap(&mut self) {
        if let Some(walk) = &mut self.walk
            && !walk.gap
        {
            walk.gap = true;
            self.counted.gaps = self.counted.gaps.saturating_add(1);
        }
    }

    /// Tells what the walk found, oldest first, as each is proven: `Moved`
    /// once all of it is told.
    fn tell_found(&mut self) -> Step {
        for _ in 0..MAX_WALKED {
            let Some(walk) = &self.walk else {
                return Step::Stopped;
            };
            let anchor = walk.view.anchor;
            let Some((&at, &hash)) = walk.found.first_key_value() else {
                return Step::Moved;
            };
            match self.check(&hash, Some(at.0), &anchor) {
                Checked::Holds => {}
                Checked::Missing | Checked::Waits => return Step::Waits,
                Checked::Lie => return Step::Stopped,
            }
            let Some(proof) = self.proofs.remove(&hash) else {
                return Step::Stopped;
            };
            if !self.agrees(&proof.proven.decoded.record) {
                self.doubted(Doubt::History);
                return Step::Stopped;
            }
            if let Some(walk) = &mut self.walk {
                walk.found.remove(&at);
            }
            self.transaction(&proof.proven.decoded);
            if self.failed.is_some() {
                return Step::Stopped;
            }
        }
        Step::Waits
    }

    /// Whether `record` leaves the root and the lines as the anchor proves
    /// them, where it is the last transaction that changed them: else the
    /// history and the state disagree, and one of them is a lie.
    fn agrees(&self, record: &TxRecord) -> bool {
        let Some(walk) = &self.walk else {
            return true;
        };
        let view = &walk.view;
        let us = self.account();
        let last = |key: &[u8; 32]| {
            (view.threads.get(key))
                .is_some_and(|thread| thread.is_some_and(|(hash, _)| hash == record.hash))
        };
        let root = match (record.change_of(us), view.root) {
            (Some(change), Some(root)) if last(&records::account_key(us)) => {
                change.after == root.balance
            }
            _ => true,
        };
        let lines = view.lines.iter().all(|(key, line)| {
            let change = record.line(us, key.issuer, key.currency);
            match change {
                Some(change) if last(&records::line_key(us, key.issuer, key.currency)) => {
                    change.after() == line.as_ref()
                }
                _ => true,
            }
        });
        root && lines
    }

    /// The history is walked: the anchor's state is taken.
    fn finish_walk(&mut self) {
        let Some(walk) = self.walk.take() else {
            return;
        };
        let gap = walk.gap;
        let view = walk.view;
        let moved = self.at.is_none_or(|at| view.anchor.index > at.0);
        self.apply_view(&view);
        match self.phase {
            // The schemes are told the issuers' accounts the anchor proved
            // with the lines, so that a line can be set as they sync.
            Phase::CatchingUp { info } => {
                self.tell_issuers(&view.issuers, true);
                self.synced(info, &view.lines);
                self.tell_pools(&view.pools);
                self.read_asked();
            }
            Phase::Synced => {
                self.tell_issuers(&view.issuers, false);
                self.tell_pools(&view.pools);
                // A gap the walk left while synced, as a burst past what
                // one walk finds leaves: the schemes match their books to
                // the balance the anchor proves, as when they sync, rather
                // than wait for the next connection.
                if let (true, Some(root)) = (gap, view.root) {
                    let info = AccountInfo {
                        balance: root.balance,
                        sequence: root.sequence,
                        owner_count: root.owner_count,
                        ledger_index: view.anchor.index,
                    };
                    self.synced(info, &view.lines);
                }
                if moved || self.reserve_changed {
                    let reserve_changed = core::mem::take(&mut self.reserve_changed);
                    self.tell(News::LedgerClosed { reserve_changed });
                }
                self.expire();
            }
            _ => {}
        }
    }

    /// What the account holds is as `view` proves it.
    fn apply_view(&mut self, view: &View) {
        if let Some(root) = view.root {
            self.balance = Some(root.balance);
            self.owner_count = root.owner_count;
            self.proven_sequence = root.sequence;
            self.reset_sequence();
        }
        for (key, line) in &view.lines {
            if let Some(state) = self.lines.get_mut(key) {
                *state = Some(*line);
            }
        }
        if let Some(fees) = view.fees {
            let reserve = (fees.reserve_base, fees.reserve_inc);
            let known = self.reserve_base > 0 || self.reserve_inc > 0;
            if known && reserve != (self.reserve_base, self.reserve_inc) {
                self.reserve_changed = true;
            }
            (self.reserve_base, self.reserve_inc) = reserve;
        }
        let at = (view.anchor.index, u32::MAX);
        self.at = Some(self.at.map_or(at, |told| told.max(at)));
    }

    /// The issuers' accounts the schemes asked for, as an anchor proves
    /// them in `issuers`, or, with `all`, every one it proves.
    fn tell_issuers(&mut self, issuers: &BTreeMap<AccountId, Option<AccountSettings>>, all: bool) {
        for (issuer, settings) in issuers {
            if self.lines.keys().any(|key| key.issuer == *issuer) {
                match settings {
                    Some(settings) => self.issuers.insert(*issuer, *settings),
                    None => self.issuers.remove(issuer),
                };
            }
            if all || self.issuers_asked.contains(issuer) {
                self.issuers_asked.remove(issuer);
                self.tell(News::Issuer {
                    issuer: *issuer,
                    settings: *settings,
                });
            }
        }
    }

    /// The pools the schemes asked for, as an anchor proves them in
    /// `pools`; a pool whose account it proved only by name is read again.
    fn tell_pools(&mut self, pools: &BTreeMap<LineKey, PoolSeen>) {
        for (key, seen) in pools {
            let Some(watch) = self.pools.get_mut(key) else {
                continue;
            };
            let (account, pool) = match seen {
                PoolSeen::Gone => (None, Some(None)),
                PoolSeen::Named(account) => (Some(*account), None),
                PoolSeen::Read(reading) => (Some(reading.account), Some(Some(*reading))),
            };
            watch.account = account;
            if let (true, Some(pool)) = (watch.asked, pool) {
                watch.asked = false;
                self.tell(News::Pool { key: *key, pool });
            }
        }
    }

    /// The keys a read at the anchor proves of what the schemes asked for:
    /// each issuer's root, and each pool's `AMM` object, with its account's
    /// root and line once known.
    fn asked_keys(&self) -> Vec<[u8; 32]> {
        let mut keys = Vec::new();
        for issuer in &self.issuers_asked {
            push_new(&mut keys, records::account_key(*issuer));
        }
        for (key, watch) in self.pools.iter().filter(|(_, watch)| watch.asked) {
            push_new(&mut keys, records::amm_key(key.issuer, key.currency));
            if let Some(account) = watch.account {
                push_new(&mut keys, records::account_key(account));
                push_new(
                    &mut keys,
                    records::line_key(account, key.issuer, key.currency),
                );
            }
        }
        assert!(keys.len() <= MAX_PROOF_KEYS, "one state proof's worth");
        keys
    }

    /// Reads what the schemes asked for at the anchor, in one state proof,
    /// unless a read is on its way.
    pub(super) fn read_asked(&mut self) {
        let Some(kept) = self.anchor else {
            return;
        };
        if self.phase != Phase::Synced || self.reading.is_some() || self.stalled_until_ms > 0 {
            return;
        }
        let keys = self.asked_keys();
        if keys.is_empty() {
            return;
        }
        self.reading = Some(keys);
        self.send(Request::Read(kept.anchor.hash));
    }

    /// What the schemes asked for, proven at the ledger of `at`: told, if
    /// that is still the anchor; else read again at the one now.
    pub(super) fn read_answered(&mut self, at: [u8; 32], outcome: Result<Value, ApiError>) {
        let Some(keys) = self.reading.take() else {
            return;
        };
        let Some(anchor) = self
            .anchor
            .map(|kept| kept.anchor)
            .filter(|anchor| anchor.hash == at)
        else {
            return self.read_asked();
        };
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = match prove_state(&answer, &keys, Some(&at)) {
            Ok(proven) => proven,
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        let read = (self.read_issuers(anchor, &proven)).and_then(|issuers| {
            let pools = self.read_pools(anchor, &proven)?;
            Ok((issuers, pools))
        });
        let (issuers, pools) = match read {
            Ok(read) => read,
            Err(doubt) => return self.doubted(doubt),
        };
        self.tell_issuers(&issuers, false);
        self.tell_pools(&pools);
        self.read_asked();
    }

    /// Our transactions past their last ledger whose proof came: each is
    /// told, as one of ours the ledger validated. Only once the history is
    /// told up to the anchor, as when a walk left a gap: a walk dropped
    /// halfway, by a stall, leaves older transactions untold, and one of
    /// ours told before them would move the schemes' cursors past them.
    fn advance_ours(&mut self) {
        let Some(anchor) = self.anchor.map(|kept| kept.anchor) else {
            return;
        };
        if self.at.is_none_or(|at| at.0 < anchor.index) {
            return;
        }
        let asked: Vec<[u8; 32]> = (self.in_flight.iter())
            .filter(|t| t.looked_up && self.proofs.contains_key(&t.hash))
            .map(|t| t.hash)
            .collect();
        for hash in asked {
            match self.check(&hash, None, &anchor) {
                Checked::Holds => {
                    if let Some(proof) = self.proofs.remove(&hash) {
                        self.transaction(&proof.proven.decoded);
                    }
                }
                Checked::Missing | Checked::Waits => {}
                Checked::Lie => return,
            }
        }
    }

    /// The schemes' lookups whose proof came: each is told under the hash
    /// asked for.
    fn advance_lookups(&mut self) {
        let Some(anchor) = self.anchor.map(|kept| kept.anchor) else {
            return;
        };
        let proven: Vec<[u8; 32]> = (self.lookups.iter())
            .filter(|hash| self.proofs.contains_key(*hash))
            .copied()
            .collect();
        for hash in proven {
            match self.check(&hash, None, &anchor) {
                Checked::Holds => {
                    self.lookups.remove(&hash);
                    let Some(proof) = self.proofs.remove(&hash) else {
                        continue;
                    };
                    let record = proof.proven.decoded.record;
                    if !self.bookable_record(&record) {
                        self.failed = Some(TOO_MUCH_XRP);
                        return;
                    }
                    self.tell(News::Found { hash, record });
                }
                Checked::Missing | Checked::Waits => {}
                Checked::Lie => return,
            }
        }
    }

    /// Whether the proof of `hash` holds on the chain of `anchor`: its
    /// ledger is the anchor, or one its skip lists hold, which are asked
    /// for as needed; and it is `expected`, where a proven object or
    /// transaction names its ledger. One in another ledger is a lie, as a
    /// transaction applies in one ledger only.
    fn check(&mut self, hash: &[u8; 32], expected: Option<u32>, anchor: &Anchor) -> Checked {
        let Some(proof) = self.proofs.get(hash) else {
            return Checked::Missing;
        };
        let (index, ledger_hash) = (proof.proven.header.sequence, proof.proven.ledger_hash);
        if expected.is_some_and(|expected| expected != index) {
            self.proofs.remove(hash);
            self.doubted(Doubt::Ledger);
            return Checked::Lie;
        }
        if proof.holds {
            return Checked::Holds;
        }
        match ledger_proof(anchor, index, &ledger_hash, &self.lists) {
            LedgerProof::Holds => {
                if let Some(proof) = self.proofs.get_mut(hash) {
                    proof.holds = true;
                }
                Checked::Holds
            }
            LedgerProof::Wrong => {
                self.proofs.remove(hash);
                self.doubted(Doubt::Ledger);
                Checked::Lie
            }
            LedgerProof::Later => Checked::Waits,
            LedgerProof::Prove { at, key } => {
                self.ask_list(at, key);
                Checked::Waits
            }
        }
    }

    /// Asks for the skip list of `key` at the ledger of `at`, unless it is
    /// asked already, or as many as may be are.
    fn ask_list(&mut self, at: [u8; 32], key: [u8; 32]) {
        let asking = (self.pending.values())
            .filter(|(request, _)| matches!(request, Request::Ledger(..)))
            .count();
        let asked =
            (self.pending.values()).any(|(request, _)| *request == Request::Ledger(at, key));
        if !asked && asking < MAX_LIST_ASKS {
            self.send(Request::Ledger(at, key));
        }
    }

    /// Asks for the proofs that are needed, most needed first: the walk's,
    /// ours past their last ledger, the schemes' lookups, then the hints',
    /// at most [`MAX_PROVING`] at once.
    pub(super) fn dispatch_proofs(&mut self) {
        if self.phase == Phase::Disconnected || self.stalled_until_ms > 0 || self.failed.is_some() {
            return;
        }
        let mut wanted: Vec<([u8; 32], Option<u32>)> = Vec::new();
        if let Some(walk) = &self.walk {
            wanted.extend(
                walk.chains
                    .values()
                    .flatten()
                    .map(|(hash, at)| (*hash, Some(*at))),
            );
            if walk.chains.values().all(Option::is_none) {
                let first = walk.found.iter().take(MAX_PROVING);
                wanted.extend(first.map(|(at, hash)| (*hash, Some(at.0))));
            }
        }
        let ours = (self.in_flight.iter()).filter(|t| t.looked_up && !self.tried.contains(&t.hash));
        wanted.extend(ours.map(|t| (t.hash, None)));
        wanted.extend(self.lookups.iter().map(|hash| (*hash, None)));
        wanted.extend(self.hints.iter().map(|hash| (*hash, None)));
        // A proof is in flight from its request until its transaction's
        // bytes came: its branch waits for them meanwhile.
        let proving = |account: &Self| {
            (account.pending.values())
                .filter(|(request, _)| matches!(request, Request::TxProof(..)))
                .count()
                .saturating_add(account.branches.len())
        };
        for (hash, ledger) in wanted {
            if proving(self) >= MAX_PROVING {
                return;
            }
            let asked = self.branches.contains_key(&hash)
                || (self.pending.values())
                    .any(|(request, _)| matches!(request, Request::TxProof(of, _) if *of == hash));
            if !asked && !self.proofs.contains_key(&hash) {
                self.send(Request::TxProof(hash, ledger));
            }
        }
    }

    /// A page of the account's history: each transaction on it past what
    /// was told a hint, as if from the stream. A page that does not read,
    /// or an error, says nothing.
    pub(super) fn history_hinted(&mut self, outcome: Result<Value, ApiError>) {
        let Some(page) = outcome.ok().as_ref().and_then(api::account_tx_result) else {
            return;
        };
        let told = self.walk.as_ref().map(|walk| walk.told);
        for record in &page.transactions {
            if told.is_none_or(|told| (record.ledger_index, record.index) > told) {
                self.hinted(record);
            }
        }
    }

    /// A transaction the stream says validated: a hint, whose proof is
    /// asked for, so that the history finds it proven.
    pub(super) fn hinted(&mut self, record: &TxRecord) {
        let hash = record.hash;
        if !record.validated
            || self.phase == Phase::Disconnected
            || self.proofs.contains_key(&hash)
            || self.hints.len() >= super::MAX_HINTS
        {
            return;
        }
        self.hints.insert(hash);
        self.dispatch_proofs();
    }

    /// The proof of transaction `hash`, asked for in ledger `ledger` or
    /// wherever the server finds it: its branch starts above the leaf, so
    /// it waits for the transaction's bytes, which `tx` gives in binary.
    pub(super) fn tx_proof_read(
        &mut self,
        hash: [u8; 32],
        ledger: Option<u32>,
        outcome: Result<Value, ApiError>,
    ) {
        self.hints.remove(&hash);
        match outcome {
            Ok(result) => {
                let Some(answer) = api::tx_proof_result(&result) else {
                    return self.doubted(Doubt::Unreadable);
                };
                self.branches.insert(hash, (answer, None));
                self.send(Request::TxBlobs(hash, ledger));
            }
            Err(error) => {
                if self.not_proven(hash, ledger, &error) {
                    self.advance();
                }
            }
        }
    }

    /// The bytes and metadata of transaction `hash`, whose proof came: the
    /// leaf built from them, with the proof's branch, proves them, or the
    /// server lied. Bytes that do not read, or that the branch does not
    /// prove, are a lie. No error is: the proof just showed that the
    /// ledger holds the transaction, so a server that does not find it
    /// (`txnNotFound`, or `lgrNotFound`), or answers it without its
    /// metadata, has not saved it with its ledger yet, as an honest rippled
    /// for a moment after it validated or acquired a ledger. It is asked
    /// again a while later ([`UNSAVED_RETRY_MS`]), at most
    /// [`MAX_UNSAVED_TRIES`] times, and then taken as refusing it. Any other
    /// error is a refusal at once.
    pub(super) fn tx_blobs_read(
        &mut self,
        hash: [u8; 32],
        ledger: Option<u32>,
        outcome: Result<Value, ApiError>,
    ) {
        let not_yet = match &outcome {
            Ok(result) => api::tx_not_yet(result),
            Err(error) => matches!(error.error.as_str(), "txnNotFound" | "lgrNotFound"),
        };
        if not_yet {
            return self.unsaved_yet(hash, ledger);
        }
        let Some((answer, _)) = self.branches.remove(&hash) else {
            return;
        };
        let result = match outcome {
            Ok(result) => result,
            Err(_) => return self.stall(REFUSED),
        };
        let Some(blobs) = api::tx_binary_result(&result) else {
            return self.doubted(Doubt::Unreadable);
        };
        match prove_transaction(&answer, &blobs, &hash, &self.thread_keys()) {
            Ok(proven) => {
                self.counted.transactions = self.counted.transactions.saturating_add(1);
                self.keep_proof(hash, proven);
            }
            Err(error) => return self.doubted(Doubt::of(error)),
        }
        self.advance();
    }

    /// The server has not saved transaction `hash` with its ledger yet: its
    /// bytes are asked again a while later, or, past the bound, the server
    /// is taken as refusing them.
    fn unsaved_yet(&mut self, hash: [u8; 32], ledger: Option<u32>) {
        let now = self.now_ms;
        let Some((_, unsaved)) = self.branches.get_mut(&hash) else {
            return;
        };
        let tries = unsaved.map_or(0, |unsaved| unsaved.tries);
        if tries >= MAX_UNSAVED_TRIES {
            self.branches.remove(&hash);
            return self.stall(REFUSED);
        }
        *unsaved = Some(Unsaved {
            ledger,
            due_ms: Some(now.saturating_add(UNSAVED_RETRY_MS)),
            tries: tries.saturating_add(1),
        });
    }

    /// Asks again for the bytes whose time came.
    pub(super) fn ask_unsaved(&mut self) {
        if self.phase == Phase::Disconnected {
            return;
        }
        let now = self.now_ms;
        let due: Vec<([u8; 32], Option<u32>)> = (self.branches.iter_mut())
            .filter_map(|(hash, (_, unsaved))| {
                let unsaved = unsaved.as_mut()?;
                unsaved.due_ms.filter(|at| *at <= now)?;
                unsaved.due_ms = None;
                Some((*hash, unsaved.ledger))
            })
            .collect();
        for (hash, ledger) in due {
            self.send(Request::TxBlobs(hash, ledger));
        }
    }

    /// The server did not prove transaction `hash`: returns whether the
    /// account goes on. Not found where the server looked, a hint, a lookup
    /// or ours is not found, and nothing more; one the history names in a
    /// proven ledger is history the server no longer has only beyond the
    /// history bound, and within it the server's history is too short.
    fn not_proven(&mut self, hash: [u8; 32], ledger: Option<u32>, error: &ApiError) -> bool {
        match (error.error.as_str(), ledger) {
            ("txnNotFound" | "lgrNotFound", None) => {
                self.lookups.remove(&hash);
                if self.in_flight.iter().any(|t| t.hash == hash) && self.tried.len() < MAX_IN_FLIGHT
                {
                    self.tried.insert(hash);
                    self.counted.unproven = self.counted.unproven.saturating_add(1);
                }
                true
            }
            ("lgrNotFound", Some(at)) if self.beyond(at) => {
                self.lost(hash);
                true
            }
            ("lgrNotFound", Some(_)) => {
                self.stall(HISTORY);
                false
            }
            ("txnNotFound", Some(_)) => {
                self.doubted(Doubt::History);
                false
            }
            ("unknownCmd", _) => {
                self.stall(NO_PROOFS);
                false
            }
            _ => {
                self.stall(REFUSED);
                false
            }
        }
    }

    /// Whether ledger `ledger`, a proven index, is beyond the history bound
    /// from the anchor.
    fn beyond(&self, ledger: u32) -> bool {
        (self.anchor).is_some_and(|kept| kept.anchor.index.saturating_sub(ledger) > HISTORY_BOUND)
    }

    /// The transaction `hash` is history the server no longer has: the
    /// chains that name it end there, and the walk leaves a gap.
    fn lost(&mut self, hash: [u8; 32]) {
        let Some(walk) = &mut self.walk else {
            return;
        };
        let mut ended = Vec::new();
        for (key, next) in &mut walk.chains {
            if next.is_some_and(|(of, _)| of == hash) {
                *next = None;
                ended.push(*key);
            }
        }
        for key in ended {
            walk.take_up(key);
        }
        walk.found.retain(|_, of| *of != hash);
        self.left_a_gap();
    }

    /// Keeps the proof of `hash`. Past [`MAX_PROOFS`], one no one needs
    /// goes, else the newest the walk found, which is proven again when it
    /// is told.
    fn keep_proof(&mut self, hash: [u8; 32], proven: ProvenTx) {
        if self.proofs.len() >= MAX_PROOFS && !self.proofs.contains_key(&hash) {
            let Some(victim) = self.needed_least() else {
                return;
            };
            self.proofs.remove(&victim);
        }
        self.proofs.insert(
            hash,
            Proof {
                proven,
                holds: false,
            },
        );
    }

    /// The proof kept that is needed least: one nothing names, else the
    /// one the walk tells last.
    fn needed_least(&self) -> Option<[u8; 32]> {
        let walk = self.walk.as_ref();
        let named = |hash: &[u8; 32]| {
            walk.is_some_and(|walk| {
                walk.found.values().any(|of| of == hash)
                    || walk.chains.values().flatten().any(|(of, _)| of == hash)
            }) || self.lookups.contains(hash)
                || self.in_flight.iter().any(|t| t.hash == *hash)
        };
        if let Some(hash) = self.proofs.keys().find(|hash| !named(hash)) {
            return Some(*hash);
        }
        let last = walk.and_then(|walk| {
            walk.found
                .values()
                .rev()
                .find(|hash| self.proofs.contains_key(*hash))
        });
        last.copied()
    }

    /// A skip list asked for to prove a transaction's ledger.
    pub(super) fn ledger_list_read(
        &mut self,
        at: [u8; 32],
        key: [u8; 32],
        outcome: Result<Value, ApiError>,
    ) {
        if let Err(error) = &outcome
            && error.error == "lgrNotFound"
        {
            return self.stall(HISTORY);
        }
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let list = match prove_state(&answer, &[key], Some(&at)) {
            Ok(proven) => match proven.object(&key) {
                Some(Some(object)) => match SkipList::parse(object) {
                    Ok(list) => Some(list),
                    Err(_) => return self.doubted(Doubt::SkipList),
                },
                _ => None,
            },
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        if self.lists.len() >= MAX_LISTS {
            self.lists.clear();
        }
        self.lists.insert((at, key), list);
        self.advance();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use alloc::string::ToString;

    use super::*;
    use crate::account::{FIRST, News, TESTNET_NETWORK_ID};
    use crate::anchor::KeptAnchor;
    use crate::api::BalanceChange;
    use crate::header::LedgerHeader;
    use crate::keys::Wallet;
    use crate::records::Decoded;
    use dassie_ledger::Name;
    use dassie_settlement::ExternalAsset;

    const ANCHOR: Anchor = Anchor {
        index: 50,
        close_time: 800_000_000,
        hash: [9; 32],
    };

    /// An account through which `scheme` settles, synced at ledger 10, its
    /// scheme's cursor there.
    fn synced_account(scheme: Name) -> XrplAccount {
        let mut account = XrplAccount::new(FIRST, &[scheme], "", &[1; 32], &[]).unwrap();
        account.phase = Phase::Synced;
        account.at = Some((10, u32::MAX));
        account.sequence = 5;
        account.balance = Some(1_000_000_000);
        account.validated = 10;
        account.follow(scheme, Some((10, u32::MAX)), true);
        account
    }

    /// The account, anchored at [`ANCHOR`].
    fn anchor(account: &mut XrplAccount) {
        account.anchor = Some(KeptAnchor::of(ANCHOR, None));
        account.validated = ANCHOR.index;
    }

    /// A proof, its ledger proven, of a Payment `account` signed with
    /// `sequence`, in ledger `ledger`, whose metadata names no object's
    /// thread.
    fn proof(account: AccountId, hash: [u8; 32], ledger: u32, sequence: u32) -> Proof {
        let header = LedgerHeader {
            sequence: ledger,
            total_drops: 1,
            parent_hash: [0; 32],
            transaction_hash: [0; 32],
            account_hash: [0; 32],
            parent_close_time: 0,
            close_time: 0,
            close_time_resolution: 10,
            close_flags: 0,
        };
        let record = TxRecord {
            hash,
            ledger_index: ledger,
            index: 0,
            validated: true,
            account,
            is_payment: true,
            destination: Some(Wallet::from_entropy(&[2; 32]).account()),
            destination_tag: None,
            sequence,
            fee: 12,
            result: "tesSUCCESS".to_string(),
            delivered: Some(1_000),
            delivered_token: None,
            memo: None,
            changes: alloc::vec![BalanceChange {
                account,
                before: 1_000_000_000,
                after: 999_998_988,
                owner_count: Some(0),
            }],
            lines: Vec::new(),
        };
        Proof {
            proven: ProvenTx {
                ledger_hash: header.hash(),
                header,
                decoded: Decoded {
                    record,
                    threads: Vec::new(),
                    uses: (sequence, 1),
                },
            },
            holds: true,
        }
    }

    // spec/designs/xrpl-validation.md#6-the-account-on-proofs: the schemes
    // book the history in ledger order. A walk a stall dropped halfway
    // leaves untold what it had not reached before the anchor; one of ours
    // proven meanwhile is told only once the history is told up to the
    // anchor, or it would move the schemes' cursors past what was left.
    #[test]
    fn ours_is_told_only_once_the_history_reaches_the_anchor() {
        let scheme = dassie_ledger::name!(crate::scheme::SCHEME_ID);
        let mut account = synced_account(scheme);
        let to = Wallet::from_entropy(&[2; 32]).account();
        let hash = account.sign_payment(scheme, to, 1_000, Vec::new()).unwrap();
        assert!(account.execute(scheme, hash));
        anchor(&mut account);
        // Past its last ledger, with the account's sequence past its own:
        // its proof is asked for.
        account.proven_sequence = 6;
        account.expire();
        account.told();
        let us = account.account();
        account.proofs.insert(hash, proof(us, hash, 30, 5));
        // Told up to ledger 40 alone: a walk to the anchor was dropped.
        account.at = Some((40, u32::MAX));
        account.advance();
        assert!(account.news().is_empty(), "{:?}", account.news());
        assert!(account.holds_transaction(&hash));
        // Told up to the anchor, as after a walk that left a gap: ours is
        // told.
        account.at = Some((ANCHOR.index, u32::MAX));
        account.advance();
        assert!(matches!(
            account.news(),
            [News::Transaction { ours: Some(ours), .. }] if ours.scheme == scheme
        ));
        assert!(!account.holds_transaction(&hash));
        account.check_invariants();
    }

    // A walk that left a gap while the account was synced tells the
    // schemes the balance the anchor proves, as when they sync, so they
    // match their books to it at once.
    #[test]
    fn a_gap_left_while_synced_syncs_the_schemes_again() {
        let scheme = dassie_ledger::name!(crate::scheme::SCHEME_ID);
        let mut account = synced_account(scheme);
        anchor(&mut account);
        account.told();
        let root = Root {
            account: account.account(),
            balance: 900_000_000,
            sequence: 5,
            owner_count: 0,
            flags: 0,
            transfer_rate: 0,
            regular_key: None,
            thread: None,
        };
        let walk = |gap| Walk {
            view: View {
                anchor: ANCHOR,
                recent: None,
                root: Some(root),
                lines: BTreeMap::new(),
                threads: BTreeMap::new(),
                issuers: BTreeMap::new(),
                fees: None,
                pools: BTreeMap::new(),
            },
            told: (10, u32::MAX),
            chains: BTreeMap::new(),
            found: BTreeMap::new(),
            resumes: BTreeMap::new(),
            gap,
        };
        account.walk = Some(walk(false));
        account.finish_walk();
        assert!(
            !account
                .news()
                .iter()
                .any(|news| matches!(news, News::Synced(_))),
            "{:?}",
            account.news()
        );
        account.told();
        account.walk = Some(walk(true));
        account.finish_walk();
        assert!(account.news().iter().any(|news| matches!(
            news,
            News::Synced(info) if info.balance == root.balance && info.ledger_index == ANCHOR.index
        )));
        account.check_invariants();
    }

    // A proof whose metadata does not thread an object whose chain names it
    // doubts the history, and goes, so the next connection proves it anew
    // instead of doubting it for good; and a line followed anew drops every
    // proof kept, each read for the objects followed before it.
    #[test]
    fn a_proof_that_does_not_thread_an_object_is_proven_anew() {
        let scheme = dassie_ledger::name!(crate::dsusd::SCHEME_ID);
        let mut account = synced_account(scheme);
        anchor(&mut account);
        account.network_id = Some(TESTNET_NETWORK_ID);
        let us = account.account();
        let line = records::line_key(us, crate::dsusd::ISSUER, crate::dsusd::CURRENCY);
        let hash = [7; 32];
        let view = View {
            anchor: ANCHOR,
            recent: None,
            root: None,
            lines: BTreeMap::new(),
            threads: BTreeMap::new(),
            issuers: BTreeMap::new(),
            fees: None,
            pools: BTreeMap::new(),
        };
        account.walk = Some(Walk {
            view,
            told: (10, u32::MAX),
            chains: BTreeMap::from([(line, Some((hash, 30)))]),
            found: BTreeMap::new(),
            resumes: BTreeMap::new(),
            gap: false,
        });
        account.proofs.insert(hash, proof(us, hash, 30, 5));
        account.advance();
        assert_eq!(account.doubts(Doubt::History), 1);
        assert!(account.proofs.is_empty(), "proven anew");
        account.proofs.insert(hash, proof(us, hash, 30, 5));
        account.follow_line(crate::dsusd::LINE);
        assert!(account.proofs.is_empty(), "read for the objects followed");
        account.check_invariants();
    }
}
