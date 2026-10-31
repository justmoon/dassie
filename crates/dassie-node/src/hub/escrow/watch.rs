//! What following a hub's escrow holds of the escrow itself: its cursor,
//! its subscription, the hints its server gave, the walks of its history,
//! the search for its issuer's hold, what it attested and what it submits.
//! Each call takes the [`Link`] it follows the escrow on, whose verifier
//! proves what it asks, and through which it asks and hands its host what
//! to do.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_hub::escrow::{
    IssuerHold, check_proven_escrow, clawback_of, deposit_of, escrow_keys, hold_of, issuer_keys,
    outcome_of, withdrawal_outcome,
};
use dassie_hub::operation::{Deposit, Issuer, ValidatorOperation};
use dassie_hub::service::MAX_WAITING_DEPOSITS;
use dassie_xrpl::AccountId;
use dassie_xrpl::account::HISTORY_BOUND;
use dassie_xrpl::anchor::ProvenState;
use dassie_xrpl::api::{self, PAGE, TxRecord};
use dassie_xrpl::codec::{Payment, Transact};
use dassie_xrpl::records::{self, Decoded, Thread};
use dassie_xrpl::unl::Anchor;
use dassie_xrpl::verifier::{self, ANCHOR_EVERY_MS, Query, QueryId, Verdict};
use serde_json::Value;

use super::link::{Asked, Link, Parked};
use super::{
    ANCHOR_FRESH_MS, Chain, ESCROW_CHECK_EVERY_MS, EscrowView, ISSUER_CHECK_EVERY_MS, MAX_HINTS,
    MAX_QUEUED_SUBMITS, Out, RESCAN_LEDGERS, RESUBMIT_MS, chain as judge_chain,
};
use crate::store::EscrowCursor;

/// Seconds from the Unix epoch to the XRP Ledger's, in which close times
/// are counted.
const XRPL_EPOCH_S: u64 = 946_684_800;

/// The most transactions proven from hints kept until a walk passes them:
/// what the walk steps through rather than proving each in turn. Several
/// times the hints, since a walk goes up only as far as the hints proven
/// cover, and lets go of what it stepped through only once it ends.
const MAX_PROVEN: usize = 8 * MAX_HINTS;

/// The most of them kept with their record, for the walk to hand on once
/// what waits for the hub has room: one that finds no room among these is
/// proven again when the walk reaches it.
const MAX_PROVEN_RECORDS: usize = MAX_HINTS;

/// The most steps one input takes along a chain, through transactions
/// proven already from hints.
const MAX_CHAIN_STEPS: usize = MAX_PROVEN + 1;

/// The most hints proven at once, leaving the verifier room for the
/// escrow's and the issuer's state.
const MAX_HINT_QUERIES: usize = verifier::MAX_QUERIES / 2;

/// How long after a server found no transaction it hinted at the watcher
/// asks for its proof again, and how many times before it connects anew.
const HINT_RETRY_MS: u64 = 2 * ANCHOR_EVERY_MS;
const MAX_HINT_TRIES: u32 = 4;

/// The most transactions remembered as attested, to attest each once.
const MAX_REMEMBERED: usize = 4096;

/// The most outcomes of the escrow's own transactions held until the hub
/// gives their sequence to something.
const MAX_HELD: usize = 64;

/// What one of the verifier's queries is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// The proof of a transaction hinted at.
    Hint([u8; 32]),
    /// The escrow's setup, at the anchor.
    Escrow,
    /// The issuer's hold on a token hub's escrow, at the anchor.
    Issuer,
    /// The escrow's objects at the anchor: a walk's start.
    Walk,
    /// The proof of the next transaction on the chain of the object of
    /// this key.
    Step([u8; 32]),
    /// The issuer's hold as of the ledger before the one it is proven the
    /// same since.
    HoldBefore,
}

/// One object's chain in a walk of the escrow's history: the transactions
/// that changed it, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectChain {
    /// The object's key: the escrow's account root, or a token hub's line.
    key: [u8; 32],
    /// The next transaction to find, by its hash and its proven ledger;
    /// none once the chain reached the walk's floor, or the transaction
    /// that made the object.
    next: Option<Thread>,
    /// Its proof's query, while one is open.
    query: Option<QueryId>,
    /// How often the server found no such transaction.
    tries: u32,
    /// When to ask for its proof again, after the server found none.
    retry_at_ms: u64,
    /// The next transaction, found, while it waits for room among what
    /// waits for the hub, with the one before it on the chain.
    waiting: Option<(Box<TxRecord>, Option<Thread>)>,
}

/// A walk of the escrow's history back from its objects proven at `top`
/// to `floor`, where the last walk ended: once each chain ended, every
/// transaction that changed them in between was found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Walk {
    top: u32,
    floor: u32,
    /// The root's chain, and a token hub's line's.
    chains: Vec<ObjectChain>,
}

/// A search for the ledger the issuer's hold last changed in: back from the
/// later of its objects' last changes, the two are proven as of the ledger
/// before, until the hold there differs, or the hold is known the same over
/// a ledger it was named at before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HoldSearch {
    /// The hold proven at an anchor.
    hold: IssuerHold,
    /// That anchor's ledger.
    top: u32,
    /// From this ledger to `top`, the hold is proven as it is: the ledger
    /// before is proven next.
    since: u32,
    /// The query for the ledger before, while one is open.
    query: Option<QueryId>,
}

/// A transaction proven from a hint before a walk reached it: what the
/// walk needs to step past it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ahead {
    /// The ledger it is in.
    ledger: u32,
    /// For each of the walked objects it changed, the transaction that
    /// changed it before.
    threads: Vec<([u8; 32], Option<Thread>)>,
    /// Its record, while what waits for the hub has no room for it; none
    /// once it was handed on, when the walk only steps past it.
    record: Option<Box<TxRecord>>,
}

/// Reading the escrow's history from the server, for hints: at a
/// connection, from the cursor, a window back; and again from the ledger
/// of a hint the stream brought without room.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Reading {
    from: u32,
    to: u32,
    marker: Option<Value>,
    /// Whether a page is asked for.
    asking: bool,
    /// Each transaction the history holds up to this ledger was hinted.
    through: u32,
}

/// A transaction the server hinted at, not yet proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hint {
    /// The ledger the server named for it: its proofs are asked for oldest
    /// first.
    ledger: u32,
    /// Its proof's query, while one is open.
    query: Option<QueryId>,
    /// How often the server found no such transaction.
    tries: u32,
    /// When to ask for its proof again, after the server found none.
    retry_at_ms: u64,
}

/// An escrow transaction a quorum signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Submit {
    /// The escrow's sequence it uses, if it reads as a Payment: a
    /// withdrawal's, kept and sent again while the withdrawal is open. One
    /// that does not read is sent once.
    pub(super) sequence: Option<u32>,
    blob: Vec<u8>,
    /// When it was last sent on this connection.
    sent_ms: Option<u64>,
}

/// A deposit attested whose attestation the hub has not recorded yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiting {
    deposit: Deposit,
    /// When it was last handed to the validator to attest.
    attested_ms: u64,
}

/// Where the escrow's subscription is, on a link whose server was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Unsubscribed,
    /// Subscribed, and reading the history from the cursor.
    CatchingUp,
    Following,
}

