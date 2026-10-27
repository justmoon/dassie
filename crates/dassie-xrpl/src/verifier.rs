//! The standalone verifier (spec/areas/xrpl.md#the-verifier), sans-IO:
//! what any account or ledger object holds, and whether a transaction
//! applied, proven against ledgers a quorum of its own UNL validated, for
//! whoever follows the XRP Ledger without an account of the node's own,
//! such as a settlement hub's validator watching its escrow.
//!
//! - **It keeps** the UNL, from its publishers' lists, which it fetches
//!   through its holder ([`crate::lists`]), and an anchor: the newest
//!   ledger a quorum of that UNL validated with fresh signatures, which it
//!   proves as the account does (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
//!   An older anchor is never current, a later one is current only once
//!   its skip lists prove the kept one its ancestor, and one off the kept
//!   chain is a reset on a network that resets
//!   (spec/decisions/ledger-resets.md), and stops the verifier on any
//!   other.
//! - **It answers queries:** what the state of a set of keys holds at a
//!   ledger, by its index or at the anchor, each object or its absence
//!   proven by its branch to the state root of a ledger the anchor's chain
//!   holds ([`Query::State`]); and whether a transaction applied, by its
//!   own bytes and metadata proven in such a ledger, with its result and
//!   the ledger it is in ([`Query::Transaction`]). A server that finds no
//!   such transaction proves nothing, not even its absence.
//! - **It says what to ask** ([`Ask`]), and its holder sends each on its
//!   connection to a proof server and hands the answer back
//!   ([`Verifier::answered`]); a list's site it fetches over HTTPS
//!   ([`Verifier::fetched`]). What the server says is never taken on its
//!   word: an answer that does not prove is a lie, counted by kind
//!   ([`Doubt`]), after which, as after a refusal, the verifier waits
//!   [`STALL_MS`] and asks for a new connection ([`Out::Reconnect`]), and
//!   asks every open query again on it. An ask left unanswered for
//!   [`REQUEST_TIMEOUT_MS`] asks for one at once: the connection may be
//!   half open.
//!
//! Time is an input, every table has a bound, and nothing recurses
//! (spec/decisions/tigerstyle.md).

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::Value;

use crate::anchor::{
    Ancestry, Doubt, Judged, KeptAnchor, LedgerHash, LedgerProof, ProvenLists, ProvenState,
    ProvenTx, ancestry, judge, ledger_hash, ledger_proof, prove_state, prove_transaction,
};
use crate::api::{self, ApiError, MAX_PROOF_KEYS, TxProof, Validations};
use crate::kept;
use crate::keys::PublicKey;
use crate::lists::{self, ListFetches};
use crate::skip_list::{SkipList, recent_key};
use crate::unl::{Anchor, AnchorError, Unl, UnlError, ValidationSignatures};
use crate::validator_list::{MAX_PUBLISHERS, UnlConfig};

/// How long after a server refused a proof, told a lie or kept too short a
/// history the verifier waits before it asks for a new connection, perhaps
/// to another server behind the same address: so that a server that keeps
/// failing costs a connection every so often, not one at once after
/// another. The account waits as long.
pub const STALL_MS: u64 = 30_000;
/// How long an ask of the server may go unanswered before the verifier
/// asks for a new connection: as long as the account waits.
pub const REQUEST_TIMEOUT_MS: u64 = crate::account::REQUEST_TIMEOUT_MS;
/// The most validators of the UNL a round asks the server's `manifest`
/// method about: the next round asks the next ones.
pub const MAX_MANIFEST_ASKS: usize = 16;
/// How long after a server answered that it had not saved a transaction
/// with its ledger yet its bytes are asked again: rippled saves a ledger
/// within moments of validating it.
pub const UNSAVED_RETRY_MS: u64 = 1_000;
/// How often the bytes of one transaction are asked again so: past it, the
/// server is taken as refusing them.
pub const MAX_UNSAVED_TRIES: u32 = 8;
/// The most queries open at once.
pub const MAX_QUERIES: usize = 64;
/// How often the verifier anchors again on its own while a query waits
/// for a later anchor, or none is kept: about as often as the ledger
/// closes. Its holder anchors it sooner on each closed ledger's hint
/// ([`Verifier::anchor_now`]).
pub const ANCHOR_EVERY_MS: u64 = 4_000;
/// Most skip lists kept to prove the ledgers queries are at.
const MAX_LISTS: usize = 16;
/// Most skip lists asked for at once for queries.
const MAX_LIST_ASKS: usize = 4;
/// Most skip lists the server said it does not keep, on this connection.
const MAX_MISSING: usize = MAX_LISTS;
/// Most asks waiting for an answer: one for each open query (a closed
/// query's goes with it, unless another open one shares it), the skip
/// lists for queries and an anchoring round's, and the round's proof or
/// its manifests.
const MAX_PENDING: usize = MAX_QUERIES + MAX_LIST_ASKS + 1 + MAX_MANIFEST_ASKS;
/// Most outputs of one input, which its holder takes before the next: the
/// asks of what is pending, twice, as a reset asks everything anew; a
/// verdict for each query; the lists' fetches; and an anchor, a reset and a
/// reconnect.
const MAX_OUT: usize = 2 * MAX_PENDING + MAX_QUERIES + MAX_PUBLISHERS + 3;

/// What the verifier asks of its server, or of a list publisher's site.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ask {
    /// A state proof of the list of recent ledgers at the server's
    /// validated ledger: an anchoring round's first step.
    Validated,
    /// The validations of the ledger of `hash`: of the validators the
    /// server trusts, or with `untrusted`, of other keys too.
    Validations {
        /// The ledger.
        hash: [u8; 32],
        /// Whether to ask for others than the server's.
        untrusted: bool,
    },
    /// The newest manifest of a validator, by its master key.
    Manifest(PublicKey),
    /// A state proof of the skip list of `key` at the ledger of `at`.
    SkipList {
        /// The ledger.
        at: [u8; 32],
        /// The list's key.
        key: [u8; 32],
    },
    /// A state proof of `keys` at the ledger of `at`.
    State {
        /// The ledger.
        at: [u8; 32],
        /// The keys, each once.
        keys: Vec<[u8; 32]>,
    },
    /// The branch of a transaction, wherever the server finds it.
    TxProof([u8; 32]),
    /// A transaction's bytes and metadata.
    TxBlobs([u8; 32]),
    /// A list publisher's site, by an HTTPS GET that follows no redirect
    /// and reads at most
    /// [`MAX_LIST_BYTES`](crate::validator_list::MAX_LIST_BYTES).
    Fetch(String),
}

