//! Following a hub's escrow on the XRP Ledger, as a validator does
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals), without
//! IO: the requests to send on its own WebSocket to its XRPL server, the
//! validator lists to fetch, and what it attests from what it proved.
//!
//! It takes nothing on the server's word. A [`Verifier`] of its own,
//! trusting the hub's network's UNL, proves everything it attests against
//! ledgers that UNL validated (spec/designs/xrpl-validation.md#12-who-else-uses-it):
//!
//! - **Transactions by their proofs.** The server's stream and the
//!   escrow's history (`account_tx`) only hint at hashes. Each hint is
//!   proven by its transaction's own bytes and metadata in a validated
//!   ledger, and read as `dassie_hub::escrow` reads it: a deposit, the
//!   outcome of a pending withdrawal, the old validators' change of the
//!   signer list, and for a token hub the issuer's clawback. A hint that
//!   proves nothing is attested never. The cursor passes a ledger only
//!   once every hint in it was proven and taken, and no further than the
//!   newest proven ledger: hints are bounded by paging through the
//!   history ([`MAX_HINTS`]), never by dropping one, so no number of small
//!   payments to the escrow makes it skip a deposit.
//! - **State by state proofs, at a fresh anchor.** The escrow's setup is
//!   checked, while no quorum has, from proofs of its account root, its
//!   signer list and a token hub's line at one validated ledger; a token
//!   hub's issuer's hold (a freeze of the escrow's line, a global freeze, a
//!   transfer fee) from proofs of the line and the issuer's account root,
//!   and attested only where it differs from the hub's. What is judged as
//!   of the newest ledger is judged only from a proof as of the newest
//!   anchor, while it is fresh ([`ANCHOR_FRESH_MS`]): an old anchor delays
//!   attestations, never invents them.
//! - **Resets by the verifier.** A reset of the test network is attested
//!   only once a ledger of the new chain is proven under the UNL
//!   (spec/decisions/ledger-resets.md), never from what the server says
//!   of its validated ledger, which only tells the watcher to wait.
//!
//! - **Connecting:** check the network, and where the server's validated
//!   ledger stands against the newest proven ([`chain`]); subscribe to the
//!   escrow; catch up with the escrow's history from the last ledger
//!   attested, a window back ([`RESCAN_LEDGERS`]), since an attestation
//!   lost in a crash must go again and the hub refuses one sent twice; then
//!   follow the stream. A server that lies, refuses or stops answering is
//!   left for a new connection ([`Out::Reconnect`]), and what it hinted is
//!   forgotten: the next connection reads the history again.
//! - **What it submits:** the escrow's transactions a quorum signed, once
//!   it follows the escrow, and only while the issuer holds a token hub's
//!   escrow neither as of the hub's last final block nor as of a later
//!   state it proved, judged when it sends, not when it queues: one queued
//!   before a freeze waits for the thaw. A withdrawal's transaction is kept
//!   while the withdrawal stays open, and sent again every
//!   [`RESUBMIT_MS`], so one lost on the way, or never asked for while its
//!   requests were full, still goes.
//! - **Deposits until the hub recorded them.** A deposit's attestation
//!   holds the cursor until the hub's ledger recorded it
//!   ([`EscrowView::deposits_recorded`]), and is attested again while it
//!   has not, once the issuer lets go and the validator's share of the
//!   pending table has room: so one a full share refused during a freeze is
//!   lost neither to a restart, which reads it again, nor past the bound on
//!   what the hub's service keeps to attest again.
//!
//! The verifier's state is not kept across restarts: a restarted validator
//! fetches its lists and anchors anew, and its cursor's newest ledger still
//! tells a server behind, or a chain gone back.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_hub::Rules;
use dassie_hub::escrow::{
    check_proven_escrow, clawback_of, deposit_of, escrow_keys, issuer_keys, issuer_of, outcome_of,
    withdrawal_outcome,
};
use dassie_hub::ledger::Withdrawal;
use dassie_hub::operation::{Deposit, Issuer, ValidatorOperation};
use dassie_hub::service::MAX_WAITING_DEPOSITS;
use dassie_xrpl::AccountId;
use dassie_xrpl::account::resets;
use dassie_xrpl::anchor::ProvenState;
use dassie_xrpl::api::{self, ApiError, Message, PAGE, TxRecord};
use dassie_xrpl::codec::{Payment, Transact};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{
    self, ANCHOR_EVERY_MS, Ask, Query, QueryId, REQUEST_TIMEOUT_MS, STALL_MS, Verdict, Verifier,
};
use serde_json::Value;

use crate::store::EscrowCursor;

/// How far back from the last ledger attested a catch-up starts: what was
/// attested just before a crash may never have reached a block.
pub const RESCAN_LEDGERS: u32 = 1024;

/// Seconds from the Unix epoch to the XRP Ledger's, in which close times
/// are counted.
const XRPL_EPOCH_S: u64 = 946_684_800;

/// How often the escrow is checked while no quorum has.
pub const ESCROW_CHECK_EVERY_MS: u64 = 30_000;

/// How often a token hub's issuer's hold on the escrow is proven: every
/// few ledgers.
pub const ISSUER_CHECK_EVERY_MS: u64 = 2 * ANCHOR_EVERY_MS;

/// How often a server behind, or on another network, is asked again where
/// it stands.
pub const RETRY_MS: u64 = 10_000;

/// How long after the newest anchor was proven the watcher still judges
/// what holds as of the newest ledger: the escrow's setup, the issuer's
/// hold. An honest server's network closes a ledger about every
/// [`ANCHOR_EVERY_MS`], and a round that fails takes at most
/// [`REQUEST_TIMEOUT_MS`] to give up on, or a stall of [`STALL_MS`]
/// before the next connection: so a validator on an honest server proves
/// a newer anchor within that, two closes allowed for the round itself.
/// Past it, its server stopped closing ledgers, or keeps the newer ones
/// from it, and nothing is judged until a newer anchor is proven.
pub const ANCHOR_FRESH_MS: u64 = REQUEST_TIMEOUT_MS + STALL_MS + 2 * ANCHOR_EVERY_MS;