/// Follows one hub's escrow, on a [`Link`].
#[derive(Debug)]
pub(super) struct EscrowWatcher {
    escrow: AccountId,
    cursor: Option<EscrowCursor>,
    stream: Stream,
    /// What each of the verifier's open queries is for.
    queries: BTreeMap<QueryId, Purpose>,
    hints: BTreeMap<[u8; 32], Hint>,
    /// The escrow's history being read for hints.
    reading: Option<Reading>,
    /// The lowest ledger of a hint the stream brought without room: the
    /// history is read again from there.
    overflow_from: Option<u32>,
    /// The newest ledger the stream said closed.
    streamed_to: u32,
    /// Hinted transactions proven, kept for the walk, by hash.
    proven: BTreeMap<[u8; 32], Ahead>,
    /// The same, by ledger: the newest make room for older ones.
    proven_order: BTreeSet<(u32, [u8; 32])>,
    /// The keys of the objects whose history is walked: the escrow's
    /// account root, and a token hub's line.
    walk_keys: Vec<[u8; 32]>,
    /// The walk under way.
    walk: Option<Walk>,
    /// The ledger the last walk reached: every transaction that changed
    /// the escrow's objects above where the first began, and at or below
    /// it, was found. The cursor passes no ledger beyond it, nor beyond the
    /// newest proven one.
    walked_to: u32,
    remembered: BTreeSet<[u8; 32]>,
    remembered_order: VecDeque<[u8; 32]>,
    held: VecDeque<TxRecord>,
    /// The escrow transactions a quorum signed, in the order they came.
    pub(super) submits: VecDeque<Submit>,
    /// Deposits attested whose attestation the hub has not recorded yet,
    /// by transaction: each holds the cursor below its ledger.
    deposits: BTreeMap<[u8; 32], Waiting>,
    /// The newest state of the issuer's hold it proved, as of the ledger it
    /// was proven at: not the ledger it changed in, which a search finds.
    proven_issuer: Option<Issuer>,
    /// The search for the ledger the issuer's hold last changed in.
    search: Option<HoldSearch>,
    /// The hold last named, with the newest ledger it was proven at: so
    /// the next search ends once it reaches that stretch.
    named: Option<(Issuer, u32)>,
    /// A hold a search could not name on this connection, with the
    /// stretch it was proven the same over, from the ledger after one its
    /// server keeps no state of: a search that reaches that stretch ends
    /// there, naming nothing, rather than proving the same again.
    unnamed: Option<(IssuerHold, u32, u32)>,
    /// When the hub was last seen to let go of the escrow.
    thawed_ms: Option<u64>,
    /// The newest anchor, and when it was proven: judged against this
    /// escrow's cursor.
    anchored: Option<(Anchor, u64)>,
    escrow_checked_ms: Option<u64>,
    /// The anchor the escrow was last checked at, by its ledger: a check
    /// at it judged every transaction proven up to it.
    escrow_checked_at: Option<u32>,
    /// Whether the escrow is to be checked once the anchor is newer than
    /// the last check's: one of its own transactions applied after it, or
    /// a founding asked.
    escrow_check_wanted: bool,
    attested_escrow: Option<u32>,
    pub(super) issuer_checked_ms: Option<u64>,
    attested_issuer: Option<Issuer>,
    /// What it attested of the issuer's hold before `attested_issuer`: what
    /// the validator last heard, should the host drop that.
    attested_issuer_before: Option<Issuer>,
    /// What the validator's ledger said last.
    view: Option<EscrowView>,
    /// Transactions attested that the validator has not taken yet, with
    /// the ledgers they are in.
    untaken: BTreeMap<[u8; 32], u32>,
    /// The highest ledger a persisted cursor may name as attested, below a
    /// transaction whose attestation was dropped: a restart reads it again.
    ceiling: Option<u32>,
    /// The cursor last persisted.
    persisted: Option<EscrowCursor>,
}

/// Building and reading it.
impl EscrowWatcher {
    /// A watcher of `escrow`, from where it left off.
    pub(super) fn new(escrow: AccountId, cursor: Option<EscrowCursor>) -> Self {
        Self {
            escrow,
            cursor,
            stream: Stream::Unsubscribed,
            queries: BTreeMap::new(),
            hints: BTreeMap::new(),
            reading: None,
            overflow_from: None,
            streamed_to: 0,
            proven: BTreeMap::new(),
            proven_order: BTreeSet::new(),
            walk_keys: Vec::new(),
            walk: None,
            walked_to: cursor.map_or(0, |cursor| cursor.scanned_to.saturating_sub(RESCAN_LEDGERS)),
            remembered: BTreeSet::new(),
            remembered_order: VecDeque::new(),
            held: VecDeque::new(),
            submits: VecDeque::new(),
            deposits: BTreeMap::new(),
            proven_issuer: None,
            search: None,
            named: None,
            unnamed: None,
            thawed_ms: None,
            anchored: None,
            escrow_checked_ms: None,
            escrow_checked_at: None,
            escrow_check_wanted: false,
            attested_escrow: None,
            issuer_checked_ms: None,
            attested_issuer: None,
            attested_issuer_before: None,
            view: None,
            untaken: BTreeMap::new(),
            ceiling: None,
            persisted: cursor,
        }
    }

    /// Whether any of the verifier's queries is open for it.
    pub(super) fn querying(&self) -> bool {
        !self.queries.is_empty()
    }

    /// Tells the verifier of `link` the threads it reads proven
    /// transactions for, which are no part of the verifier's state: after
    /// the verifier was taken back from before a restart.
    pub(super) fn thread(&self, link: &mut Link) {
        if !self.walk_keys.is_empty() {
            link.verifier_mut().thread(self.walk_keys.clone());
        }
    }

    /// Whether it follows the escrow's stream now.
    pub(super) fn following(&self) -> bool {
        self.stream == Stream::Following
    }

    /// Whether it is subscribed to the escrow on its link.
    pub(super) const fn subscribed(&self) -> bool {
        matches!(self.stream, Stream::Following | Stream::CatchingUp)
    }

    /// How far it followed the escrow.
    pub(super) const fn cursor(&self) -> Option<EscrowCursor> {
        self.cursor
    }

    /// The cursor last persisted.
    pub(super) const fn persisted(&self) -> Option<EscrowCursor> {
        self.persisted
    }

    /// The newest anchor its verifier proved, if any.
    pub(super) fn anchor(&self) -> Option<Anchor> {
        self.anchored.map(|(anchor, _)| anchor)
    }

    /// When the newest anchor was proven, if any.
    pub(super) fn anchored_ms(&self) -> Option<u64> {
        self.anchored.map(|(_, at)| at)
    }

    /// Whether the newest anchor is fresh enough at `now_ms` to judge the
    /// newest ledger by ([`ANCHOR_FRESH_MS`]).
    pub(super) fn fresh(&self, now_ms: u64) -> bool {
        self.anchored
            .is_some_and(|(_, at)| now_ms.saturating_sub(at) <= ANCHOR_FRESH_MS)
    }

    /// Whether `state` is as of the newest anchor, or later, and that
    /// anchor fresh at `now_ms`: a state proven at an older anchor, asked
    /// before a new connection anchored anew and answered after, is not
    /// judged as the newest ledger.
    fn current(&self, state: &ProvenState, now_ms: u64) -> bool {
        self.fresh(now_ms)
            && self
                .anchor()
                .is_some_and(|anchor| state.header.sequence >= anchor.index)
    }

    /// The newest state of the issuer's hold it proved, as of the ledger it
    /// was proven at, if any.
    pub(super) const fn proven_issuer(&self) -> Option<Issuer> {
        self.proven_issuer
    }

    /// Asserts what holds of it, on `link`.
    pub(super) fn check_invariants(&self, link: &Link) {
        assert!(self.hints.len() <= MAX_HINTS, "hints are bounded");
        assert!(self.held.len() <= MAX_HELD, "held outcomes are bounded");
        assert!(self.remembered.len() <= MAX_REMEMBERED);
        assert_eq!(self.remembered.len(), self.remembered_order.len());
        assert!(self.submits.len() <= MAX_QUEUED_SUBMITS);
        assert!(
            self.deposits.len() <= MAX_WAITING_DEPOSITS,
            "deposits waiting for the hub are bounded"
        );
        let hint_queries = self
            .hints
            .values()
            .filter(|hint| hint.query.is_some())
            .count();
        assert!(hint_queries <= MAX_HINT_QUERIES, "hint proofs are bounded");
        assert!(self.proven.len() <= MAX_PROVEN, "proofs kept are bounded");
        assert_eq!(self.proven.len(), self.proven_order.len());
        assert!(
            (self.proven.values())
                .filter(|ahead| ahead.record.is_some())
                .count()
                <= MAX_PROVEN_RECORDS,
            "records kept are bounded"
        );
        self.check_queries();
        if let (Some(cursor), Some((anchor, _))) = (self.cursor, self.anchored) {
            assert!(
                cursor.scanned_to <= cursor.newest_ledger.max(anchor.index),
                "the cursor passes no ledger beyond the newest proven"
            );
        }
        if !link.is_connected() {
            assert!(self.hints.is_empty(), "hints are a connection's");
            assert!(self.reading.is_none() && self.overflow_from.is_none());
        }
        if self.subscribed() {
            assert!(
                link.checked(),
                "the escrow is subscribed to once its server is checked"
            );
        }
        if self.reading.is_some() {
            assert!(self.subscribed(), "the history is read once subscribed");
        }
    }

    /// Asserts what holds of its queries, its walk and its search.
    fn check_queries(&self) {
        assert!(self.walk_keys.len() <= 2, "the root and a line");
        for (id, purpose) in &self.queries {
            match purpose {
                Purpose::Hint(hash) => assert_eq!(
                    self.hints.get(hash).and_then(|hint| hint.query),
                    Some(*id),
                    "a hint's query is its own"
                ),
                Purpose::Step(key) => assert!(
                    (self.walk.iter().flat_map(|walk| &walk.chains))
                        .any(|chain| chain.key == *key && chain.query == Some(*id)),
                    "a step's query is its chain's"
                ),
                Purpose::HoldBefore => assert_eq!(
                    self.search.and_then(|search| search.query),
                    Some(*id),
                    "the search's query is its own"
                ),
                Purpose::Escrow | Purpose::Issuer | Purpose::Walk => {}
            }
        }
        if let Some(walk) = &self.walk {
            assert!(walk.chains.len() <= self.walk_keys.len());
            assert!(walk.floor == self.walked_to && walk.floor < walk.top);
            for chain in &walk.chains {
                assert!(
                    chain
                        .next
                        .is_none_or(|(_, ledger)| walk.floor < ledger && ledger <= walk.top),
                    "a chain names ledgers of its walk"
                );
                assert!(chain.waiting.is_none() || chain.next.is_some());
            }
        }
        if let Some(search) = self.search {
            assert!(search.since <= search.top, "a search goes back");
        }
    }
}

