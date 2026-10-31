//! What following a hub's escrow holds of its connection and its network
//! rather than of the escrow: the WebSocket's requests and their ids, the
//! verifier and its proof requests, where the server stands, and what the
//! host keeps of the verifier across a restart. One [`Link`] is meant to
//! serve every escrow on its network; the escrow's own part is
//! [`EscrowWatcher`](super::watch::EscrowWatcher), which takes the link
//! along on each call.
//!
//! The verifier answers some queries at once, in the call that asked: an
//! escrow's drain handles its own verdicts and anchors as they come, as
//! what queried relies on, and parks those that are another's
//! ([`Parked`]), for its host to hand on once the call returned.

use std::collections::{BTreeMap, VecDeque};

use dassie_xrpl::api::{self, ApiError};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{self, ANCHOR_EVERY_MS, Ask, QueryId, Verdict, Verifier};
use serde_json::Value;

use super::{Chain, KEEP_VERIFIER_EVERY_MS, Kept, Out, RETRY_MS, Unrestored, chain as judge_chain};
use crate::store::{EscrowCursor, MAX_HUB_VERIFIER_BYTES};

/// The most requests of its own waiting for an answer.
const MAX_PENDING: usize = 16;

/// The most of them that submit an escrow transaction: the rest are kept
/// for following the escrow.
const MAX_PENDING_SUBMITS: usize = MAX_PENDING / 2;

/// The most proof requests the verifier has waiting on a connection: the
/// verifier's own bound, with room for those of a round it gave up on.
const MAX_PROOF_ASKS: usize = 4 * verifier::MAX_QUERIES;

/// What a request of its own was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asked {
    ServerState,
    Subscribe,
    History,
    Submit,
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
    /// Its server is on the hub's network, and on the chain followed, as
    /// far as it says: the escrow is subscribed to.
    Checked,
}

/// What the verifier decided that an escrow's drain met and did not take,
/// or not alone: for the link's host to hand on once the call returned.
/// Handed on within the same call, as the verifier hands its verdicts:
/// boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Parked {
    /// A verdict on a query that is not the draining escrow's.
    Verdict(QueryId, Verdict),
    /// A newest anchor, `true` if a reset, which the draining escrow took
    /// at once: the others take it after.
    Anchored(Anchor, bool),
}

/// One connection to an XRPL server, and the verifier that proves what it
/// answers, for the hub's network.
#[derive(Debug)]
pub(super) struct Link {
    network: u32,
    phase: Phase,
    next_id: u64,
    pending: BTreeMap<u64, Asked>,
    /// The verifier's asks on this connection, by their request's id.
    proof_asks: BTreeMap<u64, Ask>,
    verifier: Verifier,
    /// Whether it said the verifier stopped.
    failed_told: bool,
    /// When the verifier was last asked to anchor on the link's own.
    anchor_asked_ms: Option<u64>,
    /// Whether its verifier's anchor was taken back from before a restart,
    /// and none proven since: no hint is proven against it, as it may be of
    /// a chain gone while the validator was down.
    taken_back: bool,
    /// The verifier's count of changes when its state was last handed to
    /// the host, or taken back.
    kept_changes: u64,
    /// When it was last handed to the host.
    kept_ms: Option<u64>,
    /// What to write of the verifier with the cursor, until the host takes
    /// it.
    keeping: Option<Kept>,
    /// Whether its state was last too large to keep, and what the host
    /// kept before dropped.
    over_bound: bool,
    /// What escrows' drains parked, empty between calls.
    parked: VecDeque<Parked>,
    now_ms: u64,
    out: VecDeque<Out>,
}