/// The most signed escrow transactions kept: waiting for a connection, for
/// the issuer to let go of the escrow, or for their withdrawal to settle.
pub const MAX_QUEUED_SUBMITS: usize = 64;

/// How long after it sent an escrow transaction the watcher sends it again
/// while its withdrawal stays open. A transaction the server took is in a
/// validated ledger within two closes, the open ledger's and the next, so
/// one whose withdrawal is still open then may have been lost on the way:
/// dropped by the server or the network, or never asked for while the
/// requests were full. One that applied meanwhile is refused for its
/// sequence, at no cost.
pub const RESUBMIT_MS: u64 = 2 * ANCHOR_EVERY_MS;

/// The most transactions hinted at and not yet proven: two pages of the
/// escrow's history. A page is asked for only while there is room for a
/// whole one, and a hint the stream brings that finds no room has the
/// history read again from its ledger once there is.
pub const MAX_HINTS: usize = 2 * PAGE as usize;

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

/// The most requests of its own waiting for an answer.
const MAX_PENDING: usize = 16;

/// The most of them that submit an escrow transaction: the rest are kept
/// for following the escrow.
const MAX_PENDING_SUBMITS: usize = MAX_PENDING / 2;

/// The most proof requests the verifier has waiting on a connection: the
/// verifier's own bound, with room for those of a round it gave up on.
const MAX_PROOF_ASKS: usize = 4 * verifier::MAX_QUERIES;

/// What the validator's ledger says that bears on following the escrow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowView {
    /// The rules in force, for the check of the escrow.
    pub rules: Rules,
    /// Whether a quorum checked the escrow.
    pub checked: bool,
    /// The escrow's next sequence the hub gives out.
    pub next_sequence: u32,
    /// The withdrawals with an escrow sequence, not yet settled.
    pub withdrawals: Vec<(u32, Withdrawal)>,
    /// The sequence kept for the old validators' change of signer list.
    pub signer_list: Option<u32>,
    /// When the hub last wrote off a reset, by its block's timestamp.
    pub written_off_at: Option<u64>,
    /// What a quorum last attested of a token hub's issuer's hold.
    pub issuer: Issuer,
    /// How many more deposits this validator can attest now: its share of
    /// the pending table's room, none once the hub ended or while it is in
    /// no set in force.
    pub deposit_room: usize,
    /// The newest deposits, by transaction, whose attestation by this
    /// validator the hub's ledger recorded, or takes no more: credited,
    /// pending with it, or below the hub's floor. A bounded list: one that
    /// left it before the watcher saw it is attested again, and is in it
    /// again.
    pub deposits_recorded: Vec<[u8; 32]>,
}

/// Where a server's validated ledger stands against the newest seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    /// At or past it.
    Current,
    /// Below it, closed before it: a server behind.
    Behind,
    /// Below it, closed after it: a new chain, which a server behind cannot
    /// report. A reset, on the testnet.
    Restarted,
    /// The same, on a network that never resets.
    WentBack,
}

/// Where ledger `index`, closed at `close_time`, stands against `newest`
/// on `network` (spec/decisions/ledger-resets.md).
#[must_use]
pub fn chain(newest: Option<(u32, u64)>, network: u32, index: u32, close_time: u64) -> Chain {
    match newest {
        Some((newest, newest_close)) if index < newest => {
            if close_time <= newest_close {
                Chain::Behind
            } else if resets(network) {
                Chain::Restarted
            } else {
                Chain::WentBack
            }
        }
        _ => Chain::Current,
    }
}

/// What the watcher asks its host to do. Taken apart at once: boxing would
/// only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Send this on the WebSocket.
    Send(Vec<u8>),
    /// Fetch this validator list publisher's site by an HTTPS GET that
    /// follows no redirect and reads at most
    /// [`MAX_LIST_BYTES`](dassie_xrpl::validator_list::MAX_LIST_BYTES), and
    /// hand the body to [`Watcher::fetched`].
    Fetch(String),
    /// Drop the connection and connect anew: its server lied, refused or
    /// stopped answering, or the network reset.
    Reconnect,
    /// Have the validator attest this, and tell [`Watcher::taken`] once
    /// it took it, with the transaction it is about, if any: until then
    /// the watcher neither remembers the transaction nor moves its cursor
    /// past it.
    Attest(ValidatorOperation, Option<[u8; 32]>),
    /// Persist how far it followed the escrow.
    Cursor(EscrowCursor),
    /// Something the owner should read in the log.
    Problem(&'static str),
}

/// What a request of its own was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    ServerState,
    Subscribe,
    History,
    Submit,
}

/// What one of the verifier's queries is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// The proof of a transaction hinted at.
    Hint([u8; 32]),
    /// The escrow's setup, at the anchor.
    Escrow,
    /// The issuer's hold on a token hub's escrow, at the anchor.
    Issuer,
}

/// A transaction the server hinted at, not yet proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hint {
    /// The ledger the server named for it: the cursor stays below it.
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
struct Submit {
    /// The escrow's sequence it uses, if it reads as a Payment: a
    /// withdrawal's, kept and sent again while the withdrawal is open. One
    /// that does not read is sent once.
    sequence: Option<u32>,
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

/// Where the connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Disconnected,
    /// Asked where the server stands.
    Checking,
    /// Its server is behind, on another network, or failed to answer:
    /// asked again later.
    Waiting {
        since_ms: u64,
    },
    /// Subscribed, and reading the history from `from` to `to`, at
    /// `marker`; `asking` while a page is asked for.
    CatchingUp {
        from: u32,
        to: u32,
        marker: Option<Value>,
        asking: bool,
    },
    Following,
}