impl Ask {
    /// The request to send under `id`; `None` for a fetch, which is no
    /// request on the connection.
    #[must_use]
    pub fn request(&self, id: u64) -> Option<Vec<u8>> {
        Some(match self {
            Self::Validated => api::state_proof(id, &[recent_key()], None),
            Self::Validations { hash, untrusted } => api::validations(id, hash, *untrusted),
            Self::Manifest(master) => api::manifest(id, master),
            Self::SkipList { at, key } => api::state_proof(id, &[*key], Some(at)),
            Self::State { at, keys } => api::state_proof(id, keys, Some(at)),
            Self::TxProof(hash) => api::tx_proof(id, hash, None),
            Self::TxBlobs(hash) => api::tx_binary(id, hash),
            Self::Fetch(_) => return None,
        })
    }
}

/// A question to the verifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// What the state holds under each of `keys`, each once and at most
    /// [`MAX_PROOF_KEYS`]: at the ledger of index `at`, or at the anchor.
    State {
        /// The keys ([`crate::records`]).
        keys: Vec<[u8; 32]>,
        /// The ledger, by its index; `None` for the anchor.
        at: Option<u32>,
    },
    /// Whether the transaction of `hash` applied, and whether in a ledger
    /// at or after `from`.
    Transaction {
        /// The transaction.
        hash: [u8; 32],
        /// The earliest ledger it is to be in.
        from: u32,
    },
}

/// A query's id.
pub type QueryId = u64;

/// Why a query is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// [`MAX_QUERIES`] are open.
    Full,
    /// No key, a key twice, or more than [`MAX_PROOF_KEYS`].
    Keys,
}

/// What a query found: each only once proven, but the answers that prove
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The state at the ledger asked for, which the anchor's chain holds:
    /// each key's object, or its absence ([`ProvenState::object`]).
    State(ProvenState),
    /// The transaction, with its metadata and result, in a ledger the
    /// anchor's chain holds, at or after the one asked for.
    Applied(ProvenTx),
    /// The same, in a ledger before the one asked for.
    Before(ProvenTx),
    /// The server found no such transaction: nothing is proven, not even
    /// that it never applied.
    NotFound,
    /// The server does not keep the ledger asked for.
    NoHistory,
    /// The anchor's chain holds no ledger of the index asked for: it began
    /// after it.
    NoLedger,
}

/// What the verifier has its holder do, or tells it. Taken apart at once:
/// boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ask this, and hand its answer back.
    Ask(Ask),
    /// A query's verdict: the query is closed.
    Verdict(QueryId, Verdict),
    /// A newer anchor, on the kept one's chain.
    Anchored(Anchor),
    /// A reset of a network that resets: the UNL validated a chain the kept
    /// anchor is not on, and its ledger is the anchor now.
    Reset(Anchor),
    /// Connect anew, and say so with [`Verifier::connected`]: the server
    /// lied, refused or failed, and the verifier waited [`STALL_MS`]; or it
    /// left an ask unanswered for [`REQUEST_TIMEOUT_MS`]; or the network
    /// reset, after which an answer still on its way may be of the chain
    /// gone. The verifier is disconnected meanwhile.
    Reconnect,
}

/// Why the verifier waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// There is no UNL.
    Unl(UnlError),
    /// No quorum of the UNL signed the server's validated ledger.
    NoQuorum,
    /// The server cut its validations short of a quorum.
    Cut,
    /// The server serves no proofs.
    NoProofs,
    /// The server refused a proof.
    Refused,
    /// The server's validated ledger is older than the anchor.
    Older,
    /// The server's validated ledger is not fresh by the clock.
    NotFresh,
    /// The server's answers did not prove.
    NotProven,
    /// The server's history is too short to tell an anchor's ancestry.
    History,
    /// The server left an ask unanswered.
    NotAnswered,
}

/// What the verifier proved, by kind; never kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Proofs {
    /// Anchors taken.
    pub anchors: u64,
    /// Transactions proven.
    pub transactions: u64,
    /// State proofs of queries proven.
    pub states: u64,
    /// Keys of those proven absent.
    pub absences: u64,
}

/// An anchoring round in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Round {
    /// The state proof at the server's validated ledger is asked for.
    Proving,
    /// That ledger's validations are.
    Validating {
        proven: ProvenState,
        untrusted: bool,
    },
    /// Newer manifests of the validators that did not count are, `asked`
    /// of them still unanswered.
    Manifests {
        proven: ProvenState,
        validations: Validations,
        asked: usize,
    },
    /// The ledger is validated; a skip list that tells whether it is on the
    /// kept anchor's chain is asked for.
    Ancestry {
        anchor: Anchor,
        recent: Option<SkipList>,
        lists: ProvenLists,
    },
}

/// Where an open query is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// Nothing asked yet.
    Start,
    /// A state proof at the ledger of `at` is asked for.
    State { at: [u8; 32] },
    /// The transaction's branch is asked for.
    Branch,
    /// Its bytes are, or are asked again at `due_ms`, `tries` times so far.
    Blobs {
        proof: TxProof,
        tries: u32,
        due_ms: Option<u64>,
    },
    /// Proven, in a ledger not yet proven on the anchor's chain.
    Proven(Box<ProvenTx>),
}

/// A query open.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Open {
    query: Query,
    step: Step,
}