/// Inputs.
impl EscrowWatcher {
    /// The validator took the attestation of `transaction`: it is
    /// remembered, and the cursor moves on once none is waiting before it.
    /// A deposit's waits on until the hub recorded it.
    pub(super) fn taken(&mut self, link: &mut Link, transaction: Option<[u8; 32]>) {
        let Some(hash) = transaction else {
            return;
        };
        self.untaken.remove(&hash);
        match self.deposits.get_mut(&hash) {
            Some(waiting) => waiting.attested_ms = link.now_ms(),
            None => self.remember(hash),
        }
        self.advance_cursor(link);
    }

    /// The host dropped `operation`, the attestation of `transaction`,
    /// before the validator took it ([`super::Watcher::dropped`]).
    pub(super) fn dropped(
        &mut self,
        link: &mut Link,
        operation: &ValidatorOperation,
        transaction: Option<[u8; 32]>,
    ) {
        match operation {
            ValidatorOperation::AttestIssuer(issuer) if self.attested_issuer == Some(*issuer) => {
                self.attested_issuer = self.attested_issuer_before.take();
            }
            ValidatorOperation::AttestEscrow { sequence }
                if self.attested_escrow == Some(*sequence) =>
            {
                self.attested_escrow = None;
            }
            _ => {}
        }
        let Some(hash) = transaction else {
            return;
        };
        let Some(ledger) = self.untaken.remove(&hash) else {
            return;
        };
        if !self.deposits.contains_key(&hash) {
            let below = ledger.saturating_sub(1);
            self.ceiling = Some(self.ceiling.map_or(below, |ceiling| ceiling.min(below)));
        }
        self.advance_cursor(link);
    }

    /// What its link's server hinted at is forgotten with the connection,
    /// and the escrow is subscribed to no more: the next connection reads
    /// the history again from the cursor. The verifier asks its open
    /// queries again on the next, a walk's and a search's among them.
    pub(super) fn forget_connection(&mut self, link: &mut Link) {
        self.stream = Stream::Unsubscribed;
        let hinted: Vec<QueryId> = (self.queries.iter())
            .filter(|(_, purpose)| matches!(purpose, Purpose::Hint(_)))
            .map(|(id, _)| *id)
            .collect();
        for id in hinted {
            self.queries.remove(&id);
            link.verifier_mut().cancel(id);
        }
        self.hints.clear();
        self.reading = None;
        self.overflow_from = None;
        self.streamed_to = 0;
        // The next server may keep more history than this one.
        self.unnamed = None;
        // What was sent on it goes again on the next, once following.
        for submit in &mut self.submits {
            submit.sent_ms = None;
        }
    }

    /// Submits a signed escrow transaction once it follows the escrow and
    /// the issuer does not hold it, and again while its withdrawal stays
    /// open.
    pub(super) fn submit(&mut self, link: &mut Link, blob: Vec<u8>) {
        let sequence = Payment::decode(&blob)
            .ok()
            .map(|payment| payment.sequence());
        if sequence.is_some() {
            // Signed anew, it takes the place of what was kept of its
            // sequence.
            self.submits.retain(|kept| kept.sequence != sequence);
        }
        self.prune_submits();
        if self.submits.len() >= MAX_QUEUED_SUBMITS {
            // A hub may have more withdrawals open than are kept: the one
            // of the highest sequence that went once already makes room,
            // and is not sent again. The escrow's sequences apply in order,
            // so only those above it could wait on it, never one below; so
            // it makes room only while none kept above it waits to be sent,
            // which would wait on it for good were it lost. Otherwise, as in
            // a burst whose lowest are in flight and the rest wait, the
            // highest not sent yet, this one or one kept, is dropped: none
            // of the others waits on it.
            let unsent = (self.submits.iter())
                .filter(|kept| kept.sent_ms.is_none())
                .map(|kept| kept.sequence)
                .max();
            let sent = (self.submits.iter().enumerate())
                .filter(|(_, kept)| kept.sent_ms.is_some() && Some(kept.sequence) > unsent)
                .max_by_key(|(_, kept)| kept.sequence)
                .map(|(index, _)| index);
            if let Some(index) = sent {
                self.submits.remove(index);
            } else {
                link.push(Out::Problem(
                    "too many escrow transactions wait for the XRPL server: one was dropped",
                ));
                let highest = (self.submits.iter().enumerate())
                    .filter(|(_, kept)| kept.sent_ms.is_none() && kept.sequence > sequence)
                    .max_by_key(|(_, kept)| kept.sequence)
                    .map(|(index, _)| index);
                let Some(index) = highest else {
                    return;
                };
                self.submits.remove(index);
            }
        }
        self.submits.push_back(Submit {
            sequence,
            blob,
            sent_ms: None,
        });
        self.send_submits(link);
    }

    /// Looks again at outcomes held, and attests again the deposits the hub
    /// has not recorded that are due: what a tick does for the escrow
    /// before the verifier's decisions are carried out.
    pub(super) fn tick(&mut self, link: &mut Link) {
        // Each was held with room: taken again, each has it again.
        let held = std::mem::take(&mut self.held);
        for record in held {
            let _ = self.record(link, &record);
        }
        self.retry_deposits(link);
    }

    /// The stream said ledger `index` closed.
    pub(super) fn closed(&mut self, index: u32) {
        self.streamed_to = self.streamed_to.max(index);
    }

    /// A founding asks whether a quorum checked the escrow: unless `view`
    /// says one has, it is checked once the anchor is newer than the last
    /// check's.
    pub(super) const fn want_check(&mut self, view: &EscrowView) {
        if !view.checked {
            self.escrow_check_wanted = true;
        }
    }
}

/// Requests and answers of its own.
impl EscrowWatcher {
    /// The link's server failed to answer, or what it answered of the
    /// escrow does not read: the link waits, and asks it where it stands
    /// again later, and the history is read again once it follows the
    /// server again.
    pub(super) fn wait(&mut self, link: &mut Link) {
        self.reading = None;
        self.overflow_from = None;
        self.stream = Stream::Unsubscribed;
        link.wait();
    }

    /// The link's server is on the hub's network and on the chain followed,
    /// its validated ledger `validated`: subscribe to the escrow, and read
    /// its history from the cursor, a window back, up to there.
    pub(super) fn catch_up(&mut self, link: &mut Link, validated: u32) {
        let from = self
            .cursor
            .map_or(0, |cursor| cursor.scanned_to.saturating_sub(RESCAN_LEDGERS));
        self.stream = Stream::CatchingUp;
        self.reading = Some(Reading {
            from,
            to: validated,
            marker: None,
            asking: false,
            through: from.saturating_sub(1),
        });
        let escrow = self.escrow;
        link.ask(Asked::Subscribe, |id| api::subscribe(id, escrow));
    }

    /// The newest ledger proven, and when it closed.
    pub(super) fn newest(&self) -> Option<(u32, u64)> {
        self.cursor
            .map(|cursor| (cursor.newest_ledger, cursor.newest_close_time))
    }

    /// Asks for the next page of the escrow's history, once there is room
    /// for a whole page of hints.
    pub(super) fn read_history(&mut self, link: &mut Link) {
        let room = self.hints.len().saturating_add(PAGE as usize) <= MAX_HINTS;
        let Some(reading) = self.reading.as_mut() else {
            return;
        };
        if reading.asking || !room {
            return;
        }
        reading.asking = true;
        let (from, to, marker) = (reading.from, reading.to, reading.marker.clone());
        let escrow = self.escrow;
        link.ask(Asked::History, |id| {
            api::account_tx(id, escrow, from, to, marker.as_ref())
        });
    }

    /// A page of the escrow's history: each transaction a hint, then the
    /// next page, or the stream from here on.
    pub(super) fn history(&mut self, link: &mut Link, result: &Value) {
        let Some(page) = api::account_tx_result(result) else {
            link.push(Out::Problem(
                "the XRPL server's history of the escrow is unreadable",
            ));
            return self.wait(link);
        };
        for record in &page.transactions {
            self.hinted(record);
        }
        let Some(reading) = self.reading.as_mut() else {
            return;
        };
        reading.asking = false;
        if page.marker.is_some() {
            // The next page may hold more of this one's newest ledger.
            let newest = (page.transactions.iter())
                .map(|record| record.ledger_index)
                .max();
            if let Some(newest) = newest {
                reading.through = reading.through.max(newest.saturating_sub(1));
            }
            reading.marker = page.marker;
            return self.read_history(link);
        }
        self.reading = None;
        if self.stream == Stream::CatchingUp {
            self.stream = Stream::Following;
            self.escrow_checked_ms = None;
            self.send_submits(link);
        }
    }