/// Building and reading it.
impl Link {
    /// A link on `network`, trusting the validators the publishers of
    /// `unl` list, whose validations `signatures` checks.
    pub(super) fn new(
        network: u32,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            network,
            phase: Phase::Disconnected,
            next_id: 1,
            pending: BTreeMap::new(),
            proof_asks: BTreeMap::new(),
            verifier: Verifier::new(network, unl, signatures),
            failed_told: false,
            anchor_asked_ms: None,
            taken_back: false,
            kept_changes: 0,
            kept_ms: None,
            keeping: None,
            over_bound: false,
            parked: VecDeque::new(),
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// Takes back at `now_ms` what it handed its host of its verifier
    /// before a restart, verified again at `now_ms`, whose validations
    /// `signatures` checks ([`super::Watcher::restore`]).
    pub(super) fn restore(
        &mut self,
        kept: &[u8],
        signatures: Box<dyn ValidationSignatures + Send>,
        now_ms: u64,
    ) -> Result<(), Unrestored> {
        let (network, rest) = kept.split_first_chunk::<4>().ok_or(Unrestored::Unread)?;
        let network = u32::from_be_bytes(*network);
        if network != self.network {
            return Err(Unrestored::Network(network));
        }
        let (publishers, state) = rest.split_first_chunk::<32>().ok_or(Unrestored::Unread)?;
        let config = self.verifier.unl().config().clone();
        if *publishers != publishers_digest(&config) {
            return Err(Unrestored::Publishers);
        }
        let verifier = Verifier::restore(self.network, config, signatures, state, now_ms)
            .ok_or(Unrestored::Unread)?;
        self.verifier = verifier;
        self.now(now_ms);
        self.taken_back = self.verifier.anchor().is_some();
        self.kept_changes = self.verifier.changes();
        Ok(())
    }

    /// What to write of the verifier with the cursor just taken, if
    /// anything.
    pub(super) const fn take_kept(&mut self) -> Option<Kept> {
        self.keeping.take()
    }

    pub(super) const fn network(&self) -> u32 {
        self.network
    }

    pub(super) const fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub(super) const fn now(&mut self, now_ms: u64) {
        if now_ms > self.now_ms {
            self.now_ms = now_ms;
        }
    }

    /// Whether a connection is up.
    pub(super) fn is_connected(&self) -> bool {
        self.phase != Phase::Disconnected
    }

    /// Whether its server's network and chain were checked, and escrows
    /// subscribe on it.
    pub(super) fn checked(&self) -> bool {
        self.phase == Phase::Checked
    }

    pub(super) const fn verifier(&self) -> &Verifier {
        &self.verifier
    }

    pub(super) const fn verifier_mut(&mut self) -> &mut Verifier {
        &mut self.verifier
    }

    /// Whether its verifier's anchor was taken back, and none proven since.
    pub(super) const fn taken_back(&self) -> bool {
        self.taken_back
    }

    /// The verifier proved an anchor with fresh signatures, on the chain it
    /// is on: the chain lives, whichever the anchor taken back was on.
    pub(super) const fn anchor_proven(&mut self) {
        self.taken_back = false;
    }

    /// Hands `out` to the host, after what it was handed before.
    pub(super) fn push(&mut self, out: Out) {
        self.out.push_back(out);
    }

    /// The next thing to do.
    pub(super) fn poll(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    /// Parks what the draining escrow did not take, or not alone.
    pub(super) fn park(&mut self, parked: Parked) {
        self.parked.push_back(parked);
    }

    /// The next thing parked, to hand on.
    pub(super) fn unpark(&mut self) -> Option<Parked> {
        self.parked.pop_front()
    }

    /// Asserts what holds of it.
    pub(super) fn check_invariants(&self) {
        self.verifier.check_invariants();
        assert!(self.pending.len() <= MAX_PENDING, "requests are bounded");
        assert!(
            self.proof_asks.len() <= MAX_PROOF_ASKS,
            "proof asks are bounded"
        );
        assert!(
            !matches!(&self.keeping, Some(Kept::State(kept)) if kept.len() > MAX_HUB_VERIFIER_BYTES),
            "the verifier's state handed over is bounded"
        );
        assert!(self.pending_submits() <= MAX_PENDING_SUBMITS);
        assert!(self.parked.is_empty(), "what is parked is handed on");
        if self.phase == Phase::Disconnected {
            assert!(self.pending.is_empty() && self.proof_asks.is_empty());
        }
    }
}

/// The connection.
impl Link {
    /// A connection is up: ask where the server stands, and have the
    /// verifier anchor on it.
    pub(super) fn connected(&mut self) {
        self.forget_connection();
        self.phase = Phase::Checking;
        self.ask(Asked::ServerState, api::server_state);
        self.verifier.connected(self.now_ms);
    }

    /// The connection is gone. What it decided for the host stays to be
    /// carried out, but for what was to be sent on the connection and a
    /// request to end it.
    pub(super) fn disconnected(&mut self) {
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

    /// What its server said goes with a reset of the chain: it is asked
    /// where it stands again, if connected.
    pub(super) fn check_again(&mut self) {
        self.forget_connection();
        self.phase = Phase::Checking;
        self.ask(Asked::ServerState, api::server_state);
    }

    /// What was asked on the connection is forgotten. The verifier asks its
    /// open queries again on the next.
    fn forget_connection(&mut self) {
        self.pending.clear();
        self.proof_asks.clear();
    }

    /// Waits, and asks the server where it stands again later.
    pub(super) const fn wait(&mut self) {
        self.phase = Phase::Waiting {
            since_ms: self.now_ms,
        };
    }

    /// Does what is due: ask a server that was behind again, and have the
    /// verifier anchor while the anchor, proven at `anchored_ms`, ages.
    pub(super) fn tick(&mut self, anchored_ms: Option<u64>) {
        self.verifier.tick(self.now_ms);
        if let Phase::Waiting { since_ms } = self.phase
            && self.now_ms.saturating_sub(since_ms) >= RETRY_MS
        {
            self.phase = Phase::Checking;
            self.ask(Asked::ServerState, api::server_state);
        }
        let aging = anchored_ms.is_none_or(|at| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        let rested = self
            .anchor_asked_ms
            .is_none_or(|at| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        if self.phase != Phase::Disconnected && aging && rested {
            self.anchor_now();
        }
    }

    /// Has the verifier anchor now, on the link's own.
    pub(super) fn anchor_now(&mut self) {
        self.anchor_asked_ms = Some(self.now_ms);
        self.verifier.anchor_now(self.now_ms);
    }

    /// An answer to the request `id`: the verifier's own it takes itself;
    /// another of its own it hands back, with what it was.
    pub(super) fn response(
        &mut self,
        id: u64,
        outcome: Result<Value, ApiError>,
    ) -> Option<(Asked, Result<Value, ApiError>)> {
        if let Some(ask) = self.proof_asks.remove(&id) {
            self.verifier.answered(&ask, outcome, self.now_ms);
            return None;
        }
        let asked = self.pending.remove(&id)?;
        Some((asked, outcome))
    }

    /// Where the server stands, by `result`: on the hub's network, and on
    /// the chain followed, as far as it says, against `newest`, the newest
    /// ledger proven and when it closed. Its validated ledger, once both
    /// hold; none if it is to be waited for. A new chain is only a hint for
    /// the verifier to anchor, which proves it or not.
    pub(super) fn server_state(
        &mut self,
        result: &Value,
        newest: Option<(u32, u64)>,
    ) -> Option<u32> {
        let state = api::server_state_result(result)?;
        if state.network_id != Some(self.network) {
            self.out.push_back(Out::Problem(
                "the XRPL server is on another network than the hub's escrow",
            ));
            return None;
        }
        match judge_chain(
            newest,
            self.network,
            state.validated_ledger,
            state.close_time,
        ) {
            Chain::Current => {}
            Chain::Behind | Chain::Restarted => {
                self.verifier.anchor_now(self.now_ms);
                return None;
            }
            Chain::WentBack => {
                self.out.push_back(Out::Problem(
                    "the XRPL server's ledger went back on a network that is never reset",
                ));
                return None;
            }
        }
        self.phase = Phase::Checked;
        Some(state.validated_ledger)
    }
}

/// Requests.
impl Link {
    /// Asks the server, unless its requests are full: whether it did.
    pub(super) fn ask(&mut self, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        let id = self.fresh_id();
        self.pending.insert(id, asked);
        self.out.push_back(Out::Send(request(id)));
        true
    }

    /// Whether its requests have room for one more that submits an escrow
    /// transaction.
    pub(super) fn room_to_submit(&self) -> bool {
        self.pending_submits() < MAX_PENDING_SUBMITS
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

    /// Carries out what the verifier decided that is the link's: an ask,
    /// on the connection or of a list publisher's site, or a new
    /// connection. A verdict or an anchor is parked: no escrow's drain took
    /// it.
    pub(super) fn carry_out(&mut self, out: verifier::Out) {
        match out {
            verifier::Out::Ask(Ask::Fetch(url)) => self.out.push_back(Out::Fetch(url)),
            verifier::Out::Ask(ask) => self.ask_proof(ask),
            verifier::Out::Verdict(id, verdict) => self.park(Parked::Verdict(id, verdict)),
            verifier::Out::Anchored(anchor) => self.park(Parked::Anchored(anchor, false)),
            verifier::Out::Reset(anchor) => self.park(Parked::Anchored(anchor, true)),
            verifier::Out::Reconnect => {
                self.out.push_back(Out::Problem(
                    "the XRPL server's answers did not prove, or it refused or left them: connecting anew",
                ));
                self.out.push_back(Out::Reconnect);
            }
        }
    }

    /// Says once that the verifier stopped, once it did.
    pub(super) fn tell_failed(&mut self) {
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
}

/// Keeping the verifier's state across a restart.
impl Link {
    /// Hands the verifier's state to the host with the cursor once it
    /// changed, and [`KEEP_VERIFIER_EVERY_MS`] passed since it last did, or
    /// at once if `now`: with a cursor going out already, or with
    /// `persisted`, the one persisted last. Before a cursor is, there is
    /// nothing to keep it with, and a restart starts as the first start
    /// did.
    pub(super) fn keep(&mut self, now: bool, persisted: Option<EscrowCursor>) {
        let changes = self.verifier.changes();
        let due = now
            || (self.kept_ms)
                .is_none_or(|at| self.now_ms.saturating_sub(at) >= KEEP_VERIFIER_EVERY_MS);
        let Some(cursor) = persisted else {
            return;
        };
        if changes == self.kept_changes || !due {
            return;
        }
        self.kept_changes = changes;
        self.kept_ms = Some(self.now_ms);
        let mut kept = self.network.to_be_bytes().to_vec();
        kept.extend(publishers_digest(self.verifier.unl().config()));
        kept.extend(self.verifier.state());
        if kept.len() > MAX_HUB_VERIFIER_BYTES {
            // What the host kept before is older, and may be of a chain
            // gone: it is dropped once, said once, and a restart starts
            // afresh until the state fits again.
            if self.over_bound {
                return;
            }
            self.over_bound = true;
            self.keeping = Some(Kept::Dropped);
            self.out.push_back(Out::Problem(
                "the XRPL verifier's lists are too large to keep: a restart fetches them and anchors anew",
            ));
        } else {
            self.over_bound = false;
            self.keeping = Some(Kept::State(kept));
        }
        if !(self.out.iter()).any(|out| matches!(out, Out::Cursor(_))) {
            self.out.push_back(Out::Cursor(cursor));
        }
    }
}

/// What tells the publishers a verifier's state was kept under from
/// others: a digest of their master keys, which alone make the UNL, in the
/// order of their bytes, so that a changed site or order is none
/// ([`super::Watcher::restore`]).
fn publishers_digest(config: &UnlConfig) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};

    let mut keys: Vec<&[u8; 33]> = (config.publishers.iter())
        .map(|publisher| publisher.master_key.as_bytes())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let mut hash = Sha256::new();
    hash.update(b"dassie hub verifier publishers");
    for key in keys {
        hash.update(key);
    }
    hash.finalize().into()
}