/// Follows one hub's escrow.
#[derive(Debug)]
pub struct Watcher {
    escrow: AccountId,
    network: u32,
    cursor: Option<EscrowCursor>,
    phase: Phase,
    next_id: u64,
    pending: BTreeMap<u64, Asked>,
    /// The verifier's asks on this connection, by their request's id.
    proof_asks: BTreeMap<u64, Ask>,
    verifier: Verifier,
    /// What each of the verifier's open queries is for.
    queries: BTreeMap<QueryId, Purpose>,
    hints: BTreeMap<[u8; 32], Hint>,
    /// The lowest ledger of a hint the stream brought without room for it:
    /// the history is read again from there.
    overflow_from: Option<u32>,
    /// The newest ledger the history and the stream covered, as the server
    /// says: the cursor passes no ledger beyond it, nor beyond the newest
    /// proven one.
    read_to: u32,
    /// The newest ledger the stream said closed.
    streamed_to: u32,
    remembered: BTreeSet<[u8; 32]>,
    remembered_order: VecDeque<[u8; 32]>,
    held: VecDeque<TxRecord>,
    /// The escrow transactions a quorum signed, in the order they came.
    submits: VecDeque<Submit>,
    /// Deposits attested whose attestation the hub has not recorded yet,
    /// by transaction: each holds the cursor below its ledger.
    deposits: BTreeMap<[u8; 32], Waiting>,
    /// The newest state of the issuer's hold it proved.
    proven_issuer: Option<Issuer>,
    /// When the hub was last seen to let go of the escrow.
    thawed_ms: Option<u64>,
    /// The newest anchor, and when it was proven.
    anchored: Option<(Anchor, u64)>,
    /// When the verifier was last asked to anchor on the watcher's own.
    anchor_asked_ms: Option<u64>,
    escrow_checked_ms: Option<u64>,
    attested_escrow: Option<u32>,
    issuer_checked_ms: Option<u64>,
    attested_issuer: Option<Issuer>,
    /// Whether it said the verifier stopped.
    failed_told: bool,
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
    now_ms: u64,
    out: VecDeque<Out>,
}