    /// Once there is room for a page of hints, reads the history again
    /// from the first hint the stream brought without room, while it
    /// follows the stream.
    fn reread_overflow(&mut self, link: &mut Link) {
        let room = self.hints.len().saturating_add(PAGE as usize) <= MAX_HINTS;
        if !self.following() || self.reading.is_some() || !room {
            return;
        }
        let Some(from) = self.overflow_from.take() else {
            return;
        };
        self.reading = Some(Reading {
            from,
            to: self.streamed_to.max(from),
            marker: None,
            asking: false,
            through: from.saturating_sub(1),
        });
        self.read_history(link);
    }

    /// The server hints at `record`, in its stream or the escrow's
    /// history: its proof is asked for, unless it was attested already, or
    /// its ledger is one a walk passed. A hint the stream brings without
    /// room has the history read again from its ledger once there is
    /// room, so that it is proven with the others, many at a time, and the
    /// walk steps through their proofs.
    pub(super) fn hinted(&mut self, record: &TxRecord) {
        let hash = record.hash;
        let ledger = record.ledger_index;
        let known = self.known(&hash)
            || self.hints.contains_key(&hash)
            || self.proven.contains_key(&hash)
            || ledger <= self.walked_to;
        if known {
            return;
        }
        if self.hints.len() >= MAX_HINTS {
            self.overflow_from = Some(self.overflow_from.map_or(ledger, |from| from.min(ledger)));
            return;
        }
        self.hints.insert(
            hash,
            Hint {
                ledger: record.ledger_index,
                query: None,
                tries: 0,
                retry_at_ms: 0,
            },
        );
    }
}

/// What the verifier proves.
impl EscrowWatcher {
    /// Carries out what the verifier decided. Its verdicts on this
    /// escrow's queries and its anchors are taken at once, in the call that
    /// asked, as what queried relies on: a verdict that came at once closes
    /// its query before the caller notes it. The link carries out its asks,
    /// and parks a verdict on another's query, and a copy of each anchor,
    /// for the escrows it serves besides this one.
    pub(super) fn drain(&mut self, link: &mut Link) {
        for out in link.verifier_mut().take_out() {
            match out {
                verifier::Out::Verdict(id, verdict) if self.queries.contains_key(&id) => {
                    self.verdict(link, id, verdict);
                }
                verifier::Out::Anchored(anchor) => {
                    self.anchored(link, anchor, false);
                    link.park(Parked::Anchored(anchor, false));
                }
                verifier::Out::Reset(anchor) => {
                    self.anchored(link, anchor, true);
                    link.park(Parked::Anchored(anchor, true));
                }
                out => link.carry_out(out),
            }
        }
        link.tell_failed();
    }

    /// A ledger the UNL validated is the newest anchor; `reset` if the
    /// verifier proved it off the chain it kept. Where it stands against
    /// the newest ledger proven before: newer, and the cursor may pass up
    /// to it; lower and closed later, a reset of the test network, attested
    /// once, by the ledger it was proven at, unless the hub wrote one off
    /// since that ledger closed: a quorum saw it before this validator did,
    /// and a late vote only waits out the hub's quiet period.
    fn anchored(&mut self, link: &mut Link, anchor: Anchor, reset: bool) {
        link.anchor_proven();
        let close_time = u64::from(anchor.close_time);
        let judged = match judge_chain(self.newest(), link.network(), anchor.index, close_time) {
            Chain::Current if reset => Chain::Restarted,
            judged => judged,
        };
        match judged {
            Chain::Current => {
                self.anchored = Some((anchor, link.now_ms()));
                let cursor = self.cursor.map_or(
                    EscrowCursor {
                        newest_ledger: anchor.index,
                        newest_close_time: close_time,
                        scanned_to: 0,
                    },
                    |cursor| EscrowCursor {
                        newest_ledger: cursor.newest_ledger.max(anchor.index),
                        newest_close_time: cursor.newest_close_time.max(close_time),
                        ..cursor
                    },
                );
                self.cursor = Some(cursor);
                self.advance_cursor(link);
            }
            // Older than the newest proven before a restart: not current.
            Chain::Behind => {}
            Chain::WentBack => link.push(Out::Problem(
                "the XRP Ledger's validators validated a ledger off the chain followed, on a network that is never reset",
            )),
            Chain::Restarted => self.restarted(link, anchor),
        }
    }

    /// A reset of the test network, proven at `anchor`.
    fn restarted(&mut self, link: &mut Link, anchor: Anchor) {
        let close_time = u64::from(anchor.close_time);
        let closed_ms = close_time
            .saturating_add(XRPL_EPOCH_S)
            .saturating_mul(1_000);
        let written_off_at = self.view.as_ref().and_then(|view| view.written_off_at);
        if written_off_at.is_none_or(|at| at < closed_ms) {
            let reset = ValidatorOperation::AttestReset {
                ledger: anchor.index,
            };
            link.push(Out::Attest(reset, None));
        }
        link.push(Out::Problem(
            "the XRP Ledger testnet was reset: the escrow is checked again",
        ));
        let cursor = EscrowCursor {
            newest_ledger: anchor.index,
            newest_close_time: close_time,
            scanned_to: 0,
        };
        self.cursor = Some(cursor);
        self.ceiling = None;
        self.anchored = Some((anchor, link.now_ms()));
        self.escrow_checked_ms = None;
        self.escrow_checked_at = None;
        self.escrow_check_wanted = false;
        self.attested_escrow = None;
        self.issuer_checked_ms = None;
        self.attested_issuer = None;
        self.attested_issuer_before = None;
        self.proven_issuer = None;
        // What was proven of the old chain's history, and of its issuer's
        // hold, is no part of the new chain's.
        self.cancel_walk(link);
        self.cancel_search(link);
        self.named = None;
        self.unnamed = None;
        self.proven.clear();
        self.proven_order.clear();
        self.walked_to = 0;
        // The old chain's deposits wait no more: their ledgers would hold
        // the new chain's cursor. Its withdrawals were written off: what
        // the escrow's signers signed for it is not sent to the new one.
        self.deposits.clear();
        self.submits.clear();
        self.persist(link, cursor);
        // The new chain's anchor is kept at once: kept as of the chain
        // gone, a restart would meet the new one as another reset.
        link.keep(true, self.persisted);
        // What the server said of the old chain goes with it: the escrow's
        // history is read again on the new one.
        if link.is_connected() {
            self.forget_connection(link);
            link.check_again();
        }
    }

    /// The verdict `verdict` on the query `id`: nothing unless the query is
    /// this escrow's, and open.
    pub(super) fn verdict(&mut self, link: &mut Link, id: QueryId, verdict: Verdict) {
        let Some(purpose) = self.queries.remove(&id) else {
            return;
        };
        match (purpose, verdict) {
            (Purpose::Hint(hash), Verdict::Applied(proven) | Verdict::Before(proven)) => {
                self.hints.remove(&hash);
                let decoded = proven.decoded;
                if decoded.record.hash != hash {
                    return;
                }
                // Without room, it waits for the walk; with none for its
                // proof, the walk proves it again.
                let taken = self.record(link, &decoded.record);
                self.keep_ahead(decoded, taken);
            }
            (Purpose::Hint(hash), _) => self.not_found(link, hash),
            (Purpose::Escrow, Verdict::State(state)) => self.escrow_state(link, &state),
            (Purpose::Issuer, Verdict::State(state)) => self.hold_proven(link, &state),
            (Purpose::Walk, Verdict::State(state)) => self.walk_from(link, &state),
            (Purpose::Step(key), Verdict::Applied(proven) | Verdict::Before(proven)) => {
                if let Some(index) = self.stepped(key, id) {
                    let decoded = &proven.decoded;
                    let record = &decoded.record;
                    self.found(
                        link,
                        index,
                        record.hash,
                        record.ledger_index,
                        &decoded.threads,
                        Some(record),
                    );
                }
            }
            (Purpose::Step(key), _) => {
                if let Some(index) = self.stepped(key, id) {
                    self.step_not_found(link, index);
                }
            }
            (Purpose::HoldBefore, verdict) => self.hold_before(link, id, verdict),
            // Not proven: checked again at a newer anchor.
            (Purpose::Escrow, _) => self.escrow_check_wanted = true,
            (Purpose::Issuer | Purpose::Walk, _) => {}
        }
    }

    /// The server found no transaction it hinted at: it proved nothing, not
    /// even that there is none. Asked again later, and after a few tries on
    /// a new connection; the walk finds it meanwhile, if it is the escrow's.
    fn not_found(&mut self, link: &mut Link, hash: [u8; 32]) {
        let now = link.now_ms();
        let Some(hint) = self.hints.get_mut(&hash) else {
            return;
        };
        hint.query = None;
        hint.tries = hint.tries.saturating_add(1);
        hint.retry_at_ms = now.saturating_add(HINT_RETRY_MS);
        if hint.tries >= MAX_HINT_TRIES {
            link.push(Out::Problem(
                "the XRPL server hints at an escrow transaction it does not prove: connecting anew",
            ));
            link.push(Out::Reconnect);
        }
    }

