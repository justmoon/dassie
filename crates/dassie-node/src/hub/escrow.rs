//! Following a hub's escrow on the XRP Ledger, as a validator does
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals), without
//! IO: the requests to send on its own WebSocket to its XRPL server, the
//! validator lists to fetch, and what it attests from what it proved.
//!
//! It takes nothing on the server's word. A [`Verifier`] of its own,
//! trusting the hub's network's UNL, proves everything it attests against
//! ledgers that UNL validated (spec/designs/xrpl-validation.md#12-who-else-uses-it):
//!
//! - **Transactions by their proofs, found by the history the escrow's
//!   state names.** The escrow's account root and a token hub's line each
//!   name the transaction that last changed them, and each proven
//!   transaction's metadata the one that changed them before: from each
//!   anchor, the watcher walks these back to the ledger its last walk
//!   reached (`Walk`), as the node's account walks its own
//!   (spec/areas/xrpl.md#following-the-account), so a transaction its
//!   server leaves out of the escrow's history and of its stream is found
//!   all the same. The stream and the history (`account_tx`) only hint at
//!   hashes, sooner; each hint is proven, oldest first and many at a time,
//!   and its proof kept for the walk, which starts no higher than the
//!   hints proven cover and steps through their proofs rather than proving
//!   each in turn (`walk_top`): so a burst of payments to the
//!   escrow delays it no more than proving their hints takes.
//!   Each transaction is proven by its own bytes and metadata in a
//!   validated ledger, and read as `dassie_hub::escrow` reads it: a
//!   deposit, the outcome of a pending withdrawal, the old validators'
//!   change of the signer list, and for a token hub the issuer's clawback.
//!   A hint that proves nothing is attested never. The cursor passes a
//!   ledger only once a walk passed it, and no further than the newest
//!   proven ledger. A walk holds one transaction per object at a time, and
//!   one with no room yet among what waits for the hub waits rather than
//!   being dropped; a hint the stream brings past their bound
//!   ([`MAX_HINTS`]) has the history read again from its ledger: so no
//!   number of small payments to the escrow makes it skip a deposit.
//! - **State by state proofs, at a fresh anchor.** The escrow's setup is
//!   checked, while no quorum has, from proofs of its account root, its
//!   signer list and a token hub's line at one validated ledger; a token
//!   hub's issuer's hold (a freeze of the escrow's line, a global freeze, a
//!   transfer fee) from proofs of the line and the issuer's account root,
//!   named by the ledger it last changed in, which the watcher finds by
//!   proving the two as of the ledgers before (`HoldSearch`), so that
//!   validators name the same ledger however busy the issuer and the
//!   escrow are; and attested only where it differs from the hub's. What
//!   is judged as of the newest ledger is judged only from a proof as of
//!   the newest anchor, while it is fresh ([`ANCHOR_FRESH_MS`]): an old
//!   anchor delays attestations, never invents them.
//! - **Resets by the verifier.** A reset of the test network is attested
//!   only once a ledger of the new chain is proven under the UNL
//!   (spec/decisions/ledger-resets.md), never from what the server says
//!   of its validated ledger, which only tells the watcher to wait.
//!
//! - **Connecting:** check the network, and where the server's validated
//!   ledger stands against the newest proven ([`chain`]); subscribe to the
//!   escrow; read the escrow's history for hints from the last ledger
//!   attested, a window back ([`RESCAN_LEDGERS`]), where a restarted
//!   watcher's first walk ends too, since an attestation lost in a crash
//!   must go again and the hub refuses one sent twice; then follow the
//!   stream. A server that lies, refuses or stops answering is left for a
//!   new connection ([`Out::Reconnect`]), and what it hinted is forgotten:
//!   the next connection reads the history again, and the walk goes on.
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
//! - **The verifier's state across a restart.** Its UNL's lists and
//!   manifests, whether it is UNL-blocked, and its anchor go to its host
//!   with the cursor, after the hub's network ([`Watcher::take_kept`]):
//!   once they changed, at most every [`KEEP_VERIFIER_EVERY_MS`], and at
//!   once after a reset, so a restart never meets the chain gone as a new
//!   reset. A restarted watcher takes them back before it connects
//!   ([`Watcher::restore`]), verified again at the restart's time, so a
//!   restart neither lifts a block nor takes an older anchor as current:
//!   the anchor taken back is current to nothing, and no hint is proven
//!   against it, as it may be of a chain that reset while the validator
//!   was down, until a newer one is proven; one off its chain is a reset on
//!   the testnet, and a fork, which stops the verifier, on any other
//!   network. A state of another network, or one the verifier does not
//!   read, is dropped, and the verifier starts afresh, its cursor's newest
//!   ledger still telling a server behind, or a chain gone back.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_hub::Rules;
use dassie_hub::escrow::{
    IssuerHold, check_proven_escrow, clawback_of, deposit_of, escrow_keys, hold_of, issuer_keys,
    outcome_of, withdrawal_outcome,
};
use dassie_hub::ledger::Withdrawal;
use dassie_hub::operation::{Deposit, Issuer, ValidatorOperation};
use dassie_hub::service::MAX_WAITING_DEPOSITS;
use dassie_xrpl::AccountId;
use dassie_xrpl::account::{HISTORY_BOUND, resets};
use dassie_xrpl::anchor::ProvenState;
use dassie_xrpl::api::{self, ApiError, Message, PAGE, TxRecord};
use dassie_xrpl::codec::{Payment, Transact};
use dassie_xrpl::records::{self, Decoded, Thread};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{
    self, ANCHOR_EVERY_MS, Ask, Query, QueryId, REQUEST_TIMEOUT_MS, STALL_MS, Verdict, Verifier,
};
use serde_json::Value;