/// Building and reading it.
impl Watcher {
    /// A watcher of `escrow` on `network`, from where it left off, trusting
    /// the validators the publishers of `unl` list, whose validations
    /// `signatures` checks.
    #[must_use]
    pub fn new(
        escrow: AccountId,
        network: u32,
        cursor: Option<EscrowCursor>,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            escrow,
            network,
            cursor,
            phase: Phase::Disconnected,
            next_id: 1,
            pending: BTreeMap::new(),
            proof_asks: BTreeMap::new(),
            verifier: Verifier::new(network, unl, signatures),
            queries: BTreeMap::new(),
            hints: BTreeMap::new(),
            overflow_from: None,
            read_to: cursor.map_or(0, |cursor| cursor.scanned_to),
            streamed_to: 0,
            remembered: BTreeSet::new(),
            remembered_order: VecDeque::new(),
            held: VecDeque::new(),
            submits: VecDeque::new(),
            deposits: BTreeMap::new(),
            proven_issuer: None,
            thawed_ms: None,
            anchored: None,
            anchor_asked_ms: None,
            escrow_checked_ms: None,
            attested_escrow: None,
            issuer_checked_ms: None,
            attested_issuer: None,
            failed_told: false,
            view: None,
            untaken: BTreeMap::new(),
            ceiling: None,
            persisted: cursor,
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// Whether it follows the escrow's stream now.
    #[must_use]
    pub const fn following(&self) -> bool {
        matches!(self.phase, Phase::Following)
    }

    /// How far it followed the escrow.
    #[must_use]
    pub const fn cursor(&self) -> Option<EscrowCursor> {
        self.cursor
    }

    /// The newest anchor its verifier proved, if any.
    #[must_use]
    pub fn anchor(&self) -> Option<Anchor> {
        self.anchored.map(|(anchor, _)| anchor)
    }

    /// Whether the newest anchor is fresh enough to judge the newest
    /// ledger by ([`ANCHOR_FRESH_MS`]).
    #[must_use]
    pub fn fresh(&self) -> bool {
        self.anchored
            .is_some_and(|(_, at)| self.now_ms.saturating_sub(at) <= ANCHOR_FRESH_MS)
    }

    /// Whether `state` is as of the newest anchor, or later, and that
    /// anchor fresh: a state proven at an older anchor, asked before a new
    /// connection anchored anew and answered after, is not judged as the
    /// newest ledger.
    fn current(&self, state: &ProvenState) -> bool {
        self.fresh()
            && self
                .anchor()
                .is_some_and(|anchor| state.header.sequence >= anchor.index)
    }

    /// Its verifier, to read what it proved and the lies it caught.
    #[must_use]
    pub const fn verifier(&self) -> &Verifier {
        &self.verifier
    }

    /// The next thing to do.
    pub fn poll(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    /// Asserts what holds of it.
    pub fn check_invariants(&self) {
        self.verifier.check_invariants();
        assert!(self.hints.len() <= MAX_HINTS, "hints are bounded");
        assert!(self.held.len() <= MAX_HELD, "held outcomes are bounded");
        assert!(self.pending.len() <= MAX_PENDING, "requests are bounded");
        assert!(
            self.proof_asks.len() <= MAX_PROOF_ASKS,
            "proof asks are bounded"
        );
        assert!(self.remembered.len() <= MAX_REMEMBERED);
        assert_eq!(self.remembered.len(), self.remembered_order.len());
        assert!(self.submits.len() <= MAX_QUEUED_SUBMITS);
        assert!(self.pending_submits() <= MAX_PENDING_SUBMITS);
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
        for (id, purpose) in &self.queries {
            if let Purpose::Hint(hash) = purpose {
                assert_eq!(
                    self.hints.get(hash).and_then(|hint| hint.query),
                    Some(*id),
                    "a hint's query is its own"
                );
            }
        }
        if let (Some(cursor), Some((anchor, _))) = (self.cursor, self.anchored) {
            assert!(
                cursor.scanned_to <= cursor.newest_ledger.max(anchor.index),
                "the cursor passes no ledger beyond the newest proven"
            );
        }
        if self.phase == Phase::Disconnected {
            assert!(self.pending.is_empty() && self.proof_asks.is_empty());
            assert!(self.hints.is_empty(), "hints are a connection's");
        }
    }
}

/// Inputs.
impl Watcher {
    /// The validator took the attestation of `transaction`: it is
    /// remembered, and the cursor moves on once none is waiting before it.
    /// A deposit's waits on until the hub recorded it.
    pub fn taken(&mut self, transaction: Option<[u8; 32]>) {
        let Some(hash) = transaction else {
            return;
        };
        self.untaken.remove(&hash);
        match self.deposits.get_mut(&hash) {
            Some(waiting) => waiting.attested_ms = self.now_ms,
            None => self.remember(hash),
        }
        self.advance_cursor();
    }

    /// The host dropped the attestation of `transaction` before the
    /// validator took it: it waits no more. A deposit's is attested again
    /// later, and holds the cursor until the hub recorded it; of any other,
    /// no cursor this watcher persists passes its ledger, so a restart
    /// reads it again.
    pub fn dropped(&mut self, transaction: Option<[u8; 32]>) {
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
        self.advance_cursor();
    }

    /// A connection is up at `now_ms`: ask where the server stands, and
    /// have the verifier anchor on it.
    pub fn connected(&mut self, now_ms: u64) {
        self.now(now_ms);
        self.forget_connection();
        self.phase = Phase::Checking;
        self.ask(Asked::ServerState, api::server_state);
        self.verifier.connected(self.now_ms);
        self.drain_verifier();
    }

    /// The connection is gone. What it decided for the host stays to be
    /// carried out, but for what was to be sent on the connection and a
    /// request to end it.
    pub fn disconnected(&mut self) {
        self.forget_connection();
        self.phase = Phase::Disconnected;
        self.verifier.disconnected();
        // The verifier asks everything anew on the next connection: its
        // asks on this one are forgotten with it.
        let _ = self.verifier.take_out();
        // What was to go on this connection, or to end it, goes with it:
        // the next one is asked anew, and is not ended for this one's
        // sake.
        self.out
            .retain(|out| !matches!(out, Out::Send(_) | Out::Reconnect));
    }

    /// What was asked on the connection, and what its server hinted at, is
    /// forgotten: the next one reads the history again from the cursor,
    /// which passed no hint not yet proven.
    fn forget_connection(&mut self) {
        self.pending.clear();
        self.proof_asks.clear();
        let hinted: Vec<QueryId> = (self.queries.iter())
            .filter(|(_, purpose)| matches!(purpose, Purpose::Hint(_)))
            .map(|(id, _)| *id)
            .collect();
        for id in hinted {
            self.queries.remove(&id);
            self.verifier.cancel(id);
        }
        self.hints.clear();
        self.overflow_from = None;
        self.streamed_to = 0;
        // What was sent on it goes again on the next, once following.
        for submit in &mut self.submits {
            submit.sent_ms = None;
        }
        self.read_to = self.cursor.map_or(0, |cursor| cursor.scanned_to);
    }

    /// Submits a signed escrow transaction once it follows the escrow and
    /// the issuer does not hold it, and again while its withdrawal stays
    /// open.
    pub fn submit(&mut self, blob: Vec<u8>) {
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
            self.out.push_back(Out::Problem(
                "too many escrow transactions wait for the XRPL server: one was dropped",
            ));
            return;
        }
        self.submits.push_back(Submit {
            sequence,
            blob,
            sent_ms: None,
        });
        self.send_submits();
    }

    /// Does what is due at `now`: ask a server that was behind again, have
    /// the verifier anchor while the anchor ages, check the escrow while no
    /// quorum has and the issuer's hold, ask again for proofs the server
    /// did not find, and look again at outcomes held.
    pub fn tick(&mut self, now_ms: u64, view: &EscrowView) {
        self.now(now_ms);
        self.see(view);
        self.verifier.tick(self.now_ms);
        if let Phase::Waiting { since_ms } = self.phase
            && self.now_ms.saturating_sub(since_ms) >= RETRY_MS
        {
            self.phase = Phase::Checking;
            self.ask(Asked::ServerState, api::server_state);
        }
        let aging = self
            .anchored
            .is_none_or(|(_, at)| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        let rested = self
            .anchor_asked_ms
            .is_none_or(|at| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        if self.phase != Phase::Disconnected && aging && rested {
            self.anchor_asked_ms = Some(self.now_ms);
            self.verifier.anchor_now(self.now_ms);
        }
        let held = std::mem::take(&mut self.held);
        for record in held {
            self.record(&record);
        }
        self.retry_deposits();
        self.drain_verifier();
        self.work();
    }

    /// Takes a message from the server.
    pub fn message(&mut self, now_ms: u64, bytes: &[u8], view: &EscrowView) {
        self.now(now_ms);
        self.see(view);
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some(ask) = self.proof_asks.remove(&id) {
                    self.verifier.answered(&ask, outcome, self.now_ms);
                } else if let Some(asked) = self.pending.remove(&id) {
                    self.answer(asked, outcome);
                }
            }
            Message::Transaction(record) if self.subscribed() => self.hinted(&record),
            Message::LedgerClosed(closed) if self.subscribed() => {
                self.streamed_to = self.streamed_to.max(closed.ledger_index);
                if self.following() && self.overflow_from.is_none() {
                    self.read_to = self.read_to.max(closed.ledger_index);
                }
                self.anchor_asked_ms = Some(self.now_ms);
                self.verifier.anchor_now(self.now_ms);
            }
            Message::Transaction(_) | Message::LedgerClosed(_) | Message::Ignored => {}
        }
        self.drain_verifier();
        self.work();
    }

    /// A fetch of a list publisher's `url` answered with `body`, or failed.
    pub fn fetched(&mut self, now_ms: u64, url: &str, body: Option<&[u8]>) {
        self.now(now_ms);
        self.verifier.fetched(url, body, self.now_ms);
        self.drain_verifier();
        self.work();
    }
}

/// Requests and answers of its own.
impl Watcher {
    const fn now(&mut self, now_ms: u64) {
        if now_ms > self.now_ms {
            self.now_ms = now_ms;
        }
    }