    /// The escrow's setup, proven at an anchor: its next sequence attested
    /// once it passes the check, if it is as of the newest anchor and that
    /// anchor is fresh.
    fn escrow_state(&mut self, link: &mut Link, state: &ProvenState) {
        let checked = match self.view.as_ref() {
            Some(view) if !view.checked && self.current(state, link.now_ms()) => {
                check_proven_escrow(&view.rules, state)
            }
            Some(view) if !view.checked => {
                // As of an older anchor than the newest: judged at a newer.
                self.escrow_check_wanted = true;
                return;
            }
            _ => return,
        };
        match checked {
            Ok(sequence) if self.attested_escrow != Some(sequence) => {
                self.attested_escrow = Some(sequence);
                link.push(Out::Attest(
                    ValidatorOperation::AttestEscrow { sequence },
                    None,
                ));
            }
            Ok(_) => {}
            Err(_) => link.push(Out::Problem(
                "the hub's escrow is not set up as its rules say: no deposits are credited",
            )),
        }
    }

    /// The issuer's hold on a token hub's escrow, proven and named by the
    /// ledger it last changed in: attested if the newest anchor is fresh,
    /// and it differs from the hub's, or from what it attested since the
    /// hub's, as of a later ledger, and was not attested already. So the
    /// validator hears of each change it proved, a freeze again after a
    /// thaw among them, after the hub stopped too, when no block can show
    /// it (spec/areas/hubs.md#payout-bundles).
    fn judge_issuer(&mut self, link: &mut Link, proven: Issuer) {
        let hub = match self.view.as_ref() {
            Some(view) if self.fresh(link.now_ms()) => view.issuer,
            _ => return,
        };
        let hold = |issuer: Issuer| (issuer.frozen, issuer.fee);
        let differs = hold(proven) != hold(hub);
        let reverts = self
            .attested_issuer
            .is_some_and(|voted| voted.ledger > hub.ledger && hold(voted) != hold(proven));
        let forward = self
            .attested_issuer
            .is_none_or(|voted| voted != proven && voted.ledger <= proven.ledger);
        if (differs || reverts) && proven.ledger > hub.ledger && forward {
            self.attested_issuer_before = self.attested_issuer.replace(proven);
            link.push(Out::Attest(ValidatorOperation::AttestIssuer(proven), None));
        }
    }
}

/// What it asks, and what it attests.
impl EscrowWatcher {
    /// Asks what is due: the next page of history, the proofs of hints, the
    /// walk's next steps, and the escrow's and the issuer's state while the
    /// anchor is fresh.
    pub(super) fn work(&mut self, link: &mut Link) {
        if !link.is_connected() {
            return;
        }
        self.reread_overflow(link);
        self.read_history(link);
        self.prove_hints(link);
        self.start_walk(link);
        self.step_walk(link);
        self.search_hold(link);
        self.send_submits(link);
        let now = link.now_ms();
        if !self.following() || !self.fresh(now) {
            return self.advance_cursor(link);
        }
        let Some(view) = self.view.clone() else {
            return;
        };
        if !view.checked
            && !self.open(Purpose::Escrow)
            && (due(now, self.escrow_checked_ms, ESCROW_CHECK_EVERY_MS) || self.escrow_check_due())
        {
            self.escrow_checked_ms = Some(now);
            let asked = self.query(
                link,
                Purpose::Escrow,
                Query::State {
                    keys: escrow_keys(&view.rules.terms),
                    at: None,
                },
            );
            if asked.is_some() {
                self.escrow_checked_at = self.anchor().map(|anchor| anchor.index);
                self.escrow_check_wanted = false;
            }
        }
        if let Some(keys) = issuer_keys(&view.rules.terms)
            && !self.open(Purpose::Issuer)
            && due(now, self.issuer_checked_ms, ISSUER_CHECK_EVERY_MS)
        {
            self.issuer_checked_ms = Some(now);
            self.query(
                link,
                Purpose::Issuer,
                Query::State {
                    keys: keys.to_vec(),
                    at: None,
                },
            );
        }
        self.advance_cursor(link);
    }

    /// Whether a check of the escrow is wanted, and the anchor is newer
    /// than the last check's: one at the same anchor would judge the same
    /// state again.
    fn escrow_check_due(&self) -> bool {
        self.escrow_check_wanted
            && self.anchor().is_some_and(|anchor| {
                self.escrow_checked_at
                    .is_none_or(|checked| anchor.index > checked)
            })
    }

    /// Whether a query for `purpose` is open.
    fn open(&self, purpose: Purpose) -> bool {
        self.queries.values().any(|open| *open == purpose)
    }

    /// Asks the verifier of `link` `query`, for `purpose`, and carries out
    /// what it decided at once: its verdict among it, which closes the
    /// query before the caller sees its id.
    fn query(&mut self, link: &mut Link, purpose: Purpose, query: Query) -> Option<QueryId> {
        let now = link.now_ms();
        let id = link.verifier_mut().query(query, now).ok()?;
        self.queries.insert(id, purpose);
        self.drain(link);
        Some(id)
    }

    /// Asks for the proofs of hints, the oldest ledgers first, as many at
    /// once as [`MAX_HINT_QUERIES`].
    fn prove_hints(&mut self, link: &mut Link) {
        // Proven against an anchor taken back, a transaction of a chain
        // that reset while the validator was down would be applied.
        if link.taken_back() {
            return;
        }
        let open = self
            .hints
            .values()
            .filter(|hint| hint.query.is_some())
            .count();
        let room = MAX_HINT_QUERIES.saturating_sub(open);
        if room == 0 {
            return;
        }
        let now = link.now_ms();
        let mut due: Vec<(u32, [u8; 32])> = (self.hints.iter())
            .filter(|(_, hint)| hint.query.is_none() && hint.retry_at_ms <= now)
            .map(|(hash, hint)| (hint.ledger, *hash))
            .collect();
        due.sort_unstable();
        for (_, hash) in due.into_iter().take(room) {
            let query = Query::Transaction { hash, from: 0 };
            let Some(id) = self.query(link, Purpose::Hint(hash), query) else {
                return;
            };
            // Its verdict may have come at once, and closed it.
            if self.queries.contains_key(&id)
                && let Some(hint) = self.hints.get_mut(&hash)
            {
                hint.query = Some(id);
            }
        }
    }

    /// Whether the transaction of `hash` was attested already, or waits
    /// for the hub.
    fn known(&self, hash: &[u8; 32]) -> bool {
        self.remembered.contains(hash)
            || self.untaken.contains_key(hash)
            || self.deposits.contains_key(hash)
            || self.held.iter().any(|held| held.hash == *hash)
    }

    /// What a proven transaction of the escrow's attests: whether it was
    /// taken, false while what waits for the hub has no room for it.
    fn record(&mut self, link: &mut Link, record: &TxRecord) -> bool {
        let Some(view) = self.view.clone() else {
            return false;
        };
        if !record.validated || self.known(&record.hash) {
            return true;
        }
        // One of the escrow's own transactions applied while no quorum
        // checked it, as its sponsor's setup does: checked again as soon
        // as the anchor is newer than the last check's, unless that check
        // was at its ledger or later, and saw it.
        let applied = outcome_of(record, self.escrow).is_some_and(|(_, succeeded)| succeeded);
        let unseen = (self.escrow_checked_at).is_none_or(|checked| checked < record.ledger_index);
        if !view.checked && applied && unseen {
            self.escrow_check_wanted = true;
        }
        let terms = &view.rules.terms;
        if let Some(deposit) = deposit_of(record, terms) {
            if view.deposits_recorded.contains(&record.hash) {
                self.remember(record.hash);
                return true;
            }
            if self.deposits.len() >= MAX_WAITING_DEPOSITS {
                return false;
            }
            let waiting = Waiting {
                deposit,
                attested_ms: link.now_ms(),
            };
            self.deposits.insert(record.hash, waiting);
            self.attest(link, record, ValidatorOperation::AttestDeposit(deposit));
            return true;
        }
        if let Some(clawback) = clawback_of(record, terms) {
            self.attest(link, record, ValidatorOperation::AttestClawback(clawback));
            return true;
        }
        let Some((sequence, succeeded)) = outcome_of(record, self.escrow) else {
            // Nothing the hub reads: a payment it takes as gas, a line's
            // change, someone else's.
            self.remember(record.hash);
            return true;
        };
        let withdrawal = view.withdrawals.iter().find(|(s, _)| *s == sequence);
        if let Some((_, withdrawal)) = withdrawal {
            match withdrawal_outcome(record, terms, sequence, withdrawal) {
                Some(delivered) => self.attest(
                    link,
                    record,
                    ValidatorOperation::AttestWithdrawal {
                        escrow_sequence: sequence,
                        delivered,
                    },
                ),
                // Not the withdrawal's own Payment: it burns nothing.
                None => self.remember(record.hash),
            }
        } else if view.signer_list == Some(sequence) {
            if succeeded {
                self.attest(
                    link,
                    record,
                    ValidatorOperation::AttestSignerList { sequence },
                );
            } else {
                self.remember(record.hash);
            }
        } else if sequence >= view.next_sequence {
            // A sequence the hub has not given out, as far as this
            // validator executed: held until it has, if there is room.
            if self.held.len() >= MAX_HELD {
                return false;
            }
            self.held.push_back(record.clone());
        } else {
            // Settled already, or one of the escrow's setup: nothing to say.
            self.remember(record.hash);
        }
        true
    }