/// The verifier.
pub struct Verifier {
    /// Whether its network resets.
    resets: bool,
    /// Whether its publishers are the owner's, not its network's: then its
    /// UNL is not the one the server follows.
    own_publishers: bool,
    config: UnlConfig,
    unl: Unl,
    fetches: ListFetches,
    signatures: Box<dyn ValidationSignatures + Send>,
    anchor: Option<KeptAnchor>,
    /// Counted up whenever what it keeps changes.
    changes: u64,
    round: Option<Round>,
    /// A round is to start as soon as it may.
    round_due: bool,
    /// When the last round started.
    round_ms: Option<u64>,
    /// Where in the UNL's validators that did not count the next round's
    /// questions for newer manifests start.
    manifests_from: usize,
    /// Skip lists proven, by the ledger they were proven at and their key.
    lists: ProvenLists,
    /// Skip lists the server said it does not keep.
    missing: BTreeSet<([u8; 32], [u8; 32])>,
    queries: BTreeMap<QueryId, Open>,
    next_query: QueryId,
    /// Whether a query waits for an anchor, or a later one.
    needs_anchor: bool,
    /// What is asked of the server, with when.
    pending: BTreeMap<Ask, u64>,
    connected: bool,
    /// Until when it waits before it asks for a new connection; 0 while it
    /// does not.
    stalled_until_ms: u64,
    waiting: Option<Waiting>,
    /// It stopped: the UNL validated two chains of a network that never
    /// resets.
    failed: bool,
    doubts: BTreeMap<Doubt, u64>,
    counted: Proofs,
    /// The objects whose threads a proven transaction is read for.
    threaded: Vec<[u8; 32]>,
    now_ms: u64,
    out: VecDeque<Out>,
}

impl core::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Verifier")
            .field("anchor", &self.anchor)
            .field("queries", &self.queries.len())
            .field("waiting", &self.waiting)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