    const fn subscribed(&self) -> bool {
        matches!(self.phase, Phase::Following | Phase::CatchingUp { .. })
    }

    /// Asks the server, unless its requests are full: whether it did.
    fn ask(&mut self, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        let id = self.fresh_id();
        self.pending.insert(id, asked);
        self.out.push_back(Out::Send(request(id)));
        true
    }

    /// How many of its requests submit an escrow transaction.
    fn pending_submits(&self) -> usize {
        (self.pending.values())
            .filter(|asked| **asked == Asked::Submit)
            .count()
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    fn answer(&mut self, asked: Asked, outcome: Result<Value, ApiError>) {
        match (asked, outcome) {
            (Asked::ServerState, Ok(result)) => self.server_state(&result),
            (Asked::Subscribe, Ok(_)) => self.read_history(),
            (Asked::History, Ok(result)) => self.history(&result),
            (Asked::Submit, Ok(result)) => {
                if api::submit_result(&result).is_none() {
                    self.out.push_back(Out::Problem(
                        "the XRPL server did not take an escrow transaction",
                    ));
                }
            }
            (Asked::ServerState | Asked::Subscribe | Asked::History, Err(_)) => self.wait(),
            (Asked::Submit, Err(_)) => {}
        }
    }

    /// Waits, and asks the server where it stands again later.
    fn wait(&mut self) {
        self.phase = Phase::Waiting {
            since_ms: self.now_ms,
        };
    }

    /// Where the server stands: on the hub's network, and on the chain the
    /// watcher follows, as far as it says. A server behind is waited for;
    /// a new chain is only a hint for the verifier to anchor, which proves
    /// it or not. Then it subscribes.
    fn server_state(&mut self, result: &Value) {
        let Some(state) = api::server_state_result(result) else {
            return self.wait();
        };
        if state.network_id != Some(self.network) {
            self.out.push_back(Out::Problem(
                "the XRPL server is on another network than the hub's escrow",
            ));
            return self.wait();
        }
        let newest = self.newest();
        match chain(
            newest,
            self.network,
            state.validated_ledger,
            state.close_time,
        ) {
            Chain::Current => {}
            Chain::Behind | Chain::Restarted => {
                self.verifier.anchor_now(self.now_ms);
                return self.wait();
            }
            Chain::WentBack => {
                self.out.push_back(Out::Problem(
                    "the XRPL server's ledger went back on a network that is never reset",
                ));
                return self.wait();
            }
        }
        let from = self
            .cursor
            .map_or(0, |cursor| cursor.scanned_to.saturating_sub(RESCAN_LEDGERS));
        self.phase = Phase::CatchingUp {
            from,
            to: state.validated_ledger,
            marker: None,
            asking: false,
        };
        let escrow = self.escrow;
        self.ask(Asked::Subscribe, |id| api::subscribe(id, escrow));
    }

    /// The newest ledger proven, and when it closed.
    fn newest(&self) -> Option<(u32, u64)> {
        self.cursor
            .map(|cursor| (cursor.newest_ledger, cursor.newest_close_time))
    }

    /// Asks for the next page of the escrow's history, once there is room
    /// for a whole page of hints.
    fn read_history(&mut self) {
        let room = self.hints.len().saturating_add(PAGE as usize) <= MAX_HINTS;
        let Phase::CatchingUp {
            from,
            to,
            marker,
            asking,
        } = &mut self.phase
        else {
            return;
        };
        if *asking || !room {
            return;
        }
        *asking = true;
        let (from, to, marker) = (*from, *to, marker.clone());
        let escrow = self.escrow;
        self.ask(Asked::History, |id| {
            api::account_tx(id, escrow, from, to, marker.as_ref())
        });
    }

    /// A page of the escrow's history: each transaction a hint, then the
    /// next page, or the stream from here on.
    fn history(&mut self, result: &Value) {
        let Some(page) = api::account_tx_result(result) else {
            self.out.push_back(Out::Problem(
                "the XRPL server's history of the escrow is unreadable",
            ));
            return self.wait();
        };
        for record in &page.transactions {
            self.hinted(record);
        }
        let Phase::CatchingUp {
            to, marker, asking, ..
        } = &mut self.phase
        else {
            return;
        };
        *asking = false;
        if page.marker.is_some() {
            *marker = page.marker;
            return self.read_history();
        }
        let to = *to;
        self.read_to = self.read_to.max(to).max(self.streamed_to);
        self.phase = Phase::Following;
        self.escrow_checked_ms = None;
        self.send_submits();
    }

    /// The server hints at `record`, in its stream or the escrow's
    /// history: its proof is asked for, unless it was attested already. A
    /// hint without room has the history read again from its ledger.
    fn hinted(&mut self, record: &TxRecord) {
        let hash = record.hash;
        let known = self.remembered.contains(&hash)
            || self.untaken.contains_key(&hash)
            || self.deposits.contains_key(&hash)
            || self.hints.contains_key(&hash)
            || self.held.iter().any(|held| held.hash == hash);
        if known {
            return;
        }
        if self.hints.len() >= MAX_HINTS {
            let ledger = record.ledger_index;
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
impl Watcher {
    /// Carries out what the verifier decided.
    fn drain_verifier(&mut self) {
        for out in self.verifier.take_out() {
            match out {
                verifier::Out::Ask(Ask::Fetch(url)) => self.out.push_back(Out::Fetch(url)),
                verifier::Out::Ask(ask) => self.ask_proof(ask),
                verifier::Out::Verdict(id, verdict) => self.verdict(id, verdict),
                verifier::Out::Anchored(anchor) => self.anchored(anchor, false),
                verifier::Out::Reset(anchor) => self.anchored(anchor, true),
                verifier::Out::Reconnect => {
                    self.out.push_back(Out::Problem(
                        "the XRPL server's answers did not prove, or it refused or left them: connecting anew",
                    ));
                    self.out.push_back(Out::Reconnect);
                }
            }
        }
        if self.verifier.failed() && !self.failed_told {
            self.failed_told = true;
            self.out.push_back(Out::Problem(
                "the XRP Ledger's validators validated two chains of a network that never resets: the escrow is followed no more",
            ));
        }
    }

    fn ask_proof(&mut self, ask: Ask) {
        if self.phase == Phase::Disconnected {
            return;
        }
        if self.proof_asks.len() >= MAX_PROOF_ASKS {
            self.out.push_back(Out::Reconnect);
            return;
        }
        let id = self.fresh_id();
        if let Some(request) = ask.request(id) {
            self.proof_asks.insert(id, ask);
            self.out.push_back(Out::Send(request));
        }
    }

    /// A ledger the UNL validated is the newest anchor; `reset` if the
    /// verifier proved it off the chain it kept. Where it stands against
    /// the newest ledger proven before: newer, and the cursor may pass up
    /// to it; lower and closed later, a reset of the test network, attested
    /// once, by the ledger it was proven at, unless the hub wrote one off
    /// since that ledger closed: a quorum saw it before this validator did,
    /// and a late vote only waits out the hub's quiet period.
    fn anchored(&mut self, anchor: Anchor, reset: bool) {
        let close_time = u64::from(anchor.close_time);
        let judged = match chain(self.newest(), self.network, anchor.index, close_time) {
            Chain::Current if reset => Chain::Restarted,
            judged => judged,
        };
        match judged {
            Chain::Current => {
                self.anchored = Some((anchor, self.now_ms));
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
                self.advance_cursor();
            }
            // Older than the newest proven before a restart: not current.
            Chain::Behind => {}
            Chain::WentBack => self.out.push_back(Out::Problem(
                "the XRP Ledger's validators validated a ledger off the chain followed, on a network that is never reset",
            )),
            Chain::Restarted => self.restarted(anchor),
        }
    }

    /// A reset of the test network, proven at `anchor`.
    fn restarted(&mut self, anchor: Anchor) {
        let close_time = u64::from(anchor.close_time);
        let closed_ms = close_time
            .saturating_add(XRPL_EPOCH_S)
            .saturating_mul(1_000);
        let written_off_at = self.view.as_ref().and_then(|view| view.written_off_at);
        if written_off_at.is_none_or(|at| at < closed_ms) {
            let reset = ValidatorOperation::AttestReset {
                ledger: anchor.index,
            };
            self.out.push_back(Out::Attest(reset, None));
        }
        self.out.push_back(Out::Problem(
            "the XRP Ledger testnet was reset: the escrow is checked again",
        ));
        let cursor = EscrowCursor {
            newest_ledger: anchor.index,
            newest_close_time: close_time,
            scanned_to: 0,
        };
        self.cursor = Some(cursor);
        self.ceiling = None;
        self.anchored = Some((anchor, self.now_ms));
        self.escrow_checked_ms = None;
        self.attested_escrow = None;
        self.issuer_checked_ms = None;
        self.attested_issuer = None;
        self.proven_issuer = None;
        // The old chain's deposits wait no more: their ledgers would hold
        // the new chain's cursor.
        self.deposits.clear();
        self.persist(cursor);
        // What the server said of the old chain goes with it: the escrow's
        // history is read again on the new one.
        if self.phase != Phase::Disconnected {
            self.forget_connection();
            self.phase = Phase::Checking;
            self.ask(Asked::ServerState, api::server_state);
        }
    }

    fn verdict(&mut self, id: QueryId, verdict: Verdict) {
        let Some(purpose) = self.queries.remove(&id) else {
            return;
        };
        match (purpose, verdict) {
            (Purpose::Hint(hash), Verdict::Applied(proven) | Verdict::Before(proven)) => {
                self.hints.remove(&hash);
                let record = proven.decoded.record;
                if record.hash == hash {
                    self.record(&record);
                }
            }
            (Purpose::Hint(hash), _) => self.not_found(hash),
            (Purpose::Escrow, Verdict::State(state)) => self.escrow_state(&state),
            (Purpose::Issuer, Verdict::State(state)) => self.issuer_state(&state),
            (Purpose::Escrow | Purpose::Issuer, _) => {}
        }
    }

    /// The server found no transaction it hinted at: it proved nothing, not
    /// even that there is none. Asked again later, and after a few tries on
    /// a new connection; the cursor stays below it meanwhile.
    fn not_found(&mut self, hash: [u8; 32]) {
        let now = self.now_ms;
        let Some(hint) = self.hints.get_mut(&hash) else {
            return;
        };
        hint.query = None;
        hint.tries = hint.tries.saturating_add(1);
        hint.retry_at_ms = now.saturating_add(HINT_RETRY_MS);
        if hint.tries >= MAX_HINT_TRIES {
            self.out.push_back(Out::Problem(
                "the XRPL server hints at an escrow transaction it does not prove: connecting anew",
            ));
            self.out.push_back(Out::Reconnect);
        }
    }

    /// The escrow's setup, proven at an anchor: its next sequence attested
    /// once it passes the check, if it is as of the newest anchor and that
    /// anchor is fresh.
    fn escrow_state(&mut self, state: &ProvenState) {
        let checked = match self.view.as_ref() {
            Some(view) if !view.checked && self.current(state) => {
                check_proven_escrow(&view.rules, state)
            }
            _ => return,
        };
        match checked {
            Ok(sequence) if self.attested_escrow != Some(sequence) => {
                self.attested_escrow = Some(sequence);
                self.out.push_back(Out::Attest(
                    ValidatorOperation::AttestEscrow { sequence },
                    None,
                ));
            }
            Ok(_) => {}
            Err(_) => self.out.push_back(Out::Problem(
                "the hub's escrow is not set up as its rules say: no deposits are credited",
            )),
        }
    }

    /// The issuer's hold on a token hub's escrow, proven at an anchor: kept
    /// as the newest proven, and attested if it is as of the newest anchor,
    /// that anchor fresh, and it differs from the hub's, or from what it
    /// attested since the hub's, as of a later ledger, and was not attested
    /// already. So the validator hears of each change it proved, a freeze
    /// again after a thaw among them, after the hub stopped too, when no
    /// block can show it (spec/areas/hubs.md#payout-bundles).
    fn issuer_state(&mut self, state: &ProvenState) {
        let (proven, hub) = match self.view.as_ref() {
            Some(view) if self.current(state) => (issuer_of(&view.rules.terms, state), view.issuer),
            _ => return,
        };
        let Some(proven) = proven else {
            return;
        };
        if self
            .proven_issuer
            .is_none_or(|newest| newest.ledger <= proven.ledger)
        {
            self.proven_issuer = Some(proven);
        }
        let hold = |issuer: Issuer| (issuer.frozen, issuer.fee);
        let differs = hold(proven) != hold(hub);
        let reverts = self
            .attested_issuer
            .is_some_and(|voted| voted.ledger > hub.ledger && hold(voted) != hold(proven));
        let forward = self
            .attested_issuer
            .is_none_or(|voted| voted != proven && voted.ledger <= proven.ledger);
        if (differs || reverts) && proven.ledger > hub.ledger && forward {
            self.attested_issuer = Some(proven);
            self.out
                .push_back(Out::Attest(ValidatorOperation::AttestIssuer(proven), None));
        }
    }
}

/// What it asks, and what it attests.
impl Watcher {
    /// Asks what is due: the next page of history, a history read again
    /// after an overflow, the proofs of hints, and the escrow's and the
    /// issuer's state while the anchor is fresh.
    fn work(&mut self) {
        if self.phase == Phase::Disconnected {
            return;
        }
        self.read_history();
        self.reread_overflow();
        self.prove_hints();
        self.send_submits();
        if !self.following() || !self.fresh() {
            return self.advance_cursor();
        }
        let Some(view) = self.view.clone() else {
            return;
        };
        if !view.checked
            && !self.open(Purpose::Escrow)
            && self.due(self.escrow_checked_ms, ESCROW_CHECK_EVERY_MS)
        {
            self.escrow_checked_ms = Some(self.now_ms);
            self.query(
                Purpose::Escrow,
                Query::State {
                    keys: escrow_keys(&view.rules.terms),
                    at: None,
                },
            );
        }
        if let Some(keys) = issuer_keys(&view.rules.terms)
            && !self.open(Purpose::Issuer)
            && self.due(self.issuer_checked_ms, ISSUER_CHECK_EVERY_MS)
        {
            self.issuer_checked_ms = Some(self.now_ms);
            self.query(
                Purpose::Issuer,
                Query::State {
                    keys: keys.to_vec(),
                    at: None,
                },
            );
        }
        self.advance_cursor();
    }

    /// Whether a query for `purpose` is open.
    fn open(&self, purpose: Purpose) -> bool {
        self.queries.values().any(|open| *open == purpose)
    }

    /// Whether what was done at `at` is due again, `every` later.
    fn due(&self, at: Option<u64>, every: u64) -> bool {
        at.is_none_or(|at| self.now_ms.saturating_sub(at) >= every)
    }

    fn query(&mut self, purpose: Purpose, query: Query) -> Option<QueryId> {
        let id = self.verifier.query(query, self.now_ms).ok()?;
        self.queries.insert(id, purpose);
        self.drain_verifier();
        Some(id)
    }

    /// Asks for the proofs of hints, the oldest ledgers first, as many at
    /// once as [`MAX_HINT_QUERIES`].
    fn prove_hints(&mut self) {
        let open = self
            .hints
            .values()
            .filter(|hint| hint.query.is_some())
            .count();
        let room = MAX_HINT_QUERIES.saturating_sub(open);
        if room == 0 {
            return;
        }
        let now = self.now_ms;
        let mut due: Vec<(u32, [u8; 32])> = (self.hints.iter())
            .filter(|(_, hint)| hint.query.is_none() && hint.retry_at_ms <= now)
            .map(|(hash, hint)| (hint.ledger, *hash))
            .collect();
        due.sort_unstable();
        for (_, hash) in due.into_iter().take(room) {
            let query = Query::Transaction { hash, from: 0 };
            let Some(id) = self.query(Purpose::Hint(hash), query) else {
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

    /// Once the hints a stream overflowed are proven, reads the history
    /// again from the first of them.
    fn reread_overflow(&mut self) {
        let Some(from) = self.overflow_from else {
            return;
        };
        // Outcomes held wait for the hub: read again only once one has
        // room, or the same would be held back again.
        let room = self.hints.len().saturating_add(PAGE as usize) <= MAX_HINTS
            && self.held.len() < MAX_HELD
            && self.deposits.len() < MAX_WAITING_DEPOSITS;
        if !self.following() || !room {
            return;
        }
        self.overflow_from = None;
        self.phase = Phase::CatchingUp {
            from,
            to: self.streamed_to.max(from),
            marker: None,
            asking: false,
        };
        self.read_history();
    }

    /// What a proven transaction of the escrow's attests.
    fn record(&mut self, record: &TxRecord) {
        let Some(view) = self.view.clone() else {
            return;
        };
        let known = self.remembered.contains(&record.hash)
            || self.untaken.contains_key(&record.hash)
            || self.deposits.contains_key(&record.hash);
        if !record.validated || known {
            return;
        }
        let terms = &view.rules.terms;
        if let Some(deposit) = deposit_of(record, terms) {
            if view.deposits_recorded.contains(&record.hash) {
                return self.remember(record.hash);
            }
            if self.deposits.len() >= MAX_WAITING_DEPOSITS {
                // Read again from the history once the hub recorded one.
                let ledger = record.ledger_index;
                self.overflow_from =
                    Some(self.overflow_from.map_or(ledger, |from| from.min(ledger)));
                return;
            }
            let waiting = Waiting {
                deposit,
                attested_ms: self.now_ms,
            };
            self.deposits.insert(record.hash, waiting);
            return self.attest(record, ValidatorOperation::AttestDeposit(deposit));
        }
        if let Some(clawback) = clawback_of(record, terms) {
            return self.attest(record, ValidatorOperation::AttestClawback(clawback));
        }
        let Some((sequence, succeeded)) = outcome_of(record, self.escrow) else {
            // Nothing the hub reads: a payment it takes as gas, a line's
            // change, someone else's.
            return self.remember(record.hash);
        };
        let withdrawal = view.withdrawals.iter().find(|(s, _)| *s == sequence);
        if let Some((_, withdrawal)) = withdrawal {
            match withdrawal_outcome(record, terms, sequence, withdrawal) {
                Some(delivered) => self.attest(
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
                self.attest(record, ValidatorOperation::AttestSignerList { sequence });
            } else {
                self.remember(record.hash);
            }
        } else if sequence >= view.next_sequence {
            // A sequence the hub has not given out, as far as this
            // validator executed: held until it has. Without room, it is
            // read again from the history once there is.
            if self.held.len() >= MAX_HELD {
                let ledger = record.ledger_index;
                self.overflow_from =
                    Some(self.overflow_from.map_or(ledger, |from| from.min(ledger)));
            } else {
                self.held.push_back(record.clone());
            }
        } else {
            // Settled already, or one of the escrow's setup: nothing to say.
            self.remember(record.hash);
        }
    }

    fn attest(&mut self, record: &TxRecord, operation: ValidatorOperation) {
        self.untaken.insert(record.hash, record.ledger_index);
        self.out
            .push_back(Out::Attest(operation, Some(record.hash)));
    }

    /// What the validator's ledger says now: when it let go of the escrow,
    /// and which deposits' attestations it recorded, which wait no more.
    fn see(&mut self, view: &EscrowView) {
        let was_held = self.view.as_ref().is_some_and(|seen| seen.issuer.holds());
        if was_held && !view.issuer.holds() {
            self.thawed_ms = Some(self.now_ms);
        }
        let mut recorded = false;
        for hash in &view.deposits_recorded {
            if self.deposits.remove(hash).is_some() {
                self.remember(*hash);
                recorded = true;
            }
        }
        self.view = Some(view.clone());
        if recorded {
            self.advance_cursor();
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

    /// The newest state of the issuer's hold it proved, if any.
    #[must_use]
    pub const fn proven_issuer(&self) -> Option<Issuer> {
        self.proven_issuer
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
    /// escrow and the issuer does not hold it, and while its requests have
    /// room: one that finds none goes on a later turn.
    fn send_submits(&mut self) {
        if !self.following() || self.held() {
            return;
        }
        self.prune_submits();
        let now = self.now_ms;
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
            if self.pending_submits() >= MAX_PENDING_SUBMITS {
                break;
            }
            let Some(blob) = self.submits.get(index).map(|submit| submit.blob.clone()) else {
                continue;
            };
            if !self.ask(Asked::Submit, |id| api::submit(id, &blob)) {
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
    fn retry_deposits(&mut self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        if self.deposits.is_empty() || self.held() {
            return;
        }
        let wait = view.rules.terms.timers.margin;
        let since = self.thawed_ms.unwrap_or(0);
        let now = self.now_ms;
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
            self.out.push_back(Out::Attest(
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

    /// Moves the cursor as far as it may: up to what the history and the
    /// stream covered, no further than the newest ledger proven, and below
    /// every hint not yet proven, every attestation not yet taken, every
    /// deposit the hub has not recorded, every outcome held and every
    /// ledger a page must be read again from.
    fn advance_cursor(&mut self) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let mut to = self.read_to.min(cursor.newest_ledger);
        let blocking = (self.hints.values().map(|hint| hint.ledger))
            .chain(self.untaken.values().copied())
            .chain(self.deposits.values().map(|waiting| waiting.deposit.ledger))
            .chain(self.held.iter().map(|record| record.ledger_index))
            .chain(self.overflow_from);
        for ledger in blocking {
            to = to.min(ledger.saturating_sub(1));
        }
        let cursor = EscrowCursor {
            scanned_to: cursor.scanned_to.max(to),
            ..cursor
        };
        self.cursor = Some(cursor);
        self.persist(cursor);
    }

    /// Persists `cursor` if it changed, never naming as attested a ledger
    /// past one whose attestation was dropped.
    fn persist(&mut self, cursor: EscrowCursor) {
        let scanned_to = self
            .ceiling
            .map_or(cursor.scanned_to, |ceiling| cursor.scanned_to.min(ceiling));
        let cursor = EscrowCursor {
            scanned_to,
            ..cursor
        };
        if self.persisted != Some(cursor) {
            self.persisted = Some(cursor);
            self.out.push_back(Out::Cursor(cursor));
        }
    }
}

#[cfg(test)]
mod tests;