    fn attest(&mut self, link: &mut Link, record: &TxRecord, operation: ValidatorOperation) {
        self.untaken.insert(record.hash, record.ledger_index);
        link.push(Out::Attest(operation, Some(record.hash)));
    }

    /// What the validator's ledger says now: when it let go of the escrow,
    /// and which deposits' attestations it recorded, which wait no more.
    pub(super) fn see(&mut self, link: &mut Link, view: &EscrowView) {
        let was_held = self.view.as_ref().is_some_and(|seen| seen.issuer.holds());
        if was_held && !view.issuer.holds() {
            self.thawed_ms = Some(link.now_ms());
        }
        let mut recorded = false;
        for hash in &view.deposits_recorded {
            if self.deposits.remove(hash).is_some() {
                self.remember(*hash);
                recorded = true;
            }
        }
        let keys = walk_keys(&view.rules.terms);
        if keys != self.walk_keys {
            link.verifier_mut().thread(keys.clone());
            self.walk_keys = keys;
        }
        self.view = Some(view.clone());
        if recorded {
            self.advance_cursor(link);
        }
    }

    /// Whether the issuer holds a token hub's escrow, as of the hub's last
    /// final block, or as of a later state proven here: nothing is sent to
    /// it then (spec/decisions/token-hubs.md). Not knowing the hub's, it
    /// does.
    fn held(&self) -> bool {
        let Some(view) = self.view.as_ref() else {
            return true;
        };
        let proven = self
            .proven_issuer
            .is_some_and(|proven| proven.ledger > view.issuer.ledger && proven.holds());
        view.issuer.holds() || proven
    }

    /// Lets go of what was sent and needs no more: a withdrawal the hub
    /// settled, a sequence below the next it gives out and no longer open,
    /// and what does not read as a withdrawal once sent.
    fn prune_submits(&mut self) {
        let view = self.view.as_ref();
        self.submits.retain(|submit| match submit.sequence {
            Some(sequence) => view.is_none_or(|view| {
                sequence >= view.next_sequence
                    || view.withdrawals.iter().any(|(open, _)| *open == sequence)
            }),
            None => submit.sent_ms.is_none(),
        });
    }

    /// Sends, in sequence order, each escrow transaction not yet sent on
    /// this connection or sent [`RESUBMIT_MS`] ago, while it follows the
    /// escrow and the issuer does not hold it, and while its link's
    /// requests have room: one that finds none goes on a later turn.
    fn send_submits(&mut self, link: &mut Link) {
        if !self.following() || self.held() {
            return;
        }
        self.prune_submits();
        let now = link.now_ms();
        let mut due: Vec<(Option<u32>, usize)> = (self.submits.iter().enumerate())
            .filter(|(_, submit)| {
                submit
                    .sent_ms
                    .is_none_or(|at| now.saturating_sub(at) >= RESUBMIT_MS)
            })
            .map(|(index, submit)| (submit.sequence, index))
            .collect();
        due.sort_unstable();
        for (_, index) in due {
            if !link.room_to_submit() {
                break;
            }
            let Some(blob) = self.submits.get(index).map(|submit| submit.blob.clone()) else {
                continue;
            };
            if !link.ask(Asked::Submit, |id| api::submit(id, &blob)) {
                break;
            }
            if let Some(submit) = self.submits.get_mut(index) {
                submit.sent_ms = Some(now);
            }
        }
        self.prune_submits();
    }

    /// Attests again, oldest ledger first, each deposit the hub has not
    /// recorded the hub's margin after it was last attested or after the
    /// hub let go of the escrow, whichever is later, which is longer than
    /// an attestation takes to be final ([`dassie_hub::rules::Timers::margin`]):
    /// one a full share refused during a freeze, past what the service
    /// keeps, or one the host dropped. As many as the validator's share of
    /// the pending table has room for beside those on their way, and none
    /// while the issuer holds the escrow, when the share would only fill
    /// again.
    fn retry_deposits(&mut self, link: &mut Link) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        if self.deposits.is_empty() || self.held() {
            return;
        }
        let wait = view.rules.terms.timers.margin;
        let since = self.thawed_ms.unwrap_or(0);
        let now = link.now_ms();
        let on_the_way = (self.deposits.keys())
            .filter(|hash| self.untaken.contains_key(*hash))
            .count();
        let room = view.deposit_room.saturating_sub(on_the_way);
        let mut due: Vec<(u32, [u8; 32])> = (self.deposits.iter())
            .filter(|(hash, waiting)| {
                !self.untaken.contains_key(*hash)
                    && now.saturating_sub(waiting.attested_ms.max(since)) >= wait
            })
            .map(|(hash, waiting)| (waiting.deposit.ledger, *hash))
            .collect();
        due.sort_unstable();
        for (ledger, hash) in due.into_iter().take(room) {
            let Some(waiting) = self.deposits.get_mut(&hash) else {
                continue;
            };
            waiting.attested_ms = now;
            let deposit = waiting.deposit;
            self.untaken.insert(hash, ledger);
            link.push(Out::Attest(
                ValidatorOperation::AttestDeposit(deposit),
                Some(hash),
            ));
        }
    }

    fn remember(&mut self, hash: [u8; 32]) {
        if !self.remembered.insert(hash) {
            return;
        }
        self.remembered_order.push_back(hash);
        if self.remembered_order.len() > MAX_REMEMBERED
            && let Some(oldest) = self.remembered_order.pop_front()
        {
            self.remembered.remove(&oldest);
        }
    }

    /// Moves the cursor as far as it may: up to where the last walk of the
    /// escrow's history reached, no further than the newest ledger proven,
    /// and below every attestation not yet taken, every deposit the hub has
    /// not recorded and every outcome held.
    fn advance_cursor(&mut self, link: &mut Link) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let mut to = self.walked_to.min(cursor.newest_ledger);
        let blocking = (self.untaken.values().copied())
            .chain(self.deposits.values().map(|waiting| waiting.deposit.ledger))
            .chain(self.held.iter().map(|record| record.ledger_index));
        for ledger in blocking {
            to = to.min(ledger.saturating_sub(1));
        }
        let cursor = EscrowCursor {
            scanned_to: cursor.scanned_to.max(to),
            ..cursor
        };
        self.cursor = Some(cursor);
        self.persist(link, cursor);
    }

    /// Persists `cursor` if it changed, never naming as attested a ledger
    /// past one whose attestation was dropped.
    fn persist(&mut self, link: &mut Link, cursor: EscrowCursor) {
        let scanned_to = self
            .ceiling
            .map_or(cursor.scanned_to, |ceiling| cursor.scanned_to.min(ceiling));
        let cursor = EscrowCursor {
            scanned_to,
            ..cursor
        };
        if self.persisted != Some(cursor) {
            self.persisted = Some(cursor);
            link.push(Out::Cursor(cursor));
        }
    }
}

/// Whether what was done at `at` is due again at `now_ms`, `every` later.
fn due(now_ms: u64, at: Option<u64>, every: u64) -> bool {
    at.is_none_or(|at| now_ms.saturating_sub(at) >= every)
}

/// The keys of the escrow's objects whose history is walked: its account
/// root, and a token hub's line to the issuer, which a token's deposits
/// and clawbacks change without the root.
fn walk_keys(terms: &dassie_hub::rules::Terms) -> Vec<[u8; 32]> {
    let mut keys = vec![records::account_key(AccountId(terms.escrow))];
    if let Some([line, _]) = issuer_keys(terms) {
        keys.push(line);
    }
    keys
}

/// The transaction that last changed the object `bytes` of `key`, as the
/// object names it; `Err` if it does not read as what `key` holds.
fn thread_of(escrow: AccountId, key: &[u8; 32], bytes: &[u8]) -> Result<Option<Thread>, ()> {
    if *key == records::account_key(escrow) {
        let root = records::account_root(bytes).map_err(|_| ())?;
        if root.account != escrow {
            return Err(());
        }
        Ok(root.thread)
    } else {
        let (_, thread) = records::ripple_state(bytes).map_err(|_| ())?;
        Ok(thread)
    }
}