/// Building, keeping and reading it.
impl Verifier {
    /// A verifier of the XRP Ledger network `network_id`, trusting the
    /// validators the publishers of `config` list, whose validations
    /// `signatures` checks: with no list and no anchor yet.
    #[must_use]
    pub fn new(
        network_id: u32,
        config: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        let own_publishers = UnlConfig::for_network(network_id).as_ref() != Some(&config);
        Self {
            resets: crate::account::resets(network_id),
            own_publishers,
            unl: Unl::new(config.clone()),
            fetches: ListFetches::new(config.publishers.len()),
            config,
            signatures,
            anchor: None,
            changes: 0,
            round: None,
            round_due: false,
            round_ms: None,
            manifests_from: 0,
            lists: ProvenLists::new(),
            missing: BTreeSet::new(),
            queries: BTreeMap::new(),
            next_query: 1,
            needs_anchor: false,
            pending: BTreeMap::new(),
            connected: false,
            stalled_until_ms: 0,
            waiting: None,
            failed: false,
            doubts: BTreeMap::new(),
            counted: Proofs::default(),
            threaded: Vec::new(),
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// What it keeps, to restore after a restart: its UNL's lists and
    /// manifests and whether it is blocked, and its anchor, so that a
    /// restart neither lifts a block nor takes an older anchor as current.
    /// It is written as the account's state is: a version, then the
    /// anchor, then the UNL (spec/areas/xrpl.md#state-and-restarts).
    #[must_use]
    pub fn state(&self) -> Vec<u8> {
        let mut out = kept::Out::state(kept::ACCOUNT_VERSION);
        out.option(self.anchor.as_ref(), |out, anchor| anchor.write(out));
        self.unl.write(&mut out);
        out.0
    }

    /// A verifier as [`Self::new`] makes it, with what [`Self::state`]
    /// kept, verified again at `now_ms`; `None` for a state it never
    /// writes.
    #[must_use]
    pub fn restore(
        network_id: u32,
        config: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
        state: &[u8],
        now_ms: u64,
    ) -> Option<Self> {
        let mut input = kept::In::state(state, kept::ACCOUNT_VERSION)?;
        let anchor = input.option(KeptAnchor::read)?;
        let unl = Unl::read(config.clone(), &mut input, now_ms)?;
        input.finish()?;
        let mut verifier = Self::new(network_id, config, signatures);
        verifier.unl = unl;
        verifier.anchor = anchor;
        verifier.now_ms = now_ms;
        Some(verifier)
    }

    /// Reads each transaction it proves from now on for the transaction
    /// before it that changed each object of `keys`, at most
    /// [`MAX_PROOF_KEYS`] ([`records::Decoded::threads`](crate::records::Decoded)),
    /// so that its holder walks those objects' history back from their
    /// proven state, as the account walks its own
    /// (spec/areas/xrpl.md#following-the-account).
    pub fn thread(&mut self, keys: Vec<[u8; 32]>) {
        assert!(keys.len() <= MAX_PROOF_KEYS, "one state proof's worth");
        self.threaded = keys;
    }

    /// Counted up whenever what [`Self::state`] keeps changes, so that its
    /// holder knows to write it.
    #[must_use]
    pub const fn changes(&self) -> u64 {
        self.changes
    }

    /// The newest anchor.
    #[must_use]
    pub fn anchor(&self) -> Option<Anchor> {
        self.anchor.map(|kept| kept.anchor)
    }

    /// The UNL, from the lists.
    #[must_use]
    pub const fn unl(&self) -> &Unl {
        &self.unl
    }

    /// Why it waits, while it does.
    #[must_use]
    pub const fn waiting(&self) -> Option<Waiting> {
        self.waiting
    }

    /// Whether it stopped: the UNL validated two chains of a network that
    /// never resets.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// The lies it caught, by kind.
    #[must_use]
    pub const fn doubts(&self) -> &BTreeMap<Doubt, u64> {
        &self.doubts
    }

    /// What it proved.
    #[must_use]
    pub const fn proofs(&self) -> Proofs {
        self.counted
    }

    /// How many queries are open.
    #[must_use]
    pub fn open_queries(&self) -> usize {
        self.queries.len()
    }

    /// What it has its holder do, and tells it, in order. Its holder takes
    /// them after each input.
    pub fn take_out(&mut self) -> Vec<Out> {
        self.out.drain(..).collect()
    }

    /// When it next has something to do on its own: a fetch due or late, an
    /// ask unanswered too long, a transaction's bytes to ask again, a new
    /// connection to ask for, or a round while a query waits for one;
    /// `None` once it stopped, or while it is disconnected.
    #[must_use]
    pub fn deadline(&self) -> Option<u64> {
        if self.failed {
            return None;
        }
        let fetch = self.connected.then(|| self.fetches.deadline()).flatten();
        let request =
            (self.pending.values().min()).map(|sent| sent.saturating_add(REQUEST_TIMEOUT_MS));
        let stall = (self.stalled_until_ms > 0).then_some(self.stalled_until_ms);
        let blobs = (self.queries.values()).filter_map(|open| match open.step {
            Step::Blobs { due_ms, .. } => due_ms,
            _ => None,
        });
        // A round starts only while it asks: stalled or disconnected, a
        // query that waits for one waits for the connection.
        let round = (self.active() && self.needs_anchor && self.round.is_none())
            .then(|| (self.round_ms).map_or(self.now_ms, |at| at.saturating_add(ANCHOR_EVERY_MS)));
        [fetch, request, stall, round]
            .into_iter()
            .flatten()
            .chain(blobs)
            .min()
    }
}

/// Inputs.
impl Verifier {
    /// A connection to its server is made: anything asked on an earlier
    /// one is forgotten, and its answers are not to be handed back.
    pub fn connected(&mut self, now_ms: u64) {
        self.now(now_ms);
        self.disconnected();
        self.connected = true;
        self.round_due = true;
        self.dispatch();
    }

    /// The connection went: what was asked on it is forgotten, and asked
    /// again on the next.
    pub fn disconnected(&mut self) {
        self.connected = false;
        self.pending.clear();
        self.round = None;
        self.fetches.forget();
        self.lists.clear();
        self.missing.clear();
        self.stalled_until_ms = 0;
        self.restart_queries();
    }

    /// Time passed: an ask unanswered too long, or a stall past, asks for a
    /// new connection; fetches that took too long fail, lists expire or
    /// take effect, a transaction's bytes are asked again, and a round
    /// starts while a query waits for one.
    pub fn tick(&mut self, now_ms: u64) {
        self.now(now_ms);
        let late = (self.pending.values())
            .any(|sent| sent.saturating_add(REQUEST_TIMEOUT_MS) <= self.now_ms);
        // A connection that leaves an ask unanswered may be half open, or
        // its server drops what it is asked: made anew.
        if late {
            self.waiting = Some(Waiting::NotAnswered);
            return self.reconnect();
        }
        if self.stalled_until_ms > 0 && self.stalled_until_ms <= self.now_ms {
            return self.reconnect();
        }
        for publisher in self.fetches.late(self.now_ms) {
            if let Some(url) = self.fetches.site(&self.config, publisher) {
                self.list_answered(&url, None);
            }
        }
        self.tick_lists();
        self.dispatch();
    }

    /// A hint that a ledger closed: a round, where none is in flight.
    pub fn anchor_now(&mut self, now_ms: u64) {
        self.now(now_ms);
        self.round_due = true;
        self.dispatch();
    }

    /// Asks `query`; its verdict comes as [`Out::Verdict`] under the id
    /// returned.
    ///
    /// # Errors
    ///
    /// Why it is not taken.
    pub fn query(&mut self, query: Query, now_ms: u64) -> Result<QueryId, QueryError> {
        self.now(now_ms);
        if let Query::State { keys, .. } = &query {
            let distinct: BTreeSet<&[u8; 32]> = keys.iter().collect();
            if keys.is_empty() || keys.len() > MAX_PROOF_KEYS || distinct.len() != keys.len() {
                return Err(QueryError::Keys);
            }
        }
        if self.queries.len() >= MAX_QUERIES {
            return Err(QueryError::Full);
        }
        let id = self.next_query;
        self.next_query = self.next_query.wrapping_add(1).max(1);
        assert!(!self.queries.contains_key(&id), "a query's id is its own");
        self.queries.insert(
            id,
            Open {
                query,
                step: Step::Start,
            },
        );
        self.dispatch();
        Ok(id)
    }

    /// Closes query `id` without a verdict. What it asked goes with it,
    /// unless another open query asked the same; its answer, when it comes,
    /// is dropped.
    pub fn cancel(&mut self, id: QueryId) {
        let Some(open) = self.queries.remove(&id) else {
            return;
        };
        if let Some(ask) = own_ask(&open)
            && !(self.queries.values()).any(|other| own_ask(other).as_ref() == Some(&ask))
        {
            self.pending.remove(&ask);
        }
    }

    /// The server answered `ask` with `outcome`. An answer to what is not
    /// asked on this connection is dropped.
    pub fn answered(&mut self, ask: &Ask, outcome: Result<Value, ApiError>, now_ms: u64) {
        self.now(now_ms);
        if self.pending.remove(ask).is_none() {
            return;
        }
        match ask {
            Ask::Validated => self.proved_anchor(outcome),
            Ask::Validations { .. } => self.validated_anchor(outcome),
            Ask::Manifest(_) => self.manifest_read(outcome),
            Ask::SkipList { at, key } => self.skip_list_read(*at, *key, outcome),
            Ask::State { at, keys } => self.state_read(*at, keys, outcome),
            Ask::TxProof(hash) => self.branch_read(*hash, outcome),
            Ask::TxBlobs(hash) => self.blobs_read(*hash, outcome),
            Ask::Fetch(_) => {}
        }
        self.dispatch();
    }

    /// A fetch of `url` answered with `body`, or failed.
    pub fn fetched(&mut self, url: &str, body: Option<&[u8]>, now_ms: u64) {
        self.now(now_ms);
        self.list_answered(url, body);
        self.dispatch();
    }
}

/// Lists, rounds and anchors.
impl Verifier {
    const fn now(&mut self, now_ms: u64) {
        if now_ms > self.now_ms {
            self.now_ms = now_ms;
        }
    }

    fn push(&mut self, out: Out) {
        self.out.push_back(out);
        assert!(self.out.len() <= MAX_OUT, "its outputs are taken");
    }

    fn ask(&mut self, ask: Ask) {
        if !self.connected || self.pending.contains_key(&ask) {
            return;
        }
        self.pending.insert(ask.clone(), self.now_ms);
        assert!(
            self.pending.len() <= MAX_PENDING,
            "what is asked is bounded"
        );
        self.push(Out::Ask(ask));
    }

    /// Whether it asks anything now.
    const fn active(&self) -> bool {
        self.connected && !self.failed && self.stalled_until_ms == 0
    }

    /// Asks its holder for a new connection, and forgets this one.
    fn reconnect(&mut self) {
        self.disconnected();
        self.push(Out::Reconnect);
    }

    /// Asks what is due: each query's next step, a round, and the lists,
    /// which are fetched while it waits after a stall too.
    fn dispatch(&mut self) {
        if self.active() {
            self.advance_queries();
        }
        let rested = self
            .round_ms
            .is_none_or(|at| at.saturating_add(ANCHOR_EVERY_MS) <= self.now_ms);
        if self.active()
            && self.round.is_none()
            && (self.round_due || (self.needs_anchor && rested))
        {
            self.round_due = false;
            self.start_round();
        }
        if self.connected && !self.failed {
            for url in self.fetches.due(&self.config, self.now_ms) {
                self.push(Out::Ask(Ask::Fetch(url)));
            }
        }
    }

    fn list_answered(&mut self, url: &str, body: Option<&[u8]>) {
        let Some(answered) =
            (self.fetches).answered(&mut self.unl, &self.config, (url, body), self.now_ms)
        else {
            return;
        };
        self.changes = self.changes.wrapping_add(answered.changes);
        let ready = self.unl.unl(self.now_ms).is_ok();
        if ready && self.round.is_none() && self.waiting.is_some() {
            self.round_due = true;
        }
    }

    fn tick_lists(&mut self) {
        if lists::tick(&mut self.unl, self.now_ms) {
            self.changes = self.changes.wrapping_add(1);
        }
    }

    /// Starts a round; without a UNL, it waits for its lists.
    fn start_round(&mut self) {
        assert!(self.round.is_none(), "one round at a time");
        self.round_ms = Some(self.now_ms);
        if let Err(error) = self.unl.unl(self.now_ms) {
            self.waiting = Some(Waiting::Unl(error));
            self.tick_lists();
            self.fetches.hasten(self.now_ms);
            return;
        }
        self.round = Some(Round::Proving);
        self.ask(Ask::Validated);
    }

    /// The round's state proof at the server's validated ledger.
    fn proved_anchor(&mut self, outcome: Result<Value, ApiError>) {
        if self.round != Some(Round::Proving) {
            return;
        }
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = match prove_state(&answer, &[recent_key()], None) {
            Ok(proven) => proven,
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        // Refused before any signature is checked.
        let new = Anchor::of(&proven.header);
        if self
            .anchor
            .is_some_and(|kept| judge(&kept.anchor, &new) == Judged::Older)
        {
            return self.wait_for_newer();
        }
        self.ask_validations(proven, self.own_publishers);
    }

    fn ask_validations(&mut self, proven: ProvenState, untrusted: bool) {
        let hash = proven.hash;
        self.round = Some(Round::Validating { proven, untrusted });
        self.ask(Ask::Validations { hash, untrusted });
    }

    /// The round's validations.
    fn validated_anchor(&mut self, outcome: Result<Value, ApiError>) {
        let Some(Round::Validating { proven, untrusted }) = self.round.clone() else {
            return;
        };
        let Some(validations) = self.answer(outcome, api::validations_result) else {
            return;
        };
        self.tally(proven, validations, true, untrusted);
    }

    /// Counts the UNL's validations of the proven ledger, as the account
    /// counts them (spec/areas/xrpl.md#anchors): an anchor, the others
    /// asked for where only the server's came, newer manifests asked for
    /// those that did not count, once a round, or a wait.
    fn tally(&mut self, proven: ProvenState, validations: Validations, ask: bool, untrusted: bool) {
        self.tick_lists();
        let kept = self.anchor.map(|kept| kept.anchor);
        let anchored = self.unl.anchor(
            &proven.header,
            validations.validations.iter().map(Vec::as_slice),
            self.now_ms,
            kept.as_ref(),
            self.signatures.as_ref(),
        );
        match anchored {
            Ok(anchored) => self.anchored(&proven, anchored.anchor),
            Err(AnchorError::Older) => self.wait_for_newer(),
            Err(AnchorError::TooMany) => self.doubted(Doubt::TooMany),
            Err(AnchorError::Unl(error)) => {
                self.round = None;
                self.waiting = Some(Waiting::Unl(error));
                self.fetches.hasten(self.now_ms);
            }
            Err(AnchorError::NoQuorum(tally))
                if tally.stale > 0 && tally.counted.saturating_add(tally.stale) >= tally.quorum =>
            {
                self.count(Doubt::Older);
                self.round = None;
                self.waiting = Some(Waiting::NotFresh);
            }
            Err(AnchorError::NoQuorum(_)) if !untrusted => self.ask_validations(proven, true),
            Err(AnchorError::NoQuorum(tally))
                if ask && tally.unknown > 0 && !tally.missing.is_empty() =>
            {
                self.fetches.hasten(self.now_ms);
                let count = tally.missing.len().min(MAX_MANIFEST_ASKS);
                let from = self.manifests_from % tally.missing.len();
                self.manifests_from = self.manifests_from.wrapping_add(count);
                let asked: Vec<PublicKey> = (tally.missing.iter().cycle())
                    .skip(from)
                    .take(count)
                    .copied()
                    .collect();
                self.round = Some(Round::Manifests {
                    proven,
                    validations,
                    asked: asked.len(),
                });
                for master in asked {
                    self.ask(Ask::Manifest(master));
                }
            }
            Err(AnchorError::NoQuorum(_)) if validations.truncated => {
                self.fetches.hasten(self.now_ms);
                self.stall(Waiting::Cut);
            }
            Err(AnchorError::NoQuorum(_)) => {
                self.fetches.hasten(self.now_ms);
                self.count(Doubt::NoQuorum);
                self.stall(Waiting::NoQuorum);
            }
        }
    }

    /// A newer manifest the server gave, or not; once the last asked
    /// answered, the validations are counted again.
    fn manifest_read(&mut self, outcome: Result<Value, ApiError>) {
        let Some(Round::Manifests { asked, .. }) = &mut self.round else {
            return;
        };
        *asked = asked.saturating_sub(1);
        let done = *asked == 0;
        if let Some(bytes) = outcome.ok().as_ref().and_then(api::manifest_result)
            && self.unl.take_manifest(&bytes) == Ok(true)
        {
            self.changes = self.changes.wrapping_add(1);
        }
        if done
            && let Some(Round::Manifests {
                proven,
                validations,
                ..
            }) = self.round.take()
        {
            self.tally(proven, validations, false, true);
        }
    }

    /// The proven ledger is validated: taken as the anchor, after its
    /// ancestry is proven if it is later than the one kept.
    fn anchored(&mut self, proven: &ProvenState, anchor: Anchor) {
        let recent = match proven.skip_list(&recent_key()) {
            Ok(recent) => recent,
            Err(doubt) => return self.doubted(doubt),
        };
        match self.anchor.map(|kept| judge(&kept.anchor, &anchor)) {
            None | Some(Judged::Take) => self.take_anchor(anchor, recent),
            Some(Judged::Older) => self.wait_for_newer(),
            Some(Judged::Ignore) if self.resets => {
                self.count(Doubt::OlderChain);
                self.round = None;
                self.waiting = Some(Waiting::Older);
            }
            // On a network that never resets the UNL validated no older
            // chain: a ledger off the kept one is a fork.
            Some(Judged::Ignore | Judged::Reset) => self.off_chain(anchor, recent),
            Some(Judged::Later) => {
                let mut lists = ProvenLists::new();
                lists.insert((anchor.hash, recent_key()), recent.clone());
                self.round = Some(Round::Ancestry {
                    anchor,
                    recent,
                    lists,
                });
                self.trace_ancestry();
            }
        }
    }

    /// The next step of telling a later anchor's ancestry.
    fn trace_ancestry(&mut self) {
        let (
            Some(kept),
            Some(Round::Ancestry {
                anchor,
                recent,
                lists,
            }),
        ) = (self.anchor, self.round.clone())
        else {
            return;
        };
        match ancestry(&kept, &anchor, &lists) {
            Ancestry::OnChain => self.take_anchor(anchor, recent),
            Ancestry::OffChain => self.off_chain(anchor, recent),
            Ancestry::Lie => self.doubted(Doubt::SkipList),
            Ancestry::Prove { at, key } => self.ask(Ask::SkipList { at, key }),
        }
    }

    /// A validated ledger the kept anchor's chain does not hold: on a
    /// network that resets, a reset, and the ledger is the anchor; on any
    /// other, the verifier stops.
    fn off_chain(&mut self, anchor: Anchor, recent: Option<SkipList>) {
        self.round = None;
        if !self.resets {
            self.failed = true;
            self.pending.clear();
            return;
        }
        self.keep_anchor(anchor, recent.as_ref());
        self.push(Out::Reset(anchor));
        // Everything is asked anew, on the new chain and a new connection:
        // an answer still on its way on this one may be of the chain gone,
        // and asked the same as on the new one.
        self.reconnect();
        self.keep_list((anchor.hash, recent_key()), recent);
    }

    /// `anchor` is current.
    fn take_anchor(&mut self, anchor: Anchor, recent: Option<SkipList>) {
        self.round = None;
        let newer = self.anchor.is_none_or(|kept| kept.anchor != anchor);
        self.keep_anchor(anchor, recent.as_ref());
        self.counted.anchors = self.counted.anchors.saturating_add(1);
        self.keep_list((anchor.hash, recent_key()), recent);
        if newer {
            self.push(Out::Anchored(anchor));
        }
    }

    fn keep_anchor(&mut self, anchor: Anchor, recent: Option<&SkipList>) {
        let kept = KeptAnchor::of(anchor, recent);
        if self.anchor != Some(kept) {
            self.changes = self.changes.wrapping_add(1);
        }
        self.anchor = Some(kept);
        self.waiting = None;
    }

    /// The server's validated ledger is older than the anchor kept: never
    /// current. The verifier waits for a newer one.
    fn wait_for_newer(&mut self) {
        self.count(Doubt::Older);
        self.round = None;
        self.waiting = Some(Waiting::Older);
    }

    /// What a proof method answered, read by `read`; `None` after a
    /// refusal, which stalls the verifier, or an answer that does not read,
    /// which is a lie.
    fn answer<T>(
        &mut self,
        outcome: Result<Value, ApiError>,
        read: impl FnOnce(&Value) -> Option<T>,
    ) -> Option<T> {
        match outcome {
            Ok(result) => {
                let read = read(&result);
                if read.is_none() {
                    self.doubted(Doubt::Unreadable);
                }
                read
            }
            Err(error) => {
                self.stall(if error.error == "unknownCmd" {
                    Waiting::NoProofs
                } else {
                    Waiting::Refused
                });
                None
            }
        }
    }

    /// A lie caught: counted, and a new connection asked for.
    fn doubted(&mut self, doubt: Doubt) {
        self.count(doubt);
        self.stall(Waiting::NotProven);
    }

    /// The verifier waits, for `why`, and asks for a new connection after
    /// [`STALL_MS`]: nothing asked on this one counts any more.
    fn stall(&mut self, why: Waiting) {
        self.round = None;
        self.pending.clear();
        self.restart_queries();
        self.waiting = Some(why);
        self.stalled_until_ms = self.now_ms.saturating_add(STALL_MS).max(1);
    }

    fn count(&mut self, doubt: Doubt) {
        let count = self.doubts.entry(doubt).or_insert(0);
        *count = count.saturating_add(1);
    }
}

/// Queries.
impl Verifier {
    /// Every open query starts again.
    fn restart_queries(&mut self) {
        for open in self.queries.values_mut() {
            open.step = Step::Start;
        }
    }

    /// Each query's next step, in the order they were asked.
    fn advance_queries(&mut self) {
        self.needs_anchor = false;
        let ids: Vec<QueryId> = self.queries.keys().copied().collect();
        for id in ids {
            if !self.active() {
                return;
            }
            self.advance(id);
        }
    }

    fn advance(&mut self, id: QueryId) {
        let Some(open) = self.queries.get(&id) else {
            return;
        };
        match (&open.query, &open.step) {
            (Query::State { keys, at }, Step::Start) => {
                let (keys, at) = (keys.clone(), *at);
                self.advance_state(id, keys, at);
            }
            (Query::Transaction { hash, .. }, Step::Start) => {
                let hash = *hash;
                self.set_step(id, Step::Branch);
                self.ask(Ask::TxProof(hash));
            }
            (Query::Transaction { hash, .. }, Step::Blobs { due_ms, .. }) => {
                let hash = *hash;
                if due_ms.is_some_and(|due| due <= self.now_ms) {
                    if let Some(Open {
                        step: Step::Blobs { due_ms, .. },
                        ..
                    }) = self.queries.get_mut(&id)
                    {
                        *due_ms = None;
                    }
                    self.ask(Ask::TxBlobs(hash));
                }
            }
            (Query::Transaction { from, .. }, Step::Proven(proven)) => {
                let (from, proven) = (*from, (**proven).clone());
                self.advance_proven(id, from, proven);
            }
            _ => {}
        }
    }

    fn set_step(&mut self, id: QueryId, step: Step) {
        if let Some(open) = self.queries.get_mut(&id) {
            open.step = step;
        }
    }

    fn verdict(&mut self, id: QueryId, verdict: Verdict) {
        if self.queries.remove(&id).is_some() {
            self.push(Out::Verdict(id, verdict));
        }
    }

    /// A state query: the hash of its ledger, from the anchor and its skip
    /// lists, then the state proof there.
    fn advance_state(&mut self, id: QueryId, keys: Vec<[u8; 32]>, at: Option<u32>) {
        let Some(kept) = self.anchor else {
            self.needs_anchor = true;
            return;
        };
        let index = at.unwrap_or(kept.anchor.index);
        match ledger_hash(&kept.anchor, index, &self.lists) {
            LedgerHash::Proven(hash) => {
                self.set_step(id, Step::State { at: hash });
                self.ask(Ask::State { at: hash, keys });
            }
            LedgerHash::None => self.verdict(id, Verdict::NoLedger),
            LedgerHash::Later => self.needs_anchor = true,
            LedgerHash::Lie => self.lied_lists(),
            LedgerHash::Prove { at, key } => self.ask_list(id, at, key),
        }
    }

    /// A proven transaction: in a ledger the anchor's chain holds, or a lie.
    fn advance_proven(&mut self, id: QueryId, from: u32, proven: ProvenTx) {
        let Some(kept) = self.anchor else {
            self.needs_anchor = true;
            return;
        };
        let index = proven.header.sequence;
        match ledger_proof(&kept.anchor, index, &proven.ledger_hash, &self.lists) {
            LedgerProof::Holds => {
                let verdict = if index >= from {
                    Verdict::Applied(proven)
                } else {
                    Verdict::Before(proven)
                };
                self.verdict(id, verdict);
            }
            // A transaction applies in one ledger only: its proof in
            // another the anchor's chain does not hold is a lie.
            LedgerProof::Wrong => self.doubted(Doubt::Ledger),
            LedgerProof::Later => self.needs_anchor = true,
            LedgerProof::Lie => self.lied_lists(),
            LedgerProof::Prove { at, key } => self.ask_list(id, at, key),
        }
    }

    /// Asks for a skip list a query needs, unless the server said it does
    /// not keep it, or as many are asked for as may be.
    fn ask_list(&mut self, id: QueryId, at: [u8; 32], key: [u8; 32]) {
        if self.missing.contains(&(at, key)) {
            return self.verdict(id, Verdict::NoHistory);
        }
        let asking = (self.pending.keys())
            .filter(|ask| matches!(ask, Ask::SkipList { .. }))
            .count();
        let ask = Ask::SkipList { at, key };
        if asking < MAX_LIST_ASKS || self.pending.contains_key(&ask) {
            self.ask(ask);
        }
    }

    /// The lists hold a ledger at another index than its header's: a lie.
    /// No list is kept, so the next connection proves them anew.
    fn lied_lists(&mut self) {
        self.lists.clear();
        self.doubted(Doubt::SkipList);
    }

    fn keep_list(&mut self, at_key: ([u8; 32], [u8; 32]), list: Option<SkipList>) {
        if self.lists.len() >= MAX_LISTS && !self.lists.contains_key(&at_key) {
            self.lists.clear();
        }
        self.lists.insert(at_key, list);
    }

    /// A skip list, for a round's ancestry or a query's ledger.
    fn skip_list_read(&mut self, at: [u8; 32], key: [u8; 32], outcome: Result<Value, ApiError>) {
        // The round's only if its ancestry asks for this very list now: a
        // list only a query asked for is that query's history.
        let for_round = match (&self.anchor, &self.round) {
            (Some(kept), Some(Round::Ancestry { anchor, lists, .. })) => {
                ancestry(kept, anchor, lists) == Ancestry::Prove { at, key }
            }
            _ => false,
        };
        if let Err(error) = &outcome
            && error.error == "lgrNotFound"
        {
            if for_round {
                return self.stall(Waiting::History);
            }
            if self.missing.len() >= MAX_MISSING {
                self.missing.clear();
            }
            self.missing.insert((at, key));
            return;
        }
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = prove_state(&answer, &[key], Some(&at)).map_err(Doubt::of);
        let list = match proven.and_then(|proven| proven.skip_list(&key)) {
            Ok(list) => list,
            Err(doubt) => return self.doubted(doubt),
        };
        self.keep_list((at, key), list.clone());
        if let Some(Round::Ancestry { lists, .. }) = &mut self.round {
            lists.insert((at, key), list);
            self.trace_ancestry();
        }
    }

    /// A state proof of `keys` at the ledger of `at`, for the queries that
    /// asked it.
    fn state_read(&mut self, at: [u8; 32], keys: &[[u8; 32]], outcome: Result<Value, ApiError>) {
        let asked = |open: &Open| {
            open.step == Step::State { at }
                && matches!(&open.query, Query::State { keys: asked, .. } if asked == keys)
        };
        let ids: Vec<QueryId> = (self.queries.iter())
            .filter(|(_, open)| asked(open))
            .map(|(id, _)| *id)
            .collect();
        if let Err(error) = &outcome
            && error.error == "lgrNotFound"
        {
            for id in ids {
                self.verdict(id, Verdict::NoHistory);
            }
            return;
        }
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = match prove_state(&answer, keys, Some(&at)) {
            Ok(proven) => proven,
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        // The ledger's index is its header's: one the lists gave another
        // index is a lie.
        let elsewhere = ids.iter().any(|id| {
            matches!(
                self.queries.get(id),
                Some(Open { query: Query::State { at: Some(index), .. }, .. })
                    if *index != proven.header.sequence
            )
        });
        if elsewhere {
            return self.doubted(Doubt::Header);
        }
        self.counted.states = self.counted.states.saturating_add(1);
        let absences = u64::try_from(proven.absences()).unwrap_or(u64::MAX);
        self.counted.absences = self.counted.absences.saturating_add(absences);
        for id in ids {
            self.verdict(id, Verdict::State(proven.clone()));
        }
    }

    /// The queries of transaction `hash` at `step`.
    fn of_transaction(&self, hash: [u8; 32], step: impl Fn(&Step) -> bool) -> Vec<QueryId> {
        (self.queries.iter())
            .filter(|(_, open)| {
                matches!(open.query, Query::Transaction { hash: of, .. } if of == hash)
                    && step(&open.step)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// The branch of transaction `hash`: its bytes are asked for next, to
    /// build the leaf it starts above. A server that finds no such
    /// transaction proves nothing.
    fn branch_read(&mut self, hash: [u8; 32], outcome: Result<Value, ApiError>) {
        let ids = self.of_transaction(hash, |step| *step == Step::Branch);
        if let Err(error) = &outcome
            && matches!(error.error.as_str(), "txnNotFound" | "lgrNotFound")
        {
            for id in ids {
                self.verdict(id, Verdict::NotFound);
            }
            return;
        }
        let Some(proof) = self.answer(outcome, api::tx_proof_result) else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.set_step(
                id,
                Step::Blobs {
                    proof: proof.clone(),
                    tries: 0,
                    due_ms: None,
                },
            );
        }
        self.ask(Ask::TxBlobs(hash));
    }

    /// The bytes and metadata of transaction `hash`, whose branch came: the
    /// leaf built from them, with the branch, proves them, or the server
    /// lied. A server that has not saved them with their ledger yet is
    /// asked again a while later, a bounded number of times
    /// ([`MAX_UNSAVED_TRIES`]), as the account asks.
    fn blobs_read(&mut self, hash: [u8; 32], outcome: Result<Value, ApiError>) {
        let ids = self.of_transaction(hash, |step| {
            matches!(step, Step::Blobs { due_ms: None, .. })
        });
        let not_yet = match &outcome {
            Ok(result) => api::tx_not_yet(result),
            Err(error) => matches!(error.error.as_str(), "txnNotFound" | "lgrNotFound"),
        };
        if not_yet {
            return self.unsaved_yet(&ids);
        }
        let Some(blobs) = self.answer(outcome, api::tx_binary_result) else {
            return;
        };
        let Some(Open {
            step: Step::Blobs { proof, .. },
            ..
        }) = ids.first().and_then(|id| self.queries.get(id))
        else {
            return;
        };
        let proven = match prove_transaction(proof, &blobs, &hash, &self.threaded) {
            Ok(proven) => proven,
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        self.counted.transactions = self.counted.transactions.saturating_add(1);
        for id in ids {
            self.set_step(id, Step::Proven(Box::new(proven.clone())));
        }
    }

    /// The server has not saved a transaction with its ledger yet: its
    /// bytes are asked again a while later, or, past the bound, the server
    /// is taken as refusing them.
    fn unsaved_yet(&mut self, ids: &[QueryId]) {
        let due = self.now_ms.saturating_add(UNSAVED_RETRY_MS);
        for id in ids {
            let Some(Open {
                step: Step::Blobs { tries, due_ms, .. },
                ..
            }) = self.queries.get_mut(id)
            else {
                continue;
            };
            if *tries >= MAX_UNSAVED_TRIES {
                return self.stall(Waiting::Refused);
            }
            *tries = tries.saturating_add(1);
            *due_ms = Some(due);
        }
    }
}

/// Invariants.
impl Verifier {
    /// Asserts what holds of it.
    pub fn check_invariants(&self) {
        assert!(self.queries.len() <= MAX_QUERIES);
        assert!(self.pending.len() <= MAX_PENDING);
        assert!(self.lists.len() <= MAX_LISTS);
        assert!(self.missing.len() <= MAX_MISSING);
        assert!(self.out.len() <= MAX_OUT);
        assert!(
            !self.queries.contains_key(&0) && self.next_query != 0,
            "query ids start at 1"
        );
        self.fetches.check_invariants(&self.config);
        assert_eq!(self.unl.config(), &self.config, "the config's UNL");
        if let Some(kept) = &self.anchor {
            kept.check_invariants();
        }
        if !self.connected {
            assert!(self.pending.is_empty(), "nothing asked while disconnected");
            assert!(self.round.is_none(), "a round is a connection's");
            assert!(!self.fetches.any_asked(), "a fetch is a connection's");
            assert_eq!(self.stalled_until_ms, 0, "a stall is a connection's");
        }
        if self.stalled_until_ms > 0 || self.failed {
            assert!(self.round.is_none(), "no round while stalled or failed");
            assert!(self.pending.is_empty(), "nothing asked while stalled");
        }
        for (ask, sent) in &self.pending {
            assert!(*sent <= self.now_ms, "asked in the past");
            if matches!(ask, Ask::State { .. } | Ask::TxProof(_) | Ask::TxBlobs(_)) {
                assert!(
                    (self.queries.values()).any(|open| own_ask(open).as_ref() == Some(ask)),
                    "a query's ask is an open query's"
                );
            }
        }
        if matches!(self.round, Some(Round::Ancestry { .. })) {
            assert!(self.anchor.is_some(), "ancestry is of a kept anchor");
        }
        for open in self.queries.values() {
            if let Query::State { keys, .. } = &open.query {
                assert!(!keys.is_empty() && keys.len() <= MAX_PROOF_KEYS);
            }
            let fits = matches!(
                (&open.query, &open.step),
                (_, Step::Start)
                    | (Query::State { .. }, Step::State { .. })
                    | (
                        Query::Transaction { .. },
                        Step::Branch | Step::Blobs { .. } | Step::Proven(_)
                    )
            );
            assert!(fits, "a query's step is of its kind");
            if let Step::Blobs { tries, .. } = open.step {
                assert!(tries <= MAX_UNSAVED_TRIES);
            }
        }
    }
}

/// What an open query waits for an answer to, if anything but a skip list,
/// of which it asks at most [`MAX_LIST_ASKS`] with the others.
fn own_ask(open: &Open) -> Option<Ask> {
    match (&open.query, &open.step) {
        (Query::State { keys, .. }, Step::State { at }) => Some(Ask::State {
            at: *at,
            keys: keys.clone(),
        }),
        (Query::Transaction { hash, .. }, Step::Branch) => Some(Ask::TxProof(*hash)),
        (Query::Transaction { hash, .. }, Step::Blobs { due_ms: None, .. }) => {
            Some(Ask::TxBlobs(*hash))
        }
        _ => None,
    }
}