use crate::store::{EscrowCursor, MAX_HUB_VERIFIER_BYTES};

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

/// How often at most the verifier's state goes to the host with the cursor
/// while it changes. What changes it is mostly its anchor, about every
/// [`ANCHOR_EVERY_MS`], with lists and manifests that are far larger:
/// written with each anchor, the lists' bytes would be rewritten every few
/// seconds. What a restart loses of a minute is anchors, which the next
/// round proves forward from the one kept, and lists and manifests, which
/// it fetches again; a list that lapsed since still blocks, as the state is
/// verified again at the restart's time.
pub const KEEP_VERIFIER_EVERY_MS: u64 = 60_000;

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
/// whole one; a hint the stream brings without room has the history read
/// again from its ledger, once there is.
pub const MAX_HINTS: usize = 2 * PAGE as usize;

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
    /// Persist how far it followed the escrow, and with it, in the same
    /// write, the verifier's state [`Watcher::take_kept`] hands over, if
    /// it does.
    Cursor(EscrowCursor),
    /// Something the owner should read in the log.
    Problem(&'static str),
}

/// Why what a watcher kept of its verifier before a restart is not taken
/// back: the verifier starts afresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unrestored {
    /// It was kept on another network.
    Network(u32),
    /// The verifier does not read it: of a version it does not read, cut
    /// short, or not what it writes.
    Unread,
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
    /// Subscribed, and reading the history from the cursor.
    CatchingUp,
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
    submits: VecDeque<Submit>,
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
    /// The newest anchor, and when it was proven.
    anchored: Option<(Anchor, u64)>,
    /// When the verifier was last asked to anchor on the watcher's own.
    anchor_asked_ms: Option<u64>,
    escrow_checked_ms: Option<u64>,
    attested_escrow: Option<u32>,
    issuer_checked_ms: Option<u64>,
    attested_issuer: Option<Issuer>,
    /// What it attested of the issuer's hold before `attested_issuer`: what
    /// the validator last heard, should the host drop that.
    attested_issuer_before: Option<Issuer>,
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
    /// Whether its verifier's anchor was taken back from before a restart,
    /// and none proven since: no hint is proven against it, as it may be of
    /// a chain gone while the validator was down.
    taken_back: bool,
    /// The verifier's count of changes when its state was last handed to
    /// the host, or taken back.
    kept_changes: u64,
    /// When it was last handed to the host.
    kept_ms: Option<u64>,
    /// The state handed to the host with the cursor, until it takes it.
    keeping: Option<Vec<u8>>,
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
            anchor_asked_ms: None,
            escrow_checked_ms: None,
            attested_escrow: None,
            issuer_checked_ms: None,
            attested_issuer: None,
            attested_issuer_before: None,
            failed_told: false,
            view: None,
            untaken: BTreeMap::new(),
            ceiling: None,
            persisted: cursor,
            taken_back: false,
            kept_changes: 0,
            kept_ms: None,
            keeping: None,
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// Takes back at `now_ms` what it handed its host of its verifier
    /// before a restart ([`Self::take_kept`]), before it first connects: the
    /// verifier's state, verified again at `now_ms`, whose validations
    /// `signatures` checks, as [`Self::new`]'s do. The anchor it holds is
    /// current to nothing, and no hint is proven against it: only a newer
    /// one, proven with fresh signatures, moves the cursor.
    ///
    /// # Errors
    ///
    /// Why it is not taken back, kept on another network or unread: the
    /// verifier then starts afresh, as [`Self::new`] made it.
    pub fn restore(
        &mut self,
        kept: &[u8],
        signatures: Box<dyn ValidationSignatures + Send>,
        now_ms: u64,
    ) -> Result<(), Unrestored> {
        assert!(
            self.phase == Phase::Disconnected && self.queries.is_empty(),
            "taken back before it follows the escrow"
        );
        let (network, state) = kept.split_first_chunk::<4>().ok_or(Unrestored::Unread)?;
        let network = u32::from_be_bytes(*network);
        if network != self.network {
            return Err(Unrestored::Network(network));
        }
        let config = self.verifier.unl().config().clone();
        let verifier = Verifier::restore(self.network, config, signatures, state, now_ms)
            .ok_or(Unrestored::Unread)?;
        self.verifier = verifier;
        self.now(now_ms);
        self.taken_back = self.verifier.anchor().is_some();
        self.kept_changes = self.verifier.changes();
        // The threads it reads proven transactions for are no part of its
        // state.
        if !self.walk_keys.is_empty() {
            self.verifier.thread(self.walk_keys.clone());
        }
        Ok(())
    }

    /// The verifier's state, after the hub's network, to write with the
    /// cursor of the [`Out::Cursor`] just taken, if it is handed over now.
    pub fn take_kept(&mut self) -> Option<Vec<u8>> {
        self.keeping.take()
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
        assert!(
            (self.keeping.as_ref()).is_none_or(|kept| kept.len() <= MAX_HUB_VERIFIER_BYTES),
            "the verifier's state handed over is bounded"
        );
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
        if self.phase == Phase::Disconnected {
            assert!(self.pending.is_empty() && self.proof_asks.is_empty());
            assert!(self.hints.is_empty(), "hints are a connection's");
            assert!(self.reading.is_none() && self.overflow_from.is_none());
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

    /// The host dropped `operation`, the attestation of `transaction`,
    /// before the validator took it: it waits no more. A deposit's is
    /// attested again later, and holds the cursor until the hub recorded
    /// it; of any other transaction, no cursor this watcher persists passes
    /// its ledger, so a restart reads it again. The escrow's check and the
    /// issuer's hold are attested again once proven again: the validator
    /// signs a stopped hub's payout set by what it heard of the hold.
    pub fn dropped(&mut self, operation: &ValidatorOperation, transaction: Option<[u8; 32]>) {
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
    /// forgotten: the next one reads the history again from the cursor.
    /// The verifier asks its open queries again on the next, a walk's and
    /// a search's among them.
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
                self.out.push_back(Out::Problem(
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
        // Each was held with room: taken again, each has it again.
        let held = std::mem::take(&mut self.held);
        for record in held {
            let _ = self.record(&record);
        }
        self.retry_deposits();
        self.drain_verifier();
        self.work();
        self.keep(false);
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
        matches!(self.phase, Phase::Following | Phase::CatchingUp)
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
        // The history is read again once it follows the server again.
        self.reading = None;
        self.overflow_from = None;
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
        self.phase = Phase::CatchingUp;
        self.reading = Some(Reading {
            from,
            to: state.validated_ledger,
            marker: None,
            asking: false,
            through: from.saturating_sub(1),
        });
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
        let Some(reading) = self.reading.as_mut() else {
            return;
        };
        if reading.asking || !room {
            return;
        }
        reading.asking = true;
        let (from, to, marker) = (reading.from, reading.to, reading.marker.clone());
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
            return self.read_history();
        }
        self.reading = None;
        if self.phase == Phase::CatchingUp {
            self.phase = Phase::Following;
            self.escrow_checked_ms = None;
            self.send_submits();
        }
    }

    /// Once there is room for a page of hints, reads the history again
    /// from the first hint the stream brought without room, while it
    /// follows the stream.
    fn reread_overflow(&mut self) {
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
        self.read_history();
    }

    /// The server hints at `record`, in its stream or the escrow's
    /// history: its proof is asked for, unless it was attested already, or
    /// its ledger is one a walk passed. A hint the stream brings without
    /// room has the history read again from its ledger once there is
    /// room, so that it is proven with the others, many at a time, and the
    /// walk steps through their proofs.
    fn hinted(&mut self, record: &TxRecord) {
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
        // Proven with fresh signatures on the chain it is on: the chain
        // lives, whichever the anchor taken back was on.
        self.taken_back = false;
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
        self.attested_issuer_before = None;
        self.proven_issuer = None;
        // What was proven of the old chain's history, and of its issuer's
        // hold, is no part of the new chain's.
        self.cancel_walk();
        self.cancel_search();
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
        self.persist(cursor);
        // The new chain's anchor is kept at once: kept as of the chain
        // gone, a restart would meet the new one as another reset.
        self.keep(true);
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
                let decoded = proven.decoded;
                if decoded.record.hash != hash {
                    return;
                }
                // Without room, it waits for the walk; with none for its
                // proof, the walk proves it again.
                let taken = self.record(&decoded.record);
                self.keep_ahead(decoded, taken);
            }
            (Purpose::Hint(hash), _) => self.not_found(hash),
            (Purpose::Escrow, Verdict::State(state)) => self.escrow_state(&state),
            (Purpose::Issuer, Verdict::State(state)) => self.hold_proven(&state),
            (Purpose::Walk, Verdict::State(state)) => self.walk_from(&state),
            (Purpose::Step(key), Verdict::Applied(proven) | Verdict::Before(proven)) => {
                if let Some(index) = self.stepped(key, id) {
                    let decoded = &proven.decoded;
                    let record = &decoded.record;
                    self.found(
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
                    self.step_not_found(index);
                }
            }
            (Purpose::HoldBefore, verdict) => self.hold_before(id, verdict),
            (Purpose::Escrow | Purpose::Issuer | Purpose::Walk, _) => {}
        }
    }

    /// The server found no transaction it hinted at: it proved nothing, not
    /// even that there is none. Asked again later, and after a few tries on
    /// a new connection; the walk finds it meanwhile, if it is the escrow's.
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

    /// The issuer's hold on a token hub's escrow, proven and named by the
    /// ledger it last changed in: attested if the newest anchor is fresh,
    /// and it differs from the hub's, or from what it attested since the
    /// hub's, as of a later ledger, and was not attested already. So the
    /// validator hears of each change it proved, a freeze again after a
    /// thaw among them, after the hub stopped too, when no block can show
    /// it (spec/areas/hubs.md#payout-bundles).
    fn judge_issuer(&mut self, proven: Issuer) {
        let hub = match self.view.as_ref() {
            Some(view) if self.fresh() => view.issuer,
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
            self.out
                .push_back(Out::Attest(ValidatorOperation::AttestIssuer(proven), None));
        }
    }
}

/// What it asks, and what it attests.
impl Watcher {
    /// Asks what is due: the next page of history, the proofs of hints, the
    /// walk's next steps, and the escrow's and the issuer's state while the
    /// anchor is fresh.
    fn work(&mut self) {
        if self.phase == Phase::Disconnected {
            return;
        }
        self.reread_overflow();
        self.read_history();
        self.prove_hints();
        self.start_walk();
        self.step_walk();
        self.search_hold();
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
        // Proven against an anchor taken back, a transaction of a chain
        // that reset while the validator was down would be applied.
        if self.taken_back {
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
    fn record(&mut self, record: &TxRecord) -> bool {
        let Some(view) = self.view.clone() else {
            return false;
        };
        if !record.validated || self.known(&record.hash) {
            return true;
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
                attested_ms: self.now_ms,
            };
            self.deposits.insert(record.hash, waiting);
            self.attest(record, ValidatorOperation::AttestDeposit(deposit));
            return true;
        }
        if let Some(clawback) = clawback_of(record, terms) {
            self.attest(record, ValidatorOperation::AttestClawback(clawback));
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
        let keys = walk_keys(&view.rules.terms);
        if keys != self.walk_keys {
            self.verifier.thread(keys.clone());
            self.walk_keys = keys;
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

    /// The newest state of the issuer's hold it proved, as of the ledger it
    /// was proven at, if any.
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

    /// Moves the cursor as far as it may: up to where the last walk of the
    /// escrow's history reached, no further than the newest ledger proven,
    /// and below every attestation not yet taken, every deposit the hub has
    /// not recorded and every outcome held.
    fn advance_cursor(&mut self) {
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

    /// Hands the verifier's state to the host with the cursor once it
    /// changed, and [`KEEP_VERIFIER_EVERY_MS`] passed since it last did, or
    /// at once if `now`: with a cursor going out already, or with the one
    /// persisted last. Before a cursor is, there is nothing to keep it
    /// with, and a restart starts as the first start did.
    fn keep(&mut self, now: bool) {
        let changes = self.verifier.changes();
        let due = now
            || (self.kept_ms)
                .is_none_or(|at| self.now_ms.saturating_sub(at) >= KEEP_VERIFIER_EVERY_MS);
        let Some(cursor) = self.persisted else {
            return;
        };
        if changes == self.kept_changes || !due {
            return;
        }
        self.kept_changes = changes;
        self.kept_ms = Some(self.now_ms);
        let mut kept = self.network.to_be_bytes().to_vec();
        kept.extend(self.verifier.state());
        if kept.len() > MAX_HUB_VERIFIER_BYTES {
            self.keeping = None;
            self.out.push_back(Out::Problem(
                "the XRPL verifier's lists are too large to keep: a restart fetches them anew",
            ));
            return;
        }
        self.keeping = Some(kept);
        if !(self.out.iter()).any(|out| matches!(out, Out::Cursor(_))) {
            self.out.push_back(Out::Cursor(cursor));
        }
    }
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
impl Watcher {
    /// Starts a walk once the last one ended, by proving the escrow's
    /// objects at [`Self::walk_top`].
    fn start_walk(&mut self) {
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
        self.query(Purpose::Walk, Query::State { keys, at });
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
    fn walk_from(&mut self, state: &ProvenState) {
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
                        return self.out.push_back(Out::Problem(
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
        self.step_walk();
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
    fn step_walk(&mut self) {
        let chains = self.walk.as_ref().map_or(0, |walk| walk.chains.len());
        for index in 0..chains {
            for _ in 0..MAX_CHAIN_STEPS {
                if !self.step_chain(index) {
                    break;
                }
            }
        }
        self.end_walk();
    }

    /// One step along chain `index`: a transaction found that now finds
    /// room, or the next one, from its proof kept from a hint, or asked
    /// for. Whether it moved, and may move again.
    fn step_chain(&mut self, index: usize) -> bool {
        let Some(chain) = self.chain(index) else {
            return false;
        };
        if let Some((record, before)) = chain.waiting.clone() {
            if !self.record(&record) {
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
        if retry_at_ms > self.now_ms {
            return false;
        }
        let Some(id) = self.query(
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
            self.cancel_walk();
            self.out.push_back(Out::Problem(
                "the escrow's history did not prove as its state names it: connecting anew",
            ));
            self.out.push_back(Out::Reconnect);
            return false;
        };
        match record {
            Some(record) if !self.record(record) => {
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
    fn step_not_found(&mut self, index: usize) {
        let anchor = self.anchor().map_or(0, |anchor| anchor.index);
        let now = self.now_ms;
        let Some(chain) = self.chain_mut(index) else {
            return;
        };
        let Some((_, ledger)) = chain.next else {
            return;
        };
        if anchor.saturating_sub(ledger) > HISTORY_BOUND {
            self.out.push_back(Out::Problem(
                "the escrow's history is older than XRPL servers keep: what is older is not read",
            ));
            return self.chain_to(index, None);
        }
        chain.tries = chain.tries.saturating_add(1);
        chain.retry_at_ms = now.saturating_add(HINT_RETRY_MS);
        if chain.tries >= MAX_HINT_TRIES {
            chain.tries = 0;
            self.out.push_back(Out::Problem(
                "the XRPL server does not prove the escrow's history its state names: connecting anew",
            ));
            self.out.push_back(Out::Reconnect);
        }
    }

    /// Ends the walk once each chain did: every transaction that changed
    /// the escrow's objects up to its top was found, and taken, and the
    /// cursor may pass it.
    fn end_walk(&mut self) {
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
        self.advance_cursor();
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
    fn cancel_walk(&mut self) {
        let Some(walk) = self.walk.take() else {
            return;
        };
        for id in walk.chains.iter().filter_map(|chain| chain.query) {
            self.queries.remove(&id);
            self.verifier.cancel(id);
        }
    }
}

/// Naming the issuer's hold by the ledger it last changed in.
impl Watcher {
    /// The issuer's hold, proven at the newest anchor while it is fresh:
    /// kept as the newest proven, as of that ledger, which is what the
    /// watcher submits by. Where it differs from the hub's, or from what
    /// this validator attested since the hub's, a search for the ledger it
    /// last changed in starts: anything else attests nothing, and needs no
    /// name.
    fn hold_proven(&mut self, state: &ProvenState) {
        if !self.current(state) {
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
        self.search_hold();
    }

    /// Goes on with the search: it ends once the hold is known the same
    /// over a ledger it was named at, by this watcher or by a quorum of the
    /// hub's validators; else the ledger before the one it is proven the
    /// same since is proven next.
    fn search_hold(&mut self) {
        let Some(search) = self.search else {
            return;
        };
        if search.query.is_some() {
            return;
        }
        if let Some(ledger) = self.known_name(&search) {
            return self.hold_named(search, ledger);
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
        let Some(id) = self.query(Purpose::HoldBefore, query) else {
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
    fn hold_before(&mut self, id: QueryId, verdict: Verdict) {
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
                        self.search_hold();
                    }
                    _ => self.hold_named(search, search.since),
                }
            }
            (Verdict::NoLedger, _) => self.hold_named(search, search.since),
            (Verdict::NoHistory, _) => {
                self.search = None;
                self.unnamed = Some((search.hold, search.since, search.top));
                self.out.push_back(Out::Problem(
                    "the XRPL server keeps too short a history to name the issuer's hold by the ledger it changed in",
                ));
            }
            _ => self.search = None,
        }
    }

    /// The search found the ledger the hold last changed in: it is judged.
    fn hold_named(&mut self, search: HoldSearch, ledger: u32) {
        self.search = None;
        let proven = search.hold.named(ledger);
        let to = match self.named {
            Some((named, to)) if named == proven => to.max(search.top),
            _ => search.top,
        };
        self.named = Some((proven, to));
        self.judge_issuer(proven);
    }

    /// Gives up the search under way.
    fn cancel_search(&mut self) {
        if let Some(id) = self.search.take().and_then(|search| search.query) {
            self.queries.remove(&id);
            self.verifier.cancel(id);
        }
    }
}

#[cfg(test)]
mod tests;