/// Walking the escrow's history (spec/areas/xrpl.md#following-the-account).
impl EscrowWatcher {
    /// Starts a walk once the last one ended, by proving the escrow's
    /// objects at [`Self::walk_top`].
    fn start_walk(&mut self, link: &mut Link) {
        if !self.subscribed()
            || self.walk.is_some()
            || self.walk_keys.is_empty()
            || self.open(Purpose::Walk)
        {
            return;
        }
        let (Some(anchor), Some(top)) = (self.anchor(), self.walk_top()) else {
            return;
        };
        let at = (top < anchor.index).then_some(top);
        let keys = self.walk_keys.clone();
        self.query(link, Purpose::Walk, Query::State { keys, at });
    }

    /// Where the next walk starts, if past the last one: the newest
    /// anchor, but below what the server hinted at and the watcher has not
    /// proven yet, below the first hint the stream brought without room,
    /// and no further than the history read so far, so that the walk steps
    /// through the proofs of the hints rather than proving each in turn,
    /// newest first, while the hints are proven oldest first, many at a
    /// time. A hint the server did not prove holds no walk back, nor does
    /// anything the server hints at above the anchor.
    fn walk_top(&self) -> Option<u32> {
        let mut top = self.anchor()?.index;
        let unproven = (self.hints.values())
            .filter(|hint| hint.tries == 0)
            .map(|hint| hint.ledger);
        let unread = (self
            .reading
            .iter()
            .map(|reading| reading.through.saturating_add(1)))
        .chain(self.overflow_from);
        for ledger in unproven.chain(unread) {
            top = top.min(ledger.saturating_sub(1));
        }
        (top > self.walked_to).then_some(top)
    }

    /// The escrow's objects proven at a ledger past the last walk: a walk
    /// from there, along the chain each names, back to where the last one
    /// ended. One proof that does not read as the escrow's objects starts
    /// none: the next anchor's may.
    fn walk_from(&mut self, link: &mut Link, state: &ProvenState) {
        let top = state.header.sequence;
        if self.walk.is_some() || top <= self.walked_to {
            return;
        }
        let floor = self.walked_to;
        let mut chains = Vec::new();
        for key in &self.walk_keys {
            let next = match state.object(key) {
                Some(Some(bytes)) => match thread_of(self.escrow, key, bytes) {
                    Ok(thread) => thread,
                    Err(()) => {
                        return link.push(Out::Problem(
                            "the escrow's objects did not read: its history is walked from the next anchor",
                        ));
                    }
                },
                Some(None) => None,
                None => return,
            };
            chains.push(ObjectChain {
                key: *key,
                next: next.filter(|(_, ledger)| *ledger > floor && *ledger <= top),
                query: None,
                tries: 0,
                retry_at_ms: 0,
                waiting: None,
            });
        }
        self.walk = Some(Walk { top, floor, chains });
        self.step_walk(link);
    }

    /// The walk's chain `index`.
    fn chain(&self, index: usize) -> Option<&ObjectChain> {
        self.walk.as_ref()?.chains.get(index)
    }

    fn chain_mut(&mut self, index: usize) -> Option<&mut ObjectChain> {
        self.walk.as_mut()?.chains.get_mut(index)
    }

    /// The chain whose step `id` answered for the object of `key`, its query
    /// now closed.
    fn stepped(&mut self, key: [u8; 32], id: QueryId) -> Option<usize> {
        let walk = self.walk.as_mut()?;
        let index = walk.chains.iter().position(|chain| chain.key == key)?;
        let chain = walk.chains.get_mut(index)?;
        // A verdict that came at once, before its query was noted, is the
        // chain's too.
        if chain.query.is_some_and(|open| open != id) {
            return None;
        }
        chain.query = None;
        Some(index)
    }

    /// Steps each chain as far as it goes now, then ends the walk once each
    /// chain did.
    fn step_walk(&mut self, link: &mut Link) {
        let chains = self.walk.as_ref().map_or(0, |walk| walk.chains.len());
        for index in 0..chains {
            for _ in 0..MAX_CHAIN_STEPS {
                if !self.step_chain(link, index) {
                    break;
                }
            }
        }
        self.end_walk(link);
    }

    /// One step along chain `index`: a transaction found that now finds
    /// room, or the next one, from its proof kept from a hint, or asked
    /// for. Whether it moved, and may move again.
    fn step_chain(&mut self, link: &mut Link, index: usize) -> bool {
        let Some(chain) = self.chain(index) else {
            return false;
        };
        if let Some((record, before)) = chain.waiting.clone() {
            if !self.record(link, &record) {
                return false;
            }
            self.chain_to(index, before);
            return true;
        }
        let (key, query, retry_at_ms) = (chain.key, chain.query, chain.retry_at_ms);
        let Some((hash, ledger)) = chain.next else {
            return false;
        };
        if query.is_some() {
            return false;
        }
        // Proven before its object was walked, it may not name what changed
        // it before: then it is proven again.
        if let Some(ahead) = self.proven.get(&hash)
            && ahead.threads.iter().any(|(of, _)| *of == key)
        {
            let ahead = ahead.clone();
            let moved = self.found(
                link,
                index,
                hash,
                ahead.ledger,
                &ahead.threads,
                ahead.record.as_deref(),
            );
            // Handed on now, it needs its record no more: the other
            // object's chain only steps past it.
            if moved
                && ahead.record.is_some()
                && self
                    .chain(index)
                    .is_none_or(|chain| chain.waiting.is_none())
                && let Some(kept) = self.proven.get_mut(&hash)
            {
                kept.record = None;
            }
            return moved;
        }
        // Its hint's proof is on its way: the walk waits for it rather than
        // proving it twice.
        if self
            .hints
            .get(&hash)
            .is_some_and(|hint| hint.query.is_some())
        {
            return false;
        }
        if retry_at_ms > link.now_ms() {
            return false;
        }
        let Some(id) = self.query(
            link,
            Purpose::Step(key),
            Query::Transaction { hash, from: ledger },
        ) else {
            return false;
        };
        if !self.queries.contains_key(&id) {
            // Its verdict came at once.
            return true;
        }
        if let Some(chain) = self.chain_mut(index) {
            chain.query = Some(id);
        }
        false
    }

    /// The next transaction on chain `index`, proven, of `hash` in
    /// `ledger`, with the transactions before it on each object it
    /// changed: it is the one the chain names, in the ledger it names, and
    /// changed the chain's object, or the server's proofs do not agree with
    /// the state it proved, and the walk starts again on a new connection.
    /// Its `record` is handed on to be attested, or waits for room; none,
    /// it was handed on when it was proven. The chain goes on to the
    /// transaction that changed the object before.
    fn found(
        &mut self,
        link: &mut Link,
        index: usize,
        hash: [u8; 32],
        ledger: u32,
        threads: &[([u8; 32], Option<Thread>)],
        record: Option<&TxRecord>,
    ) -> bool {
        let Some(chain) = self.chain(index) else {
            return false;
        };
        let (key, next) = (chain.key, chain.next);
        let before = (threads.iter())
            .find(|(of, _)| *of == key)
            .map(|(_, before)| *before);
        let agrees = next == Some((hash, ledger))
            && before.is_some_and(|before| before.is_none_or(|(_, earlier)| earlier <= ledger));
        let (true, Some(before)) = (agrees, before) else {
            self.cancel_walk(link);
            link.push(Out::Problem(
                "the escrow's history did not prove as its state names it: connecting anew",
            ));
            link.push(Out::Reconnect);
            return false;
        };
        match record {
            Some(record) if !self.record(link, record) => {
                if let Some(chain) = self.chain_mut(index) {
                    chain.waiting = Some((Box::new(record.clone()), before));
                }
            }
            _ => self.chain_to(index, before),
        }
        true
    }

    /// Chain `index` goes on at `before`, or ends there, at the walk's
    /// floor.
    fn chain_to(&mut self, index: usize, before: Option<Thread>) {
        let Some(walk) = self.walk.as_mut() else {
            return;
        };
        let floor = walk.floor;
        if let Some(chain) = walk.chains.get_mut(index) {
            chain.next = before.filter(|(_, ledger)| *ledger > floor);
            chain.waiting = None;
            chain.tries = 0;
            chain.retry_at_ms = 0;
        }
    }

    /// The server found no transaction chain `index` names: it proved
    /// nothing. Asked again later, and after a few tries on a new
    /// connection; one further back than the history a server keeps
    /// ([`HISTORY_BOUND`]), as its proven ledger says, ends the chain:
    /// history no server has any more.
    fn step_not_found(&mut self, link: &mut Link, index: usize) {
        let anchor = self.anchor().map_or(0, |anchor| anchor.index);
        let now = link.now_ms();
        let Some(chain) = self.chain_mut(index) else {
            return;
        };
        let Some((_, ledger)) = chain.next else {
            return;
        };
        if anchor.saturating_sub(ledger) > HISTORY_BOUND {
            link.push(Out::Problem(
                "the escrow's history is older than XRPL servers keep: what is older is not read",
            ));
            return self.chain_to(index, None);
        }
        chain.tries = chain.tries.saturating_add(1);
        chain.retry_at_ms = now.saturating_add(HINT_RETRY_MS);
        if chain.tries >= MAX_HINT_TRIES {
            chain.tries = 0;
            link.push(Out::Problem(
                "the XRPL server does not prove the escrow's history its state names: connecting anew",
            ));
            link.push(Out::Reconnect);
        }
    }

    /// Ends the walk once each chain did: every transaction that changed
    /// the escrow's objects up to its top was found, and taken, and the
    /// cursor may pass it.
    fn end_walk(&mut self, link: &mut Link) {
        let ended = self.walk.as_ref().is_some_and(|walk| {
            (walk.chains.iter()).all(|chain| chain.next.is_none() && chain.waiting.is_none())
        });
        if !ended {
            return;
        }
        let Some(walk) = self.walk.take() else {
            return;
        };
        self.walked_to = self.walked_to.max(walk.top);
        let passed = (self.proven_order.iter())
            .take_while(|(ledger, _)| *ledger <= self.walked_to)
            .copied()
            .collect::<Vec<_>>();
        for (_, hash) in passed {
            self.forget_ahead(&hash);
        }
        self.advance_cursor(link);
    }

    /// Keeps the proof of a transaction proven from a hint for a walk to
    /// step through, if a walk will reach it: with its record if it was
    /// not `taken`, while there is room for one. Without room, the newest
    /// makes room for an older one, which a walk reaches sooner, since
    /// each starts above the last.
    fn keep_ahead(&mut self, decoded: Decoded, taken: bool) {
        let Decoded {
            record, threads, ..
        } = decoded;
        let (hash, ledger) = (record.hash, record.ledger_index);
        let records = (self.proven.values())
            .filter(|ahead| ahead.record.is_some())
            .count();
        let useless = ledger <= self.walked_to
            || threads.is_empty()
            || self.proven.contains_key(&hash)
            || (!taken && records >= MAX_PROVEN_RECORDS);
        if useless {
            return;
        }
        if self.proven.len() >= MAX_PROVEN {
            match self.proven_order.last().copied() {
                Some((newest, evicted)) if newest > ledger => self.forget_ahead(&evicted),
                _ => return,
            }
        }
        self.proven_order.insert((ledger, hash));
        self.proven.insert(
            hash,
            Ahead {
                ledger,
                threads,
                record: (!taken).then(|| Box::new(record)),
            },
        );
    }

    fn forget_ahead(&mut self, hash: &[u8; 32]) {
        if let Some(ahead) = self.proven.remove(hash) {
            self.proven_order.remove(&(ahead.ledger, *hash));
        }
    }

    /// Gives up the walk under way: the next starts again from where the
    /// last one ended.
    fn cancel_walk(&mut self, link: &mut Link) {
        let Some(walk) = self.walk.take() else {
            return;
        };
        for id in walk.chains.iter().filter_map(|chain| chain.query) {
            self.queries.remove(&id);
            link.verifier_mut().cancel(id);
        }
    }
}

/// Naming the issuer's hold by the ledger it last changed in.
impl EscrowWatcher {
    /// The issuer's hold, proven at the newest anchor while it is fresh:
    /// kept as the newest proven, as of that ledger, which is what the
    /// watcher submits by. Where it differs from the hub's, or from what
    /// this validator attested since the hub's, a search for the ledger it
    /// last changed in starts: anything else attests nothing, and needs no
    /// name.
    fn hold_proven(&mut self, link: &mut Link, state: &ProvenState) {
        if !self.current(state, link.now_ms()) {
            return;
        }
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let Some(hold) = hold_of(&view.rules.terms, state) else {
            return;
        };
        let hub = view.issuer;
        let top = state.header.sequence;
        if self.proven_issuer.is_none_or(|newest| newest.ledger <= top) {
            self.proven_issuer = Some(hold.named(top));
        }
        let differs = !hold.same(&hub);
        let reverts = self
            .attested_issuer
            .is_some_and(|voted| voted.ledger > hub.ledger && !hold.same(&voted));
        if self.search.is_some() || !(differs || reverts) {
            return;
        }
        self.search = Some(HoldSearch {
            hold,
            top,
            since: hold.since.min(top),
            query: None,
        });
        self.search_hold(link);
    }

    /// Goes on with the search: it ends once the hold is known the same
    /// over a ledger it was named at, by this watcher or by a quorum of the
    /// hub's validators; else the ledger before the one it is proven the
    /// same since is proven next.
    fn search_hold(&mut self, link: &mut Link) {
        let Some(search) = self.search else {
            return;
        };
        if search.query.is_some() {
            return;
        }
        if let Some(ledger) = self.known_name(&search) {
            return self.hold_named(link, search, ledger);
        }
        // It reached a stretch its server could not search past: it names
        // nothing, as the last search did, and proves nothing again.
        if let Some((hold, from, to)) = self.unnamed
            && hold.frozen == search.hold.frozen
            && hold.fee == search.hold.fee
            && (from..=to.saturating_add(1)).contains(&search.since)
        {
            self.unnamed = Some((hold, from, to.max(search.top)));
            self.search = None;
            return;
        }
        let Some(view) = self.view.as_ref() else {
            return;
        };
        // A name no later than the hub's state, or this validator's own
        // vote, would attest nothing: the search goes no further back.
        let floor = (self.attested_issuer.map_or(0, |voted| voted.ledger)).max(view.issuer.ledger);
        let keys = issuer_keys(&view.rules.terms);
        let (Some(keys), true) = (keys, search.since > floor) else {
            self.search = None;
            return;
        };
        let before = search.since.saturating_sub(1);
        let query = Query::State {
            keys: keys.to_vec(),
            at: Some(before),
        };
        let Some(id) = self.query(link, Purpose::HoldBefore, query) else {
            return;
        };
        if self.queries.contains_key(&id)
            && let Some(search) = self.search.as_mut()
        {
            search.query = Some(id);
        }
    }

    /// The ledger the searched hold is named by, if known: one it was named
    /// at before, by this watcher's last search or by the hub, within the
    /// stretch this search proved it the same over, or just before it, so
    /// that the two stretches are one.
    fn known_name(&self, search: &HoldSearch) -> Option<u32> {
        let joins = |named: &Issuer, to: u32| {
            search.hold.same(named)
                && named.ledger > 0
                && (named.ledger..=to.saturating_add(1)).contains(&search.since)
                && named.ledger <= search.top
        };
        if let Some((named, to)) = self.named
            && joins(&named, to)
        {
            return Some(named.ledger);
        }
        let hub = self.view.as_ref()?.issuer;
        joins(&hub, hub.ledger).then_some(hub.ledger)
    }

    /// The issuer's hold as of the ledger before the one the search proved
    /// it the same since: the same there, and the search goes on from what
    /// that ledger proves; or not, or not there at all, and the hold
    /// changed in the ledger after. A server that keeps no such ledger ends
    /// the search, naming nothing; and the next searches, on this
    /// connection, end where they reach the stretch it searched.
    fn hold_before(&mut self, link: &mut Link, id: QueryId, verdict: Verdict) {
        // A verdict that came at once, before its query was noted, is the
        // search's too.
        let Some(mut search) =
            (self.search).filter(|search| search.query.is_none_or(|open| open == id))
        else {
            return;
        };
        search.query = None;
        let terms = self.view.as_ref().map(|view| view.rules.terms.clone());
        match (verdict, terms) {
            (Verdict::State(state), Some(terms)) => {
                let before = state.header.sequence;
                match hold_of(&terms, &state) {
                    Some(earlier)
                        if earlier.frozen == search.hold.frozen
                            && earlier.fee == search.hold.fee =>
                    {
                        if earlier.since > before || before.saturating_add(1) != search.since {
                            self.search = None;
                            return;
                        }
                        search.since = earlier.since;
                        self.search = Some(search);
                        self.search_hold(link);
                    }
                    _ => self.hold_named(link, search, search.since),
                }
            }
            (Verdict::NoLedger, _) => self.hold_named(link, search, search.since),
            (Verdict::NoHistory, _) => {
                self.search = None;
                self.unnamed = Some((search.hold, search.since, search.top));
                link.push(Out::Problem(
                    "the XRPL server keeps too short a history to name the issuer's hold by the ledger it changed in",
                ));
            }
            _ => self.search = None,
        }
    }

    /// The search found the ledger the hold last changed in: it is judged.
    fn hold_named(&mut self, link: &mut Link, search: HoldSearch, ledger: u32) {
        self.search = None;
        let proven = search.hold.named(ledger);
        let to = match self.named {
            Some((named, to)) if named == proven => to.max(search.top),
            _ => search.top,
        };
        self.named = Some((proven, to));
        self.judge_issuer(link, proven);
    }

    /// Gives up the search under way.
    fn cancel_search(&mut self, link: &mut Link) {
        if let Some(id) = self.search.take().and_then(|search| search.query) {
            self.queries.remove(&id);
            link.verifier_mut().cancel(id);
        }
    }
}
